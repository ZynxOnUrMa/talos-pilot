//! Network Stats component - displays network interface statistics for a node
//!
//! "Is the network the problem?"

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
use std::collections::HashMap;
use std::time::Instant;
use talos_rs::{NetDevRate, NetDevStats, TalosClient};

/// Auto-refresh interval in seconds (faster than processes for responsive rates)
const AUTO_REFRESH_INTERVAL_SECS: u64 = 2;

/// Sort order for device list
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    #[default]
    Traffic,  // rx_bytes + tx_bytes descending
    Errors,   // errors + dropped descending
}

impl SortBy {
    pub fn label(&self) -> &'static str {
        match self {
            SortBy::Traffic => "TRAFFIC",
            SortBy::Errors => "ERRORS",
        }
    }
}

/// Network stats component for viewing node network interfaces
pub struct NetworkStatsComponent {
    /// Node hostname
    hostname: String,
    /// Node address
    address: String,

    /// Current device statistics
    devices: Vec<NetDevStats>,
    /// Previous device stats (for rate calculation)
    prev_devices: HashMap<String, NetDevStats>,
    /// Calculated rates per device
    rates: HashMap<String, NetDevRate>,
    /// Time of last sample
    last_sample: Option<Instant>,

    /// Selected device index
    selected: usize,
    /// Table state for rendering
    table_state: TableState,
    /// Current sort order
    sort_by: SortBy,

    /// Total RX rate (bytes/sec)
    total_rx_rate: u64,
    /// Total TX rate (bytes/sec)
    total_tx_rate: u64,
    /// Total errors across all devices
    total_errors: u64,
    /// Total dropped across all devices
    total_dropped: u64,

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

impl Default for NetworkStatsComponent {
    fn default() -> Self {
        Self::new("".to_string(), "".to_string())
    }
}

impl NetworkStatsComponent {
    pub fn new(hostname: String, address: String) -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));

        Self {
            hostname,
            address,
            devices: Vec::new(),
            prev_devices: HashMap::new(),
            rates: HashMap::new(),
            last_sample: None,
            selected: 0,
            table_state,
            sort_by: SortBy::Traffic,
            total_rx_rate: 0,
            total_tx_rate: 0,
            total_errors: 0,
            total_dropped: 0,
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

    /// Refresh network data from the node
    pub async fn refresh(&mut self) -> Result<()> {
        let Some(client) = &self.client else {
            self.set_error("No client configured".to_string());
            return Ok(());
        };

        self.loading = true;

        let timeout = std::time::Duration::from_secs(10);
        let result = tokio::time::timeout(timeout, client.network_device_stats()).await;

        match result {
            Ok(Ok(stats)) => {
                if let Some(node_data) = stats.into_iter().next() {
                    self.update_devices(node_data.devices);
                } else {
                    self.devices.clear();
                    self.rates.clear();
                }
            }
            Ok(Err(e)) => {
                self.set_error(format!("Failed to fetch network stats: {} (node: {})", e, self.address));
                return Ok(());
            }
            Err(_) => {
                self.set_error(format!("Request timed out after {}s", timeout.as_secs()));
                return Ok(());
            }
        }

        // Reset selection if needed
        if !self.devices.is_empty() && self.selected >= self.devices.len() {
            self.selected = 0;
        }
        self.table_state.select(Some(self.selected));

        self.loading = false;
        self.error = None;
        self.last_refresh = Some(Instant::now());

        Ok(())
    }

    /// Update devices and calculate rates
    fn update_devices(&mut self, new_devices: Vec<NetDevStats>) {
        let now = Instant::now();
        let elapsed_secs = self.last_sample
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(0.0);

        // Calculate rates if we have previous data
        if elapsed_secs > 0.1 {
            for dev in &new_devices {
                if let Some(prev) = self.prev_devices.get(&dev.name) {
                    let rate = NetDevRate::from_delta(prev, dev, elapsed_secs);
                    self.rates.insert(dev.name.clone(), rate);
                }
            }
        }

        // Store current as previous for next calculation
        self.prev_devices.clear();
        for dev in &new_devices {
            self.prev_devices.insert(dev.name.clone(), dev.clone());
        }
        self.last_sample = Some(now);

        // Calculate totals
        self.total_rx_rate = self.rates.values().map(|r| r.rx_bytes_per_sec).sum();
        self.total_tx_rate = self.rates.values().map(|r| r.tx_bytes_per_sec).sum();
        self.total_errors = new_devices.iter().map(|d| d.total_errors()).sum();
        self.total_dropped = new_devices.iter().map(|d| d.total_dropped()).sum();

        // Sort and store devices
        self.devices = new_devices;
        self.sort_devices();
    }

    /// Sort devices based on current sort order
    fn sort_devices(&mut self) {
        match self.sort_by {
            SortBy::Traffic => {
                // Sort by rate if available, otherwise by cumulative traffic
                self.devices.sort_by(|a, b| {
                    let rate_a = self.rates.get(&a.name).map(|r| r.total_rate()).unwrap_or(0);
                    let rate_b = self.rates.get(&b.name).map(|r| r.total_rate()).unwrap_or(0);
                    if rate_a != rate_b {
                        rate_b.cmp(&rate_a)
                    } else {
                        b.total_traffic().cmp(&a.total_traffic())
                    }
                });
            }
            SortBy::Errors => {
                self.devices.sort_by(|a, b| {
                    let err_a = a.total_errors() + a.total_dropped();
                    let err_b = b.total_errors() + b.total_dropped();
                    err_b.cmp(&err_a)
                });
            }
        }
    }

    /// Navigate to previous device
    fn select_prev(&mut self) {
        if !self.devices.is_empty() && self.selected > 0 {
            self.selected -= 1;
            self.table_state.select(Some(self.selected));
        }
    }

    /// Navigate to next device
    fn select_next(&mut self) {
        if !self.devices.is_empty() {
            self.selected = (self.selected + 1).min(self.devices.len() - 1);
            self.table_state.select(Some(self.selected));
        }
    }

    /// Jump to top of list
    fn select_first(&mut self) {
        if !self.devices.is_empty() {
            self.selected = 0;
            self.table_state.select(Some(self.selected));
        }
    }

    /// Jump to bottom of list
    fn select_last(&mut self) {
        if !self.devices.is_empty() {
            self.selected = self.devices.len() - 1;
            self.table_state.select(Some(self.selected));
        }
    }

    /// Get selected device
    fn selected_device(&self) -> Option<&NetDevStats> {
        self.devices.get(self.selected)
    }

    /// Get rate for a device
    fn get_rate(&self, name: &str) -> Option<&NetDevRate> {
        self.rates.get(name)
    }

    /// Draw the header
    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let device_count = format!("{} ifaces", self.devices.len());

        let auto_indicator = if self.auto_refresh { "" } else { " [AUTO:OFF]" };

        let spans = vec![
            Span::styled("Network: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&self.hostname),
            Span::styled(" (", Style::default().fg(Color::DarkGray)),
            Span::raw(&self.address),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(&device_count, Style::default().fg(Color::DarkGray)),
            Span::styled(auto_indicator, Style::default().fg(Color::Yellow)),
        ];

        let header = Paragraph::new(Line::from(spans));
        frame.render_widget(header, area);
    }

    /// Draw the summary bar
    fn draw_summary_bar(&self, frame: &mut Frame, area: Rect) {
        let has_errors = self.total_errors > 0 || self.total_dropped > 0;
        let warning = if has_errors { "! " } else { "" };

        let rx_rate = NetDevStats::format_rate(self.total_rx_rate);
        let tx_rate = NetDevStats::format_rate(self.total_tx_rate);

        let mut spans = vec![
            Span::styled(warning, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled("Total:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("RX ", Style::default().fg(Color::Green)),
            Span::raw(&rx_rate),
            Span::raw("  "),
            Span::styled("TX ", Style::default().fg(Color::Blue)),
            Span::raw(&tx_rate),
        ];

        // Add errors/dropped if any
        spans.push(Span::raw("   "));
        let errors_style = if self.total_errors > 0 {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!("Errors: {}", self.total_errors), errors_style));

        spans.push(Span::raw("   "));
        let dropped_style = if self.total_dropped > 0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!("Dropped: {}", self.total_dropped), dropped_style));

        let summary = Paragraph::new(Line::from(spans));
        frame.render_widget(summary, area);
    }

    /// Draw warning banner if there are errors
    fn draw_warning(&self, frame: &mut Frame, area: Rect) {
        let mut messages = Vec::new();

        if self.total_errors > 0 {
            messages.push(format!("{} interface errors detected", self.total_errors));
        }
        if self.total_dropped > 0 {
            messages.push(format!("{} dropped packets", self.total_dropped));
        }

        if !messages.is_empty() {
            let warning = Paragraph::new(Line::from(vec![
                Span::styled("! ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(messages.join(" | "), Style::default().fg(Color::Yellow)),
            ]));
            frame.render_widget(warning, area);
        }
    }

    /// Draw the device table
    fn draw_device_table(&mut self, frame: &mut Frame, area: Rect) {
        // Build column headers with sort indicators
        let rx_rate_header = if self.sort_by == SortBy::Traffic { "RX RATE▼" } else { "RX RATE" };
        let rx_err_header = if self.sort_by == SortBy::Errors { "RX ERR▼" } else { "RX ERR" };

        let header_cells = [
            Cell::from("INTERFACE"),
            Cell::from(rx_rate_header),
            Cell::from("TX RATE"),
            Cell::from(rx_err_header),
            Cell::from("TX ERR"),
            Cell::from("RX DROP"),
            Cell::from("TX DROP"),
        ];
        let header = Row::new(header_cells)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .height(1);

        let rows: Vec<Row> = self.devices.iter().enumerate().map(|(idx, dev)| {
            let rate = self.get_rate(&dev.name);
            let rx_rate = rate.map(|r| NetDevStats::format_rate(r.rx_bytes_per_sec))
                .unwrap_or_else(|| "0 B/s".to_string());
            let tx_rate = rate.map(|r| NetDevStats::format_rate(r.tx_bytes_per_sec))
                .unwrap_or_else(|| "0 B/s".to_string());

            let has_errors = dev.has_errors();
            let is_selected = idx == self.selected;

            // Row style based on errors and selection
            let row_style = if has_errors {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            // Error column styles
            let rx_err_style = if dev.rx_errors > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let tx_err_style = if dev.tx_errors > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let rx_drop_style = if dev.rx_dropped > 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let tx_drop_style = if dev.tx_dropped > 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            Row::new([
                Cell::from(dev.name.clone()).style(row_style),
                Cell::from(rx_rate).style(Style::default().fg(Color::Green)),
                Cell::from(tx_rate).style(Style::default().fg(Color::Blue)),
                Cell::from(dev.rx_errors.to_string()).style(rx_err_style),
                Cell::from(dev.tx_errors.to_string()).style(tx_err_style),
                Cell::from(dev.rx_dropped.to_string()).style(rx_drop_style),
                Cell::from(dev.tx_dropped.to_string()).style(tx_drop_style),
            ])
        }).collect();

        let widths = [
            Constraint::Length(14),   // INTERFACE
            Constraint::Length(12),   // RX RATE
            Constraint::Length(12),   // TX RATE
            Constraint::Length(8),    // RX ERR
            Constraint::Length(8),    // TX ERR
            Constraint::Length(8),    // RX DROP
            Constraint::Length(8),    // TX DROP
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().bg(Color::DarkGray));

        frame.render_stateful_widget(table, area, &mut self.table_state);
    }

    /// Draw the detail section for selected device
    fn draw_detail_section(&self, frame: &mut Frame, area: Rect) {
        let Some(dev) = self.selected_device() else {
            return;
        };

        let rate = self.get_rate(&dev.name);
        let rx_rate = rate.map(|r| NetDevStats::format_rate(r.rx_bytes_per_sec))
            .unwrap_or_else(|| "0 B/s".to_string());
        let tx_rate = rate.map(|r| NetDevStats::format_rate(r.tx_bytes_per_sec))
            .unwrap_or_else(|| "0 B/s".to_string());

        let rx_total = NetDevStats::format_bytes(dev.rx_bytes);
        let tx_total = NetDevStats::format_bytes(dev.tx_bytes);

        let has_errors = dev.has_errors();
        let border_style = if has_errors {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let lines = vec![
            Line::from(vec![
                Span::styled("RX: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(format!("{} total", rx_total)),
                Span::styled(format!(" ({})", rx_rate), Style::default().fg(Color::DarkGray)),
                Span::raw(format!("    Packets: {}M", dev.rx_packets / 1_000_000)),
                Span::raw("    "),
                Span::styled(format!("Errors: {}", dev.rx_errors),
                    if dev.rx_errors > 0 { Style::default().fg(Color::Red) } else { Style::default().fg(Color::DarkGray) }),
                Span::raw("    "),
                Span::styled(format!("Dropped: {}", dev.rx_dropped),
                    if dev.rx_dropped > 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) }),
            ]),
            Line::from(vec![
                Span::styled("TX: ", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                Span::raw(format!("{} total", tx_total)),
                Span::styled(format!(" ({})", tx_rate), Style::default().fg(Color::DarkGray)),
                Span::raw(format!("    Packets: {}M", dev.tx_packets / 1_000_000)),
                Span::raw("    "),
                Span::styled(format!("Errors: {}", dev.tx_errors),
                    if dev.tx_errors > 0 { Style::default().fg(Color::Red) } else { Style::default().fg(Color::DarkGray) }),
                Span::raw("    "),
                Span::styled(format!("Dropped: {}", dev.tx_dropped),
                    if dev.tx_dropped > 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) }),
            ]),
        ];

        // Add warning line if there are errors
        let lines = if has_errors {
            let mut lines = lines;
            lines.push(Line::from(vec![
                Span::styled("! ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled("Interface has errors - check cable/driver/hardware", Style::default().fg(Color::Yellow)),
            ]));
            lines
        } else {
            lines
        };

        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(border_style)
            .title(Span::styled(
                format!(" {} ", dev.name),
                Style::default().add_modifier(Modifier::BOLD),
            ));

        let detail = Paragraph::new(lines).block(block);
        frame.render_widget(detail, area);
    }

    /// Draw the footer with keybindings
    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let auto_label = if self.auto_refresh { "auto:ON" } else { "auto:OFF" };

        let spans = vec![
            Span::styled("[1]", Style::default().fg(Color::Cyan)),
            Span::raw(" traffic  "),
            Span::styled("[2]", Style::default().fg(Color::Cyan)),
            Span::raw(" errors  "),
            Span::styled("[r]", Style::default().fg(Color::Cyan)),
            Span::raw(" refresh  "),
            Span::styled("[a]", Style::default().fg(Color::Cyan)),
            Span::raw(format!(" {}  ", auto_label)),
            Span::raw("          "),
            Span::styled("[q]", Style::default().fg(Color::Cyan)),
            Span::raw(" back"),
        ];

        let footer = Paragraph::new(Line::from(spans))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(footer, area);
    }
}

impl Component for NetworkStatsComponent {
    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
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
                self.sort_by = SortBy::Traffic;
                self.sort_devices();
                Ok(None)
            }
            KeyCode::Char('2') => {
                self.sort_by = SortBy::Errors;
                self.sort_devices();
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
            let loading = Paragraph::new("Loading network stats...")
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
        let has_warning = self.total_errors > 0 || self.total_dropped > 0;
        let warning_height = if has_warning { 1 } else { 0 };

        let chunks = Layout::vertical([
            Constraint::Length(1),                    // Header
            Constraint::Length(1),                    // Summary bar
            Constraint::Length(warning_height),       // Warning (if any)
            Constraint::Min(5),                       // Device table
            Constraint::Length(4),                    // Detail section
            Constraint::Length(1),                    // Footer
        ])
        .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_summary_bar(frame, chunks[1]);

        if has_warning {
            self.draw_warning(frame, chunks[2]);
        }

        self.draw_device_table(frame, chunks[3]);
        self.draw_detail_section(frame, chunks[4]);
        self.draw_footer(frame, chunks[5]);

        Ok(())
    }
}
