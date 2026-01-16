//! Insecure mode component - for connecting to maintenance mode nodes
//!
//! This component provides a simplified UI for nodes that haven't been
//! bootstrapped yet. It shows disk and volume information without requiring
//! TLS client certificates.

use crate::action::Action;
use crate::components::Component;
use color_eyre::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
};
use talos_pilot_core::AsyncState;
use talos_rs::{
    DiskInfo, InsecureVersionInfo, VolumeStatus, get_disks_insecure, get_version_insecure,
    get_volume_status_insecure,
};

/// View mode for the insecure component
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InsecureViewMode {
    #[default]
    Disks,
    Volumes,
}

impl InsecureViewMode {
    pub fn next(&self) -> Self {
        match self {
            InsecureViewMode::Disks => InsecureViewMode::Volumes,
            InsecureViewMode::Volumes => InsecureViewMode::Disks,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            InsecureViewMode::Disks => "Disks",
            InsecureViewMode::Volumes => "Volumes",
        }
    }
}

/// Data loaded in insecure mode
#[derive(Debug, Clone, Default)]
pub struct InsecureData {
    /// Endpoint address
    pub endpoint: String,
    /// Version info (if available)
    pub version: Option<InsecureVersionInfo>,
    /// Physical disks
    pub disks: Vec<DiskInfo>,
    /// Volume status
    pub volumes: Vec<VolumeStatus>,
    /// Whether connected
    pub connected: bool,
}

/// Insecure mode component for maintenance mode nodes
pub struct InsecureComponent {
    /// Async state wrapping all data
    state: AsyncState<InsecureData>,

    /// Endpoint to connect to
    endpoint: String,

    /// Current view mode (Disks or Volumes)
    view_mode: InsecureViewMode,

    /// Table state for disk list
    disk_table_state: TableState,

    /// Table state for volume list
    volume_table_state: TableState,
}

impl InsecureComponent {
    pub fn new(endpoint: String) -> Self {
        let mut disk_table_state = TableState::default();
        disk_table_state.select(Some(0));
        let mut volume_table_state = TableState::default();
        volume_table_state.select(Some(0));

        let initial_data = InsecureData {
            endpoint: endpoint.clone(),
            ..Default::default()
        };

        Self {
            state: AsyncState::with_data(initial_data),
            endpoint,
            view_mode: InsecureViewMode::Disks,
            disk_table_state,
            volume_table_state,
        }
    }

    /// Extract just the IP/hostname from endpoint (strip port if present)
    fn endpoint_for_talosctl(endpoint: &str) -> String {
        // talosctl -n expects just IP, not IP:port
        if let Some(idx) = endpoint.rfind(':') {
            // Check if this is an IPv6 address (contains multiple colons)
            if endpoint.matches(':').count() > 1 {
                // IPv6 - return as-is (talosctl handles it)
                endpoint.to_string()
            } else {
                // IPv4 with port - strip the port
                endpoint[..idx].to_string()
            }
        } else {
            endpoint.to_string()
        }
    }

    /// Connect and load data
    pub async fn connect(&mut self) -> Result<()> {
        self.state.start_loading();

        let endpoint = Self::endpoint_for_talosctl(&self.endpoint);
        let mut data = InsecureData {
            endpoint: endpoint.clone(),
            ..Default::default()
        };

        // Try to get version info (may fail in maintenance mode)
        match get_version_insecure(&endpoint).await {
            Ok(version) => {
                data.version = Some(version);
            }
            Err(e) => {
                tracing::debug!("Version info not available: {}", e);
            }
        }

        // Fetch disks
        match get_disks_insecure(&endpoint).await {
            Ok(disks) => {
                data.disks = disks;
                data.connected = true;
            }
            Err(e) => {
                self.state
                    .set_error(format!("Failed to connect: {}", e));
                return Ok(());
            }
        }

        // Fetch volumes
        match get_volume_status_insecure(&endpoint).await {
            Ok(volumes) => {
                data.volumes = volumes;
            }
            Err(e) => {
                tracing::debug!("Volume info not available: {}", e);
            }
        }

        self.state.set_data(data);
        Ok(())
    }

    /// Refresh data
    pub async fn refresh(&mut self) -> Result<()> {
        self.connect().await
    }

    /// Helper to get data reference
    fn data(&self) -> Option<&InsecureData> {
        self.state.data()
    }

    /// Get selected disk index
    fn selected_disk_index(&self) -> usize {
        self.disk_table_state.selected().unwrap_or(0)
    }

    /// Get selected volume index
    fn selected_volume_index(&self) -> usize {
        self.volume_table_state.selected().unwrap_or(0)
    }

    /// Move selection up
    fn select_prev(&mut self) {
        match self.view_mode {
            InsecureViewMode::Disks => {
                if let Some(data) = self.data()
                    && !data.disks.is_empty()
                {
                    let i = self.selected_disk_index();
                    let new_i = if i == 0 { data.disks.len() - 1 } else { i - 1 };
                    self.disk_table_state.select(Some(new_i));
                }
            }
            InsecureViewMode::Volumes => {
                if let Some(data) = self.data()
                    && !data.volumes.is_empty()
                {
                    let i = self.selected_volume_index();
                    let new_i = if i == 0 {
                        data.volumes.len() - 1
                    } else {
                        i - 1
                    };
                    self.volume_table_state.select(Some(new_i));
                }
            }
        }
    }

    /// Move selection down
    fn select_next(&mut self) {
        match self.view_mode {
            InsecureViewMode::Disks => {
                if let Some(data) = self.data()
                    && !data.disks.is_empty()
                {
                    let i = self.selected_disk_index();
                    let new_i = (i + 1) % data.disks.len();
                    self.disk_table_state.select(Some(new_i));
                }
            }
            InsecureViewMode::Volumes => {
                if let Some(data) = self.data()
                    && !data.volumes.is_empty()
                {
                    let i = self.selected_volume_index();
                    let new_i = (i + 1) % data.volumes.len();
                    self.volume_table_state.select(Some(new_i));
                }
            }
        }
    }

    /// Draw the warning banner
    fn draw_warning_banner(&self, frame: &mut Frame, area: Rect) {
        let warning = Paragraph::new(Line::from(vec![
            Span::styled(
                " ⚠ INSECURE MODE ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Connected without TLS authentication - "),
            Span::styled("Limited functionality", Style::default().fg(Color::Yellow)),
        ]))
        .style(Style::default().bg(Color::DarkGray));

        frame.render_widget(warning, area);
    }

    /// Draw connection info
    fn draw_connection_info(&self, frame: &mut Frame, area: Rect) {
        let data = self.data();

        let status = if let Some(d) = data {
            if d.connected {
                let version_str = d
                    .version
                    .as_ref()
                    .map(|v| {
                        if v.maintenance_mode {
                            "Maintenance Mode".to_string()
                        } else {
                            format!("Talos {}", v.tag)
                        }
                    })
                    .unwrap_or_else(|| "Maintenance Mode".to_string());

                Line::from(vec![
                    Span::styled("Endpoint: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(&d.endpoint, Style::default().fg(Color::White)),
                    Span::raw("  │  "),
                    Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(version_str, Style::default().fg(Color::Green)),
                ])
            } else {
                Line::from(vec![
                    Span::styled("Endpoint: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(&self.endpoint, Style::default().fg(Color::White)),
                    Span::raw("  │  "),
                    Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Disconnected", Style::default().fg(Color::Red)),
                ])
            }
        } else if self.state.is_loading() {
            Line::from(vec![
                Span::styled("Connecting to ", Style::default().fg(Color::DarkGray)),
                Span::styled(&self.endpoint, Style::default().fg(Color::White)),
                Span::raw("..."),
            ])
        } else {
            Line::from(vec![Span::styled(
                "Not connected",
                Style::default().fg(Color::Red),
            )])
        };

        let info = Paragraph::new(status);
        frame.render_widget(info, area);
    }

    /// Draw tabs for switching between Disks and Volumes
    fn draw_tabs(&self, frame: &mut Frame, area: Rect) {
        let tabs = Line::from(vec![
            Span::raw(" "),
            if self.view_mode == InsecureViewMode::Disks {
                Span::styled(
                    " Disks ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(" Disks ", Style::default().fg(Color::DarkGray))
            },
            Span::raw(" "),
            if self.view_mode == InsecureViewMode::Volumes {
                Span::styled(
                    " Volumes ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(" Volumes ", Style::default().fg(Color::DarkGray))
            },
            Span::raw("  "),
            Span::styled("[Tab]", Style::default().fg(Color::DarkGray)),
            Span::styled(" switch view", Style::default().fg(Color::DarkGray)),
        ]);

        frame.render_widget(Paragraph::new(tabs), area);
    }

    /// Draw disk table
    fn draw_disks(&mut self, frame: &mut Frame, area: Rect) {
        let data = self.data();
        let empty_disks: Vec<DiskInfo> = vec![];
        let disks = data.map(|d| &d.disks).unwrap_or(&empty_disks);

        let header = Row::new(vec![
            Cell::from("Device").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("Size").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("Type").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("Transport").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("Model").style(Style::default().add_modifier(Modifier::BOLD)),
        ])
        .height(1)
        .style(Style::default().fg(Color::Cyan));

        let rows: Vec<Row> = disks
            .iter()
            .map(|disk| {
                let disk_type = if disk.readonly {
                    ("CD-ROM", Color::Magenta)
                } else if disk.rotational {
                    ("HDD", Color::Yellow)
                } else {
                    ("SSD", Color::Green)
                };

                Row::new(vec![
                    Cell::from(disk.dev_path.clone()),
                    Cell::from(disk.size_pretty.clone()),
                    Cell::from(disk_type.0).style(Style::default().fg(disk_type.1)),
                    Cell::from(disk.transport.clone().unwrap_or_default()),
                    Cell::from(disk.model.clone().unwrap_or_default()),
                ])
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(15),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(12),
                Constraint::Fill(1),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Disks ({}) ", disks.len())),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray));

        frame.render_stateful_widget(table, area, &mut self.disk_table_state);
    }

    /// Draw volume table
    fn draw_volumes(&mut self, frame: &mut Frame, area: Rect) {
        let data = self.data();
        let empty_volumes: Vec<VolumeStatus> = vec![];
        let volumes = data.map(|d| &d.volumes).unwrap_or(&empty_volumes);

        let header = Row::new(vec![
            Cell::from("Volume").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("Size").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("Phase").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("Location").style(Style::default().add_modifier(Modifier::BOLD)),
        ])
        .height(1)
        .style(Style::default().fg(Color::Cyan));

        let rows: Vec<Row> = volumes
            .iter()
            .map(|vol| {
                let phase_color = match vol.phase.as_str() {
                    "ready" => Color::Green,
                    "waiting" => Color::Yellow,
                    _ => Color::Red,
                };

                Row::new(vec![
                    Cell::from(vol.id.clone()),
                    Cell::from(vol.size.clone()),
                    Cell::from(vol.phase.clone()).style(Style::default().fg(phase_color)),
                    Cell::from(vol.mount_location.clone().unwrap_or_default()),
                ])
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(20),
                Constraint::Length(12),
                Constraint::Length(10),
                Constraint::Fill(1),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Volumes ({}) ", volumes.len())),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray));

        frame.render_stateful_widget(table, area, &mut self.volume_table_state);
    }

    /// Draw help text
    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let help = Line::from(vec![
            Span::styled(" [Tab] ", Style::default().fg(Color::Cyan)),
            Span::raw("Switch view"),
            Span::raw("  "),
            Span::styled(" [↑/↓] ", Style::default().fg(Color::Cyan)),
            Span::raw("Navigate"),
            Span::raw("  "),
            Span::styled(" [r] ", Style::default().fg(Color::Cyan)),
            Span::raw("Refresh"),
            Span::raw("  "),
            Span::styled(" [q] ", Style::default().fg(Color::Cyan)),
            Span::raw("Quit"),
        ]);

        frame.render_widget(Paragraph::new(help), area);
    }
}

impl Component for InsecureComponent {
    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Ok(Some(Action::Quit)),
            KeyCode::Char('r') => Ok(Some(Action::Refresh)),
            KeyCode::Tab => {
                self.view_mode = self.view_mode.next();
                Ok(None)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_prev();
                Ok(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn update(&mut self, _action: Action) -> Result<Option<Action>> {
        // No tick-based updates needed for insecure mode
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> Result<()> {
        // Layout: warning banner, connection info, tabs, content, help
        let layout = Layout::vertical([
            Constraint::Length(1), // Warning banner
            Constraint::Length(1), // Connection info
            Constraint::Length(1), // Tabs
            Constraint::Fill(1),   // Content
            Constraint::Length(1), // Help
        ])
        .split(area);

        // Draw warning banner
        self.draw_warning_banner(frame, layout[0]);

        // Draw connection info
        self.draw_connection_info(frame, layout[1]);

        // Draw tabs
        self.draw_tabs(frame, layout[2]);

        // Draw content based on state
        if self.state.is_loading() && !self.state.has_data() {
            let loading = Paragraph::new("Connecting...")
                .style(Style::default().fg(Color::Yellow))
                .block(Block::default().borders(Borders::ALL));
            frame.render_widget(loading, layout[3]);
        } else if let Some(error) = self.state.error() {
            let error_widget = Paragraph::new(error.to_string())
                .style(Style::default().fg(Color::Red))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Error ")
                        .border_style(Style::default().fg(Color::Red)),
                );
            frame.render_widget(error_widget, layout[3]);
        } else {
            match self.view_mode {
                InsecureViewMode::Disks => self.draw_disks(frame, layout[3]),
                InsecureViewMode::Volumes => self.draw_volumes(frame, layout[3]),
            }
        }

        // Draw help
        self.draw_help(frame, layout[4]);

        Ok(())
    }
}
