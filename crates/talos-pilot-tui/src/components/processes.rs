//! Processes component - displays running processes on a node
//!
//! "Show me what's wrong in 5 seconds"

use crate::action::Action;
use crate::components::Component;
use color_eyre::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Frame,
};
use std::time::Instant;
use talos_rs::{ProcessInfo, ProcessState, TalosClient};

/// Auto-refresh interval in seconds
const AUTO_REFRESH_INTERVAL_SECS: u64 = 5;

/// Sort order for process list
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    #[default]
    Cpu,
    Mem,
}

impl SortBy {
    pub fn label(&self) -> &'static str {
        match self {
            SortBy::Cpu => "CPU",
            SortBy::Mem => "MEM",
        }
    }
}

/// Component mode
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Filtering,
}

/// State counts for summary bar
#[derive(Debug, Clone, Default)]
struct StateCounts {
    running: usize,
    sleeping: usize,
    disk_wait: usize,
    zombie: usize,
}

/// Processes component for viewing node processes
pub struct ProcessesComponent {
    /// Node hostname
    hostname: String,
    /// Node address
    address: String,

    /// All processes from the node
    processes: Vec<ProcessInfo>,
    /// Filtered process indices (into processes vec)
    filtered_indices: Vec<usize>,

    /// Selected index in filtered list
    selected: usize,
    /// Table state for rendering
    table_state: TableState,
    /// Current sort order
    sort_by: SortBy,
    /// Tree view enabled
    tree_view: bool,

    /// Current mode
    mode: Mode,
    /// Filter input text
    filter_input: String,
    /// Active filter (applied)
    filter: Option<String>,

    /// State counts for summary
    state_counts: StateCounts,
    /// Total memory on node (for percentage calc)
    total_memory: u64,

    /// Loading state
    loading: bool,
    /// Error message
    error: Option<String>,

    /// Auto-refresh enabled
    auto_refresh: bool,
    /// Last refresh time
    last_refresh: Option<Instant>,

    /// Client for API calls
    client: Option<TalosClient>,
}

impl Default for ProcessesComponent {
    fn default() -> Self {
        Self::new("".to_string(), "".to_string())
    }
}

impl ProcessesComponent {
    pub fn new(hostname: String, address: String) -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));

        Self {
            hostname,
            address,
            processes: Vec::new(),
            filtered_indices: Vec::new(),
            selected: 0,
            table_state,
            sort_by: SortBy::Cpu,
            tree_view: false,
            mode: Mode::Normal,
            filter_input: String::new(),
            filter: None,
            state_counts: StateCounts::default(),
            total_memory: 0,
            loading: true,
            error: None,
            auto_refresh: true,
            last_refresh: None,
            client: None,
        }
    }

    /// Set the client for API calls
    pub fn set_client(&mut self, client: TalosClient) {
        self.client = Some(client);
    }

    /// Set error message
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
        self.loading = false;
    }

    /// Refresh process data from the node
    pub async fn refresh(&mut self) -> Result<()> {
        let Some(client) = &self.client else {
            self.set_error("No client configured".to_string());
            return Ok(());
        };

        self.loading = true;

        // Fetch processes with timeout
        let timeout = std::time::Duration::from_secs(10);
        let fetch_result = tokio::time::timeout(timeout, client.processes()).await;

        let node_processes = match fetch_result {
            Ok(Ok(procs)) => procs,
            Ok(Err(e)) => {
                self.set_error(format!("Failed to fetch processes: {} (node: {})", e, self.address));
                return Ok(());
            }
            Err(_) => {
                self.set_error(format!("Request timed out after {}s", timeout.as_secs()));
                return Ok(());
            }
        };

        // Find processes for our node
        if let Some(node_data) = node_processes.into_iter().next() {
            self.processes = node_data.processes;
            self.calculate_state_counts();
            self.sort_processes();
            self.apply_filter();
        } else {
            self.processes.clear();
            self.filtered_indices.clear();
        }

        // Reset selection if needed
        if !self.filtered_indices.is_empty() && self.selected >= self.filtered_indices.len() {
            self.selected = 0;
        }
        self.table_state.select(Some(self.selected));

        self.loading = false;
        self.error = None;
        self.last_refresh = Some(Instant::now());

        Ok(())
    }

    /// Calculate state counts from processes
    fn calculate_state_counts(&mut self) {
        self.state_counts = StateCounts::default();
        for proc in &self.processes {
            match proc.state {
                ProcessState::Running => self.state_counts.running += 1,
                ProcessState::Sleeping => self.state_counts.sleeping += 1,
                ProcessState::DiskSleep => self.state_counts.disk_wait += 1,
                ProcessState::Zombie => self.state_counts.zombie += 1,
                _ => {}
            }
        }
    }

    /// Sort processes based on current sort order
    fn sort_processes(&mut self) {
        match self.sort_by {
            SortBy::Cpu => {
                self.processes.sort_by(|a, b| {
                    b.cpu_time.partial_cmp(&a.cpu_time).unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            SortBy::Mem => {
                self.processes.sort_by(|a, b| b.resident_memory.cmp(&a.resident_memory));
            }
        }
    }

    /// Apply current filter to processes
    fn apply_filter(&mut self) {
        self.filtered_indices = if let Some(ref filter) = self.filter {
            let filter_lower = filter.to_lowercase();
            self.processes
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.command.to_lowercase().contains(&filter_lower)
                        || p.args.to_lowercase().contains(&filter_lower)
                        || p.executable.to_lowercase().contains(&filter_lower)
                })
                .map(|(i, _)| i)
                .collect()
        } else {
            (0..self.processes.len()).collect()
        };
    }

    /// Get currently selected process
    fn selected_process(&self) -> Option<&ProcessInfo> {
        self.filtered_indices
            .get(self.selected)
            .and_then(|&idx| self.processes.get(idx))
    }

    /// Navigate to previous process
    fn select_prev(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected = self.selected.saturating_sub(1);
            self.table_state.select(Some(self.selected));
        }
    }

    /// Navigate to next process
    fn select_next(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered_indices.len() - 1);
            self.table_state.select(Some(self.selected));
        }
    }

    /// Jump to top of list
    fn select_first(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected = 0;
            self.table_state.select(Some(self.selected));
        }
    }

    /// Jump to bottom of list
    fn select_last(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected = self.filtered_indices.len() - 1;
            self.table_state.select(Some(self.selected));
        }
    }

    /// Get color for process based on CPU time (intensity coloring)
    fn cpu_intensity_color(&self, cpu_time: f64) -> Color {
        // Find max cpu_time for relative coloring
        let max_cpu = self.processes.iter().map(|p| p.cpu_time).fold(0.0, f64::max);
        if max_cpu == 0.0 {
            return Color::default();
        }

        let ratio = cpu_time / max_cpu;
        if ratio > 0.7 {
            Color::Red
        } else if ratio > 0.3 {
            Color::Yellow
        } else {
            Color::default()
        }
    }

    /// Get color for process state
    fn state_color(state: &ProcessState) -> Color {
        match state {
            ProcessState::Running => Color::Green,
            ProcessState::Zombie => Color::Red,
            ProcessState::DiskSleep => Color::Yellow,
            ProcessState::Stopped => Color::Magenta,
            _ => Color::default(),
        }
    }

    /// Draw the header
    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let sort_indicator = format!("[{}▼]", self.sort_by.label());
        let proc_count = format!("{} procs", self.processes.len());

        let title = format!(
            "Processes: {} ({}){}{}",
            self.hostname,
            self.address,
            " ".repeat(area.width.saturating_sub(50) as usize),
            ""
        );

        let line = Line::from(vec![
            Span::styled("Processes: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&self.hostname),
            Span::styled(" (", Style::default().fg(Color::DarkGray)),
            Span::raw(&self.address),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(sort_indicator, Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::styled(proc_count, Style::default().fg(Color::DarkGray)),
        ]);

        let para = Paragraph::new(line);
        frame.render_widget(para, area);
    }

    /// Draw the summary bar with CPU/MEM bars and state counts
    fn draw_summary_bar(&self, frame: &mut Frame, area: Rect) {
        // Create visual CPU and MEM bars (placeholder - would need node totals)
        let state_counts = format!(
            "R:{} S:{} D:{} Z:{}",
            self.state_counts.running,
            self.state_counts.sleeping,
            self.state_counts.disk_wait,
            self.state_counts.zombie
        );

        let mut spans = vec![
            Span::raw("CPU "),
            Span::styled("████████░░", Style::default().fg(Color::Green)),
            Span::raw(" --% "),
            Span::raw("   MEM "),
            Span::styled("█████░░░░░", Style::default().fg(Color::Green)),
            Span::raw(" --% "),
            Span::raw("   "),
        ];

        // Add state counts with colors
        spans.push(Span::styled(
            format!("R:{}", self.state_counts.running),
            Style::default().fg(Color::Green),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("S:{}", self.state_counts.sleeping),
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("D:{}", self.state_counts.disk_wait),
            Style::default().fg(if self.state_counts.disk_wait > 0 {
                Color::Yellow
            } else {
                Color::DarkGray
            }),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("Z:{}", self.state_counts.zombie),
            Style::default().fg(if self.state_counts.zombie > 0 {
                Color::Red
            } else {
                Color::DarkGray
            }),
        ));

        let line = Line::from(spans);
        let para = Paragraph::new(line);
        frame.render_widget(para, area);
    }

    /// Draw warning banner if needed
    fn draw_warning(&self, frame: &mut Frame, area: Rect) -> bool {
        let mut warnings = Vec::new();

        if self.state_counts.zombie > 0 {
            warnings.push(format!("{} zombie process(es) detected", self.state_counts.zombie));
        }

        if warnings.is_empty() {
            return false;
        }

        let warning_text = format!("⚠ {}", warnings.join(" • "));
        let para = Paragraph::new(warning_text)
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(para, area);
        true
    }

    /// Draw the process table
    fn draw_process_table(&mut self, frame: &mut Frame, area: Rect) {
        let rows: Vec<Row> = self
            .filtered_indices
            .iter()
            .filter_map(|&idx| self.processes.get(idx))
            .map(|proc| {
                let cpu_color = self.cpu_intensity_color(proc.cpu_time);
                let state_color = Self::state_color(&proc.state);

                let cpu_str = proc.cpu_time_human();
                let mem_str = proc.resident_memory_human();
                let command = proc.display_command();

                // Truncate command to fit
                let max_cmd_len = area.width.saturating_sub(45) as usize;
                let cmd_display = if command.len() > max_cmd_len {
                    format!("{}...", &command[..max_cmd_len.saturating_sub(3)])
                } else {
                    command.to_string()
                };

                Row::new(vec![
                    Cell::from(format!("{:>6}", proc.pid)),
                    Cell::from(format!("{:>8}", cpu_str)).style(Style::default().fg(cpu_color)),
                    Cell::from(format!("{:>8}", mem_str)),
                    Cell::from(proc.state.short()).style(Style::default().fg(state_color)),
                    Cell::from(cmd_display),
                ])
            })
            .collect();

        let header = Row::new(vec![
            Cell::from("PID"),
            Cell::from("CPU"),
            Cell::from("MEM"),
            Cell::from("S"),
            Cell::from("COMMAND"),
        ])
        .style(Style::default().add_modifier(Modifier::DIM))
        .bottom_margin(1);

        let widths = [
            Constraint::Length(7),      // PID
            Constraint::Length(9),      // CPU
            Constraint::Length(9),      // MEM
            Constraint::Length(2),      // State
            Constraint::Percentage(60), // Command
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        frame.render_stateful_widget(table, area, &mut self.table_state);
    }

    /// Draw the detail section for selected process
    fn draw_detail_section(&self, frame: &mut Frame, area: Rect) {
        let Some(proc) = self.selected_process() else {
            return;
        };

        let title = format!(" {} (PID {}) ", proc.command, proc.pid);

        // Full command line (truncated if too long)
        let full_cmd = proc.display_command();
        let max_len = (area.width as usize).saturating_sub(4);
        let cmd_display = if full_cmd.len() > max_len {
            format!("{}...", &full_cmd[..max_len.saturating_sub(3)])
        } else {
            full_cmd.to_string()
        };

        let content = vec![
            Line::from(vec![
                Span::raw("  "),
                Span::raw(&cmd_display),
            ]),
            Line::from(vec![
                Span::styled("  State: ", Style::default().add_modifier(Modifier::DIM)),
                Span::styled(
                    proc.state.description(),
                    Style::default().fg(Self::state_color(&proc.state)),
                ),
                Span::raw("    "),
                Span::styled("Threads: ", Style::default().add_modifier(Modifier::DIM)),
                Span::raw(proc.threads.to_string()),
                Span::raw("    "),
                Span::styled("Virtual: ", Style::default().add_modifier(Modifier::DIM)),
                Span::raw(proc.virtual_memory_human()),
                Span::raw("    "),
                Span::styled("Resident: ", Style::default().add_modifier(Modifier::DIM)),
                Span::raw(proc.resident_memory_human()),
            ]),
        ];

        let block = Block::default().title(title).borders(Borders::TOP);
        let para = Paragraph::new(content).block(block);
        frame.render_widget(para, area);
    }

    /// Draw the footer with keybindings
    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let auto_status = if self.auto_refresh {
            Span::styled("ON ", Style::default().fg(Color::Green))
        } else {
            Span::styled("OFF", Style::default().fg(Color::DarkGray))
        };

        let line = Line::from(vec![
            Span::styled("[1]", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" cpu  "),
            Span::styled("[2]", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" mem  "),
            Span::styled("[/]", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" filter  "),
            Span::styled("[r]", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" refresh  "),
            Span::styled("[a]", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" auto:"),
            auto_status,
            Span::raw("  "),
            Span::styled("[q]", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" back"),
        ]);

        let para = Paragraph::new(line);
        frame.render_widget(para, area);
    }

    /// Draw the filter input bar
    fn draw_filter_bar(&self, frame: &mut Frame, area: Rect) {
        let match_count = self.filtered_indices.len();
        let total = self.processes.len();

        let line = Line::from(vec![
            Span::styled("Filter: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&self.filter_input),
            Span::styled("█", Style::default().fg(Color::Cyan)), // Cursor
            Span::raw("  "),
            Span::styled(
                format!("[{}/{}]", match_count, total),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        let para = Paragraph::new(line);
        frame.render_widget(para, area);
    }
}

impl Component for ProcessesComponent {
    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match self.mode {
            Mode::Normal => self.handle_normal_key(key),
            Mode::Filtering => self.handle_filter_key(key),
        }
    }

    fn update(&mut self, action: Action) -> Result<Option<Action>> {
        if let Action::Tick = action {
            // Check for auto-refresh
            if self.auto_refresh && !self.loading {
                if let Some(last) = self.last_refresh {
                    let interval = std::time::Duration::from_secs(AUTO_REFRESH_INTERVAL_SECS);
                    if last.elapsed() >= interval {
                        return Ok(Some(Action::Refresh));
                    }
                }
            }
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> Result<()> {
        if self.loading {
            let loading = Paragraph::new("Loading processes...")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(loading, area);
            return Ok(());
        }

        if let Some(ref err) = self.error {
            let error = Paragraph::new(format!("Error: {}", err))
                .style(Style::default().fg(Color::Red));
            frame.render_widget(error, area);
            return Ok(());
        }

        // Calculate layout
        let has_warning = self.state_counts.zombie > 0;
        let warning_height = if has_warning { 1 } else { 0 };

        let chunks = Layout::vertical([
            Constraint::Length(1),                    // Header
            Constraint::Length(1),                    // Summary bar
            Constraint::Length(warning_height),       // Warning (if any)
            Constraint::Length(1),                    // Filter bar (in filter mode) or spacer
            Constraint::Min(5),                       // Process table
            Constraint::Length(4),                    // Detail section
            Constraint::Length(1),                    // Footer
        ])
        .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_summary_bar(frame, chunks[1]);

        if has_warning {
            self.draw_warning(frame, chunks[2]);
        }

        // Filter bar or spacer
        if self.mode == Mode::Filtering || self.filter.is_some() {
            self.draw_filter_bar(frame, chunks[3]);
        }

        self.draw_process_table(frame, chunks[4]);
        self.draw_detail_section(frame, chunks[5]);
        self.draw_footer(frame, chunks[6]);

        Ok(())
    }
}

impl ProcessesComponent {
    fn handle_normal_key(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Ok(Some(Action::Back)),
            KeyCode::Char('j') | KeyCode::Down => {
                self.select_next();
                Ok(None)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.select_prev();
                Ok(None)
            }
            KeyCode::Char('g') => {
                self.select_first();
                Ok(None)
            }
            KeyCode::Char('G') => {
                self.select_last();
                Ok(None)
            }
            KeyCode::Char('1') => {
                self.sort_by = SortBy::Cpu;
                self.sort_processes();
                self.apply_filter();
                self.selected = 0;
                self.table_state.select(Some(0));
                Ok(None)
            }
            KeyCode::Char('2') => {
                self.sort_by = SortBy::Mem;
                self.sort_processes();
                self.apply_filter();
                self.selected = 0;
                self.table_state.select(Some(0));
                Ok(None)
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Filtering;
                self.filter_input = self.filter.clone().unwrap_or_default();
                Ok(None)
            }
            KeyCode::Char('r') => Ok(Some(Action::Refresh)),
            KeyCode::Char('a') => {
                self.auto_refresh = !self.auto_refresh;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            KeyCode::Esc => {
                // Clear filter and exit filter mode
                self.mode = Mode::Normal;
                self.filter = None;
                self.filter_input.clear();
                self.apply_filter();
                self.selected = 0;
                self.table_state.select(Some(0));
                Ok(None)
            }
            KeyCode::Enter => {
                // Apply filter and exit filter mode
                self.mode = Mode::Normal;
                self.filter = if self.filter_input.is_empty() {
                    None
                } else {
                    Some(self.filter_input.clone())
                };
                self.apply_filter();
                self.selected = 0;
                self.table_state.select(Some(0));
                Ok(None)
            }
            KeyCode::Backspace => {
                self.filter_input.pop();
                // Live filter as you type
                self.filter = if self.filter_input.is_empty() {
                    None
                } else {
                    Some(self.filter_input.clone())
                };
                self.apply_filter();
                Ok(None)
            }
            KeyCode::Char(c) => {
                self.filter_input.push(c);
                // Live filter as you type
                self.filter = Some(self.filter_input.clone());
                self.apply_filter();
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}
