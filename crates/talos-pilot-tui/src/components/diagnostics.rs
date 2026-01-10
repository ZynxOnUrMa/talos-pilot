//! Diagnostics component for Talos node health checks
//!
//! Provides system health checks, Kubernetes component status,
//! and actionable fixes for common issues.

use crate::action::Action;
use crate::components::Component;
use color_eyre::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState},
    Frame,
};
use std::time::Instant;
use talos_rs::{ApplyConfigResult, ApplyMode, TalosClient};

/// Default auto-refresh interval in seconds
const AUTO_REFRESH_INTERVAL_SECS: u64 = 10;

/// Status of a diagnostic check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    /// Check passed
    Pass,
    /// Warning condition
    Warn,
    /// Check failed - action may be available
    Fail,
    /// Status unknown or still checking
    Unknown,
    /// Currently checking
    Checking,
}

impl CheckStatus {
    /// Get indicator character and color
    pub fn indicator(&self) -> (&'static str, Color) {
        match self {
            CheckStatus::Pass => ("●", Color::Green),
            CheckStatus::Warn => ("◐", Color::Yellow),
            CheckStatus::Fail => ("✗", Color::Red),
            CheckStatus::Unknown => ("?", Color::DarkGray),
            CheckStatus::Checking => ("◌", Color::Cyan),
        }
    }
}

/// Action that can be taken to fix an issue
#[derive(Debug, Clone)]
pub enum FixAction {
    /// Add a kernel module via machine config
    AddKernelModule(String),
    /// Restart a service
    RestartService(String),
    /// Apply a YAML config patch
    ApplyConfigPatch { yaml: String, requires_reboot: bool },
    /// Show more details (navigate to logs, etc.)
    ShowDetails(String),
    /// Install Cilium CNI
    InstallCilium,
}

impl FixAction {
    /// Get a short description of the fix
    pub fn description(&self) -> String {
        match self {
            FixAction::AddKernelModule(name) => format!("Add {} kernel module", name),
            FixAction::RestartService(name) => format!("Restart {} service", name),
            FixAction::ApplyConfigPatch { requires_reboot, .. } => {
                if *requires_reboot {
                    "Apply config patch (requires reboot)".to_string()
                } else {
                    "Apply config patch".to_string()
                }
            }
            FixAction::ShowDetails(_) => "View details".to_string(),
            FixAction::InstallCilium => "Install Cilium CNI".to_string(),
        }
    }

    /// Check if this action requires reboot
    pub fn requires_reboot(&self) -> bool {
        matches!(
            self,
            FixAction::AddKernelModule(_) | FixAction::ApplyConfigPatch { requires_reboot: true, .. }
        )
    }
}

/// A diagnostic fix with description
#[derive(Debug, Clone)]
pub struct DiagnosticFix {
    /// Description of what this fix does
    pub description: String,
    /// The action to take
    pub action: FixAction,
}

/// A single diagnostic check result
#[derive(Debug, Clone)]
pub struct DiagnosticCheck {
    /// Unique identifier for this check
    pub id: String,
    /// Display name of the check
    pub name: String,
    /// Current status
    pub status: CheckStatus,
    /// Status message (e.g., "2.1 GB / 4.0 GB")
    pub message: String,
    /// Additional details (shown when selected)
    pub details: Option<String>,
    /// Available fix if status is Fail or Warn
    pub fix: Option<DiagnosticFix>,
}

impl DiagnosticCheck {
    /// Create a new check with Pass status
    pub fn pass(id: &str, name: &str, message: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            status: CheckStatus::Pass,
            message: message.to_string(),
            details: None,
            fix: None,
        }
    }

    /// Create a new check with Fail status and optional fix
    pub fn fail(id: &str, name: &str, message: &str, fix: Option<DiagnosticFix>) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            status: CheckStatus::Fail,
            message: message.to_string(),
            details: None,
            fix,
        }
    }

    /// Create a new check with Warn status
    pub fn warn(id: &str, name: &str, message: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            status: CheckStatus::Warn,
            message: message.to_string(),
            details: None,
            fix: None,
        }
    }

    /// Create a new check with Unknown status
    pub fn unknown(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            status: CheckStatus::Unknown,
            message: "Unknown".to_string(),
            details: None,
            fix: None,
        }
    }

    /// Set details for this check
    pub fn with_details(mut self, details: &str) -> Self {
        self.details = Some(details.to_string());
        self
    }
}

/// Category of diagnostic checks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckCategory {
    System,
    Kubernetes,
    Services,
}

impl CheckCategory {
    pub fn title(&self) -> &'static str {
        match self {
            CheckCategory::System => "System Health",
            CheckCategory::Kubernetes => "Kubernetes Components",
            CheckCategory::Services => "Services",
        }
    }
}

/// Pending action waiting for confirmation
#[derive(Debug, Clone)]
pub struct PendingAction {
    /// The check this action is for
    pub check_id: String,
    /// The fix to apply
    pub fix: DiagnosticFix,
    /// Preview YAML (for config patches)
    pub preview: Option<String>,
}

/// Diagnostics component for node health checks
pub struct DiagnosticsComponent {
    /// Node hostname
    hostname: String,
    /// Node IP address
    address: String,
    /// Node role (controlplane or worker)
    node_role: String,

    /// System health checks
    system_checks: Vec<DiagnosticCheck>,
    /// Kubernetes component checks
    kubernetes_checks: Vec<DiagnosticCheck>,
    /// Service health checks
    service_checks: Vec<DiagnosticCheck>,

    /// Currently selected category
    selected_category: usize,
    /// Currently selected check within category
    selected_check: usize,
    /// Table state for rendering
    table_state: TableState,

    /// Pending action (waiting for confirmation)
    pending_action: Option<PendingAction>,
    /// Whether we're in the confirmation dialog
    show_confirmation: bool,
    /// Confirmation dialog selection (0 = Cancel, 1 = Apply)
    confirmation_selection: usize,

    /// Whether we're applying a fix
    applying_fix: bool,
    /// Result of the last apply
    apply_result: Option<Result<Vec<ApplyConfigResult>, String>>,

    /// Loading state
    loading: bool,
    /// Error message
    error: Option<String>,
    /// Last refresh time
    last_refresh: Option<Instant>,
    /// Auto-refresh enabled
    auto_refresh: bool,

    /// Client for API calls
    client: Option<TalosClient>,
}

impl Default for DiagnosticsComponent {
    fn default() -> Self {
        Self::new("".to_string(), "".to_string(), "".to_string())
    }
}

impl DiagnosticsComponent {
    pub fn new(hostname: String, address: String, node_role: String) -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));

        Self {
            hostname,
            address,
            node_role,
            system_checks: Vec::new(),
            kubernetes_checks: Vec::new(),
            service_checks: Vec::new(),
            selected_category: 0,
            selected_check: 0,
            table_state,
            pending_action: None,
            show_confirmation: false,
            confirmation_selection: 1, // Default to Apply
            applying_fix: false,
            apply_result: None,
            loading: true,
            error: None,
            last_refresh: None,
            auto_refresh: true,
            client: None,
        }
    }

    /// Set the client for making API calls
    pub fn set_client(&mut self, client: TalosClient) {
        self.client = Some(client);
    }

    /// Set an error message
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
        self.loading = false;
    }

    /// Get all checks in the current category
    fn current_checks(&self) -> &[DiagnosticCheck] {
        match self.selected_category {
            0 => &self.system_checks,
            1 => &self.kubernetes_checks,
            2 => &self.service_checks,
            _ => &[],
        }
    }

    /// Get the currently selected check
    fn selected_check(&self) -> Option<&DiagnosticCheck> {
        self.current_checks().get(self.selected_check)
    }

    /// Get total number of categories
    fn category_count(&self) -> usize {
        3
    }

    /// Select next category
    fn next_category(&mut self) {
        self.selected_category = (self.selected_category + 1) % self.category_count();
        self.selected_check = 0;
        self.update_table_state();
    }

    /// Select previous category
    fn prev_category(&mut self) {
        self.selected_category = if self.selected_category == 0 {
            self.category_count() - 1
        } else {
            self.selected_category - 1
        };
        self.selected_check = 0;
        self.update_table_state();
    }

    /// Select next check in current category
    fn next_check(&mut self) {
        let count = self.current_checks().len();
        if count > 0 {
            self.selected_check = (self.selected_check + 1) % count;
            self.update_table_state();
        }
    }

    /// Select previous check in current category
    fn prev_check(&mut self) {
        let count = self.current_checks().len();
        if count > 0 {
            self.selected_check = if self.selected_check == 0 {
                count - 1
            } else {
                self.selected_check - 1
            };
            self.update_table_state();
        }
    }

    /// Update table state to match selection
    fn update_table_state(&mut self) {
        self.table_state.select(Some(self.selected_check));
    }

    /// Initiate a fix action for the currently selected check
    fn initiate_fix(&mut self) {
        if let Some(check) = self.selected_check() {
            if let Some(fix) = &check.fix {
                // Create preview for config patches
                let preview = match &fix.action {
                    FixAction::AddKernelModule(name) => Some(format!(
                        "machine:\n  kernel:\n    modules:\n      - name: {}",
                        name
                    )),
                    FixAction::ApplyConfigPatch { yaml, .. } => Some(yaml.clone()),
                    _ => None,
                };

                self.pending_action = Some(PendingAction {
                    check_id: check.id.clone(),
                    fix: fix.clone(),
                    preview,
                });
                self.show_confirmation = true;
                self.confirmation_selection = 1; // Default to Apply
            }
        }
    }

    /// Apply the pending fix action
    pub async fn apply_pending_fix(&mut self) -> Result<()> {
        let Some(pending) = self.pending_action.take() else {
            return Ok(());
        };

        let Some(client) = &self.client else {
            self.set_error("No client configured".to_string());
            return Ok(());
        };

        self.applying_fix = true;
        self.show_confirmation = false;

        match &pending.fix.action {
            FixAction::AddKernelModule(name) => {
                let yaml = format!(
                    "machine:\n  kernel:\n    modules:\n      - name: {}",
                    name
                );
                match client
                    .apply_configuration(&yaml, ApplyMode::Reboot, false)
                    .await
                {
                    Ok(results) => {
                        self.apply_result = Some(Ok(results));
                    }
                    Err(e) => {
                        self.apply_result = Some(Err(e.to_string()));
                    }
                }
            }
            FixAction::ApplyConfigPatch { yaml, requires_reboot } => {
                let mode = if *requires_reboot {
                    ApplyMode::Reboot
                } else {
                    ApplyMode::Auto
                };
                match client.apply_configuration(yaml, mode, false).await {
                    Ok(results) => {
                        self.apply_result = Some(Ok(results));
                    }
                    Err(e) => {
                        self.apply_result = Some(Err(e.to_string()));
                    }
                }
            }
            FixAction::RestartService(service) => {
                match client.service_restart(service).await {
                    Ok(_) => {
                        self.apply_result = Some(Ok(vec![]));
                    }
                    Err(e) => {
                        self.apply_result = Some(Err(e.to_string()));
                    }
                }
            }
            FixAction::ShowDetails(_) | FixAction::InstallCilium => {
                // These don't apply directly - they navigate or open wizards
            }
        }

        self.applying_fix = false;
        Ok(())
    }

    /// Refresh diagnostics data from the node
    pub async fn refresh(&mut self) -> Result<()> {
        let Some(client) = &self.client else {
            self.set_error("No client configured".to_string());
            return Ok(());
        };

        self.loading = true;
        self.error = None;

        // Run all checks
        let timeout = std::time::Duration::from_secs(15);

        let result = tokio::time::timeout(timeout, async {
            // Fetch data in parallel
            let (memory_result, load_result, services_result, logs_result, br_netfilter_result) = tokio::join!(
                client.memory(),
                client.load_avg(),
                client.services(),
                client.logs("kubelet", 500),
                client.is_br_netfilter_loaded(),
            );

            // Process results into checks
            let mut system_checks = Vec::new();
            let mut kubernetes_checks = Vec::new();
            let mut service_checks = Vec::new();

            // Memory check
            match memory_result {
                Ok(mem_list) => {
                    if let Some(mem) = mem_list.first() {
                        if let Some(info) = &mem.meminfo {
                            let usage_pct = info.usage_percent();
                            let used_gb = (info.mem_total - info.mem_available) as f64 / 1_073_741_824.0;
                            let total_gb = info.mem_total as f64 / 1_073_741_824.0;
                            let msg = format!("{:.1} / {:.1} GB ({:.0}%)", used_gb, total_gb, usage_pct);

                            if usage_pct > 90.0 {
                                system_checks.push(DiagnosticCheck::fail(
                                    "memory",
                                    "Memory",
                                    &msg,
                                    None,
                                ));
                            } else if usage_pct > 80.0 {
                                system_checks.push(DiagnosticCheck::warn("memory", "Memory", &msg));
                            } else {
                                system_checks.push(DiagnosticCheck::pass("memory", "Memory", &msg));
                            }
                        }
                    }
                }
                Err(e) => {
                    system_checks.push(
                        DiagnosticCheck::unknown("memory", "Memory")
                            .with_details(&format!("Error: {}", e)),
                    );
                }
            }

            // CPU load check
            match load_result {
                Ok(load_list) => {
                    if let Some(load) = load_list.first() {
                        let msg = format!("{:.2} / {:.2} / {:.2}", load.load1, load.load5, load.load15);
                        // Simple heuristic: load > 2x cores is concerning
                        if load.load1 > 4.0 {
                            system_checks.push(DiagnosticCheck::warn("cpu_load", "CPU Load", &msg));
                        } else {
                            system_checks.push(DiagnosticCheck::pass("cpu_load", "CPU Load", &msg));
                        }
                    }
                }
                Err(e) => {
                    system_checks.push(
                        DiagnosticCheck::unknown("cpu_load", "CPU Load")
                            .with_details(&format!("Error: {}", e)),
                    );
                }
            }

            // Kernel modules check - directly check if br_netfilter is loaded
            // by reading /proc/sys/net/bridge/bridge-nf-call-iptables
            let br_netfilter_missing = match &br_netfilter_result {
                Ok(loaded) => {
                    tracing::info!("br_netfilter check: loaded = {}", loaded);
                    !loaded
                }
                Err(e) => {
                    tracing::info!("br_netfilter check error: {}", e);
                    true // If we can't check, assume it's missing
                }
            };

            if br_netfilter_missing {
                system_checks.push(DiagnosticCheck::fail(
                    "kernel_modules",
                    "Kernel Modules",
                    "br_netfilter missing",
                    Some(DiagnosticFix {
                        description: "Add br_netfilter kernel module".to_string(),
                        action: FixAction::AddKernelModule("br_netfilter".to_string()),
                    }),
                ).with_details("The br_netfilter kernel module is required for Kubernetes networking. Without it, CNI plugins like Flannel cannot function properly."));
            } else {
                system_checks.push(DiagnosticCheck::pass(
                    "kernel_modules",
                    "Kernel Modules",
                    "OK",
                ));
            }

            // Service checks
            match services_result {
                Ok(services_list) => {
                    for node_services in services_list {
                        for service in node_services.services {
                            let is_healthy = service
                                .health
                                .as_ref()
                                .map(|h| h.healthy)
                                .unwrap_or(false);

                            let status_msg = format!(
                                "{} ({})",
                                service.state,
                                if is_healthy { "healthy" } else { "unhealthy" }
                            );

                            if is_healthy {
                                service_checks.push(DiagnosticCheck::pass(
                                    &format!("service_{}", service.id),
                                    &service.id,
                                    &status_msg,
                                ));
                            } else {
                                service_checks.push(DiagnosticCheck::fail(
                                    &format!("service_{}", service.id),
                                    &service.id,
                                    &status_msg,
                                    Some(DiagnosticFix {
                                        description: format!("Restart {}", service.id),
                                        action: FixAction::RestartService(service.id.clone()),
                                    }),
                                ));
                            }
                        }
                    }
                }
                Err(e) => {
                    service_checks.push(
                        DiagnosticCheck::unknown("services", "Services")
                            .with_details(&format!("Error: {}", e)),
                    );
                }
            }

            // Kubernetes checks from kubelet logs
            match logs_result {
                Ok(logs) => {
                    // Check for CNI issues
                    let cni_failed = logs.contains("failed to setup network for sandbox")
                        || logs.contains("subnet.env: no such file");
                    let crashloop = logs.contains("CrashLoopBackOff");

                    if cni_failed {
                        // If br_netfilter is missing, that's likely the root cause - offer the fix
                        let fix = if br_netfilter_missing {
                            Some(DiagnosticFix {
                                description: "Add br_netfilter kernel module".to_string(),
                                action: FixAction::AddKernelModule("br_netfilter".to_string()),
                            })
                        } else {
                            None
                        };

                        let details = if br_netfilter_missing {
                            "CNI plugin failed because br_netfilter kernel module is missing. Apply the fix to add the module and reboot the node."
                        } else {
                            "CNI plugin failed to set up pod networking. Check the kernel modules and flannel pod logs."
                        };

                        kubernetes_checks.push(DiagnosticCheck::fail(
                            "cni",
                            "CNI (Flannel)",
                            if br_netfilter_missing { "br_netfilter missing" } else { "Network setup failed" },
                            fix,
                        ).with_details(details));
                    } else {
                        kubernetes_checks.push(DiagnosticCheck::pass("cni", "CNI", "OK"));
                    }

                    if crashloop {
                        kubernetes_checks.push(DiagnosticCheck::warn(
                            "pods_crashing",
                            "Pod Health",
                            "CrashLoopBackOff detected",
                        ));
                    } else {
                        kubernetes_checks.push(DiagnosticCheck::pass(
                            "pods_crashing",
                            "Pod Health",
                            "No issues detected",
                        ));
                    }
                }
                Err(_) => {
                    kubernetes_checks.push(DiagnosticCheck::unknown("cni", "CNI"));
                    kubernetes_checks.push(DiagnosticCheck::unknown("pods_crashing", "Pod Health"));
                }
            }

            // Etcd check (for control plane nodes)
            if self.node_role.contains("controlplane") || self.node_role.contains("control") {
                match client.etcd_status().await {
                    Ok(status_list) => {
                        if let Some(status) = status_list.first() {
                            let is_leader = status.is_leader();
                            let msg = if is_leader {
                                "Leader, healthy".to_string()
                            } else {
                                format!("Follower (leader: {:x})", status.leader_id)
                            };
                            kubernetes_checks.push(DiagnosticCheck::pass("etcd", "Etcd", &msg));
                        }
                    }
                    Err(e) => {
                        kubernetes_checks.push(
                            DiagnosticCheck::fail("etcd", "Etcd", "Unreachable", None)
                                .with_details(&format!("Error: {}", e)),
                        );
                    }
                }
            }

            (system_checks, kubernetes_checks, service_checks)
        })
        .await;

        match result {
            Ok((system, kubernetes, services)) => {
                self.system_checks = system;
                self.kubernetes_checks = kubernetes;
                self.service_checks = services;
                self.last_refresh = Some(Instant::now());
            }
            Err(_) => {
                self.set_error("Timeout fetching diagnostics".to_string());
            }
        }

        self.loading = false;
        Ok(())
    }

    /// Render a category section
    fn render_category(
        &self,
        frame: &mut Frame,
        area: Rect,
        category: CheckCategory,
        checks: &[DiagnosticCheck],
        is_selected: bool,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(category.title())
            .border_style(if is_selected {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if checks.is_empty() {
            let loading = Paragraph::new("Loading...")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(loading, inner);
            return;
        }

        let rows: Vec<Row> = checks
            .iter()
            .enumerate()
            .map(|(i, check)| {
                let (indicator, color) = check.status.indicator();
                let is_current = is_selected && i == self.selected_check;

                let style = if is_current {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    ratatui::widgets::Cell::from(Span::styled(
                        indicator,
                        Style::default().fg(color),
                    )),
                    ratatui::widgets::Cell::from(check.name.clone()),
                    ratatui::widgets::Cell::from(Span::styled(
                        check.message.clone(),
                        Style::default().fg(if check.status == CheckStatus::Pass {
                            Color::Green
                        } else if check.status == CheckStatus::Fail {
                            Color::Red
                        } else if check.status == CheckStatus::Warn {
                            Color::Yellow
                        } else {
                            Color::DarkGray
                        }),
                    )),
                ])
                .style(style)
            })
            .collect();

        let widths = [
            Constraint::Length(2),
            Constraint::Length(20),
            Constraint::Fill(1),
        ];

        let table = Table::new(rows, widths);
        frame.render_widget(table, inner);
    }

    /// Render the confirmation dialog
    fn render_confirmation(&self, frame: &mut Frame, area: Rect) {
        let Some(pending) = &self.pending_action else {
            return;
        };

        // Calculate dialog size and position
        let dialog_width = 70.min(area.width.saturating_sub(4));
        let dialog_height = 15.min(area.height.saturating_sub(4));
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;

        let dialog_area = Rect::new(
            area.x + dialog_x,
            area.y + dialog_y,
            dialog_width,
            dialog_height,
        );

        // Clear the area
        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Confirm Action ")
            .border_style(Style::default().fg(Color::Yellow));

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // Build dialog content
        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("Fix: {}", pending.fix.description),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];

        // Add preview if available
        if let Some(preview) = &pending.preview {
            lines.push(Line::from("This will apply the following configuration:"));
            lines.push(Line::from(""));
            for line in preview.lines().take(5) {
                lines.push(Line::from(Span::styled(
                    format!("  {}", line),
                    Style::default().fg(Color::Cyan),
                )));
            }
            lines.push(Line::from(""));
        }

        // Add reboot warning
        if pending.fix.action.requires_reboot() {
            lines.push(Line::from(Span::styled(
                "⚠ This requires a node reboot to take effect.",
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(""));
        }

        // Add buttons
        let cancel_style = if self.confirmation_selection == 0 {
            Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let apply_style = if self.confirmation_selection == 1 {
            Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };

        lines.push(Line::from(vec![
            Span::raw("         "),
            Span::styled(" Cancel ", cancel_style),
            Span::raw("     "),
            Span::styled(
                if pending.fix.action.requires_reboot() {
                    " Apply & Reboot "
                } else {
                    " Apply "
                },
                apply_style,
            ),
        ]));

        let content = Paragraph::new(lines);
        frame.render_widget(content, inner);
    }
}

impl Component for DiagnosticsComponent {
    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        // Handle confirmation dialog first
        if self.show_confirmation {
            match key.code {
                KeyCode::Left | KeyCode::Char('h') => {
                    self.confirmation_selection = 0;
                    return Ok(None);
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    self.confirmation_selection = 1;
                    return Ok(None);
                }
                KeyCode::Enter => {
                    if self.confirmation_selection == 0 {
                        // Cancel
                        self.show_confirmation = false;
                        self.pending_action = None;
                    } else {
                        // Apply - return action to trigger async apply
                        return Ok(Some(Action::ApplyDiagnosticFix));
                    }
                    return Ok(None);
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.show_confirmation = false;
                    self.pending_action = None;
                    return Ok(None);
                }
                _ => return Ok(None),
            }
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                return Ok(Some(Action::Back));
            }
            KeyCode::Char('r') => {
                return Ok(Some(Action::Refresh));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.next_check();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.prev_check();
            }
            KeyCode::Tab => {
                self.next_category();
            }
            KeyCode::BackTab => {
                self.prev_category();
            }
            KeyCode::Enter => {
                // Try to apply fix for selected check
                self.initiate_fix();
            }
            KeyCode::Char('c') => {
                // Cilium wizard - TODO
            }
            _ => {}
        }

        Ok(None)
    }

    fn update(&mut self, action: Action) -> Result<Option<Action>> {
        match action {
            Action::Tick => {
                // Auto-refresh
                if self.auto_refresh && !self.loading {
                    if let Some(last) = self.last_refresh {
                        if last.elapsed().as_secs() >= AUTO_REFRESH_INTERVAL_SECS {
                            return Ok(Some(Action::Refresh));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> Result<()> {
        // Main layout
        let chunks = Layout::vertical([
            Constraint::Length(3), // Header
            Constraint::Fill(1),   // Content
            Constraint::Length(2), // Footer
        ])
        .split(area);

        // Header
        let header_text = format!(
            " Diagnostics: {} ({}) ",
            self.hostname, self.address
        );
        let header = Paragraph::new(header_text)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(header, chunks[0]);

        // Error display
        if let Some(error) = &self.error {
            let error_msg = Paragraph::new(format!("Error: {}", error))
                .style(Style::default().fg(Color::Red));
            frame.render_widget(error_msg, chunks[1]);
        } else {
            // Content - three category sections
            let content_chunks = Layout::vertical([
                Constraint::Length(6), // System
                Constraint::Length(8), // Kubernetes
                Constraint::Fill(1),   // Services
            ])
            .split(chunks[1]);

            self.render_category(
                frame,
                content_chunks[0],
                CheckCategory::System,
                &self.system_checks,
                self.selected_category == 0,
            );
            self.render_category(
                frame,
                content_chunks[1],
                CheckCategory::Kubernetes,
                &self.kubernetes_checks,
                self.selected_category == 1,
            );
            self.render_category(
                frame,
                content_chunks[2],
                CheckCategory::Services,
                &self.service_checks,
                self.selected_category == 2,
            );
        }

        // Footer with keybindings
        let footer = Paragraph::new(Line::from(vec![
            Span::styled("[j/k]", Style::default().fg(Color::Cyan)),
            Span::raw(" Navigate  "),
            Span::styled("[Tab]", Style::default().fg(Color::Cyan)),
            Span::raw(" Section  "),
            Span::styled("[Enter]", Style::default().fg(Color::Cyan)),
            Span::raw(" Apply fix  "),
            Span::styled("[r]", Style::default().fg(Color::Cyan)),
            Span::raw(" Refresh  "),
            Span::styled("[q]", Style::default().fg(Color::Cyan)),
            Span::raw(" Back"),
        ]));
        frame.render_widget(footer, chunks[2]);

        // Render confirmation dialog if active
        if self.show_confirmation {
            self.render_confirmation(frame, area);
        }

        Ok(())
    }
}
