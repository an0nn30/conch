//! SSH Tunnel manager and new-tunnel dialogs.

use conch_core::models::{SavedTunnel, ServerEntry};
use uuid::Uuid;

/// Minimum size for dialog action buttons.
const BTN_MIN_SIZE: egui::Vec2 = egui::Vec2::new(95.0, 26.0);

fn dialog_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized(BTN_MIN_SIZE, egui::Button::new(egui::RichText::new(label).size(14.0)))
}

fn dialog_button_enabled(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).size(14.0)).min_size(BTN_MIN_SIZE),
    )
}

// ---------------------------------------------------------------------------
// Tunnel manager dialog (table of saved tunnels)
// ---------------------------------------------------------------------------

pub struct TunnelManagerState {
    /// Row index of the currently selected tunnel (if any).
    selected: Option<usize>,
    /// When `Some`, the "New Tunnel" sub-dialog is open.
    pub new_tunnel_form: Option<NewTunnelForm>,
}

impl TunnelManagerState {
    pub fn new() -> Self {
        Self {
            selected: None,
            new_tunnel_form: None,
        }
    }
}

/// Action returned by the tunnel manager dialog each frame.
pub enum TunnelManagerAction {
    None,
    /// User clicked "Activate" on the selected tunnel.
    Activate(Uuid),
    /// User clicked "Stop" on the selected tunnel.
    Stop(Uuid),
    /// User clicked "Delete" on the selected tunnel.
    Delete(Uuid),
    /// User created a new tunnel via the sub-dialog and wants it saved + activated.
    NewTunnel(SavedTunnel),
    /// User closed the manager dialog.
    Close,
}

/// Status of a tunnel for display purposes.
#[derive(Clone, Copy, PartialEq)]
pub enum TunnelStatus {
    Active,
    Inactive,
}

/// Show the main SSH Tunnels manager window. `active_ids` contains the set of
/// tunnel UUIDs that are currently running.
pub fn show_tunnel_manager(
    ctx: &egui::Context,
    state: &mut TunnelManagerState,
    tunnels: &[SavedTunnel],
    active_ids: &[Uuid],
    servers: &[ServerEntry],
) -> TunnelManagerAction {
    let mut action = TunnelManagerAction::None;

    // Handle the nested "New Tunnel" sub-dialog first.
    if let Some(mut form) = state.new_tunnel_form.take() {
        match show_new_tunnel_dialog(ctx, &mut form) {
            NewTunnelAction::Connect => {
                if let Some(tunnel) = form.to_saved_tunnel() {
                    action = TunnelManagerAction::NewTunnel(tunnel);
                } else {
                    // Validation failed — keep dialog open.
                    state.new_tunnel_form = Some(form);
                }
            }
            NewTunnelAction::Cancel => {}
            NewTunnelAction::None => {
                state.new_tunnel_form = Some(form);
            }
        }
    }

    egui::Window::new("SSH Tunnels")
        .collapsible(false)
        .resizable(false)
        .fixed_size([660.0, 420.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            // Hint text (italic).
            ui.label(
                egui::RichText::new(
                    "Tunnels forward localhost:localPort to remoteHost:remotePort via the SSH server.",
                )
                .italics()
                .size(11.0),
            );

            ui.add_space(4.0);

            // Table.
            let avail = ui.available_size();
            let table_height = (avail.y - 40.0).max(80.0);

            egui::ScrollArea::vertical()
                .max_height(table_height)
                .show(ui, |ui| {
                    egui::Grid::new("tunnel_table")
                        .num_columns(5)
                        .spacing([0.0, 0.0])
                        .min_col_width(0.0)
                        .striped(true)
                        .show(ui, |ui| {
                            // Header row.
                            let col_widths = column_widths(avail.x);
                            for (header, w) in ["Status", "Label", "Local Port", "Remote", "Via"]
                                .iter()
                                .zip(col_widths.iter())
                            {
                                ui.add_sized(
                                    [*w, 22.0],
                                    egui::Label::new(
                                        egui::RichText::new(*header).strong(),
                                    ),
                                );
                            }
                            ui.end_row();

                            // Data rows.
                            for (i, tunnel) in tunnels.iter().enumerate() {
                                let is_active = active_ids.contains(&tunnel.id);
                                let selected = state.selected == Some(i);
                                let status = if is_active {
                                    TunnelStatus::Active
                                } else {
                                    TunnelStatus::Inactive
                                };

                                // Status column.
                                let (bullet, color) = match status {
                                    TunnelStatus::Active => (
                                        "\u{25cf} Active",
                                        egui::Color32::from_rgb(60, 180, 60),
                                    ),
                                    TunnelStatus::Inactive => (
                                        "\u{25cb} Inactive",
                                        egui::Color32::from_rgb(140, 140, 140),
                                    ),
                                };

                                let row_clicked = |ui: &mut egui::Ui, text: egui::RichText, w: f32| -> bool {
                                    let resp = ui.add_sized(
                                        [w, 22.0],
                                        egui::SelectableLabel::new(selected, text),
                                    );
                                    resp.clicked()
                                };

                                let mut clicked = false;
                                clicked |= row_clicked(
                                    ui,
                                    egui::RichText::new(bullet).color(color),
                                    col_widths[0],
                                );
                                clicked |= row_clicked(
                                    ui,
                                    egui::RichText::new(&tunnel.label),
                                    col_widths[1],
                                );
                                clicked |= row_clicked(
                                    ui,
                                    egui::RichText::new(tunnel.local_port.to_string()),
                                    col_widths[2],
                                );
                                clicked |= row_clicked(
                                    ui,
                                    egui::RichText::new(format!(
                                        "{}:{}",
                                        tunnel.remote_host, tunnel.remote_port
                                    )),
                                    col_widths[3],
                                );
                                clicked |= row_clicked(
                                    ui,
                                    egui::RichText::new(&tunnel.session_key),
                                    col_widths[4],
                                );

                                if clicked {
                                    state.selected = Some(i);
                                }
                                ui.end_row();
                            }
                        });
                });

            // Bottom button row.
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if dialog_button(ui, "Close").clicked() {
                        action = TunnelManagerAction::Close;
                    }
                    if dialog_button(ui, "Stop").clicked() {
                        if let Some(idx) = state.selected {
                            if let Some(t) = tunnels.get(idx) {
                                if active_ids.contains(&t.id) {
                                    action = TunnelManagerAction::Stop(t.id);
                                }
                            }
                        }
                    }
                    if dialog_button(ui, "Delete").clicked() {
                        if let Some(idx) = state.selected {
                            if let Some(t) = tunnels.get(idx) {
                                action = TunnelManagerAction::Delete(t.id);
                            }
                        }
                    }
                    if dialog_button(ui, "Activate").clicked() {
                        if let Some(idx) = state.selected {
                            if let Some(t) = tunnels.get(idx) {
                                if !active_ids.contains(&t.id) {
                                    action = TunnelManagerAction::Activate(t.id);
                                }
                            }
                        }
                    }
                    if dialog_button(ui, "New Tunnel\u{2026}").clicked() {
                        state.new_tunnel_form = Some(NewTunnelForm::new(servers));
                    }
                });
            });
        });

    action
}

/// Compute column widths for the 5-column table given the available width.
fn column_widths(total: f32) -> [f32; 5] {
    let status_w = 90.0;
    let local_port_w = 80.0;
    let remaining = (total - status_w - local_port_w - 20.0).max(120.0);
    let label_w = remaining * 0.25;
    let remote_w = remaining * 0.35;
    let via_w = remaining * 0.40;
    [status_w, label_w, local_port_w, remote_w, via_w]
}

// ---------------------------------------------------------------------------
// New Tunnel sub-dialog
// ---------------------------------------------------------------------------

pub struct NewTunnelForm {
    /// Available servers for the dropdown.
    servers: Vec<ServerEntry>,
    /// Index of the currently selected server.
    selected_server: usize,
    pub local_port: String,
    pub remote_host: String,
    pub remote_port: String,
    pub label: String,
    pub status_msg: String,
}

impl NewTunnelForm {
    pub fn new(servers: &[ServerEntry]) -> Self {
        Self {
            servers: servers.to_vec(),
            selected_server: 0,
            local_port: String::new(),
            remote_host: "localhost".into(),
            remote_port: String::new(),
            label: String::new(),
            status_msg: if servers.is_empty() {
                "No SSH servers configured. Add one in the sidebar first.".into()
            } else {
                String::new()
            },
        }
    }

    fn selected_server(&self) -> Option<&ServerEntry> {
        self.servers.get(self.selected_server)
    }

    fn server_display(entry: &ServerEntry) -> String {
        format!("{} \u{2014} {}@{}", entry.name, entry.user, entry.host)
    }

    /// Validate and build a `SavedTunnel`. Returns `None` and sets `status_msg` on error.
    pub fn to_saved_tunnel(&mut self) -> Option<SavedTunnel> {
        let server = match self.selected_server() {
            Some(s) => s.clone(),
            None => {
                self.status_msg = "Select an SSH server.".into();
                return None;
            }
        };

        let local_port = match parse_port(&self.local_port) {
            Some(p) => p,
            None => {
                self.status_msg = "Local Port must be a number between 1 and 65535.".into();
                return None;
            }
        };

        let remote_host = self.remote_host.trim().to_string();
        if remote_host.is_empty() {
            self.status_msg = "Remote host is required.".into();
            return None;
        }

        let remote_port = match parse_port(&self.remote_port) {
            Some(p) => p,
            None => {
                self.status_msg = "Remote Port must be a number between 1 and 65535.".into();
                return None;
            }
        };

        let label = if self.label.trim().is_empty() {
            format!(":{} \u{2192} {}:{}", local_port, remote_host, remote_port)
        } else {
            self.label.trim().to_string()
        };

        Some(SavedTunnel {
            id: Uuid::new_v4(),
            label,
            session_key: server.session_key(),
            local_port,
            remote_host,
            remote_port,
            auto_start: false,
        })
    }
}

enum NewTunnelAction {
    None,
    Connect,
    Cancel,
}

fn show_new_tunnel_dialog(ctx: &egui::Context, form: &mut NewTunnelForm) -> NewTunnelAction {
    let mut action = NewTunnelAction::None;

    egui::Window::new("New SSH Tunnel")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([460.0, 340.0])
        .show(ctx, |ui| {
            ui.add_space(8.0);

            egui::Grid::new("new_tunnel_form")
                .num_columns(2)
                .spacing([12.0, 10.0])
                .show(ui, |ui| {
                    // SSH Server dropdown — match text edit height.
                    ui.label("SSH Server:");
                    let selected_text = form
                        .selected_server()
                        .map(|s| NewTunnelForm::server_display(s))
                        .unwrap_or_else(|| "(none)".into());
                    let te_h = crate::ui::widgets::text_edit_height(ui);
                    let prev_interact_y = ui.spacing().interact_size.y;
                    ui.spacing_mut().interact_size.y = te_h;
                    egui::ComboBox::from_id_salt("tunnel_server_combo")
                        .selected_text(selected_text)
                        .width(280.0)
                        .show_ui(ui, |ui| {
                            for (i, server) in form.servers.iter().enumerate() {
                                let display = NewTunnelForm::server_display(server);
                                ui.selectable_value(&mut form.selected_server, i, display);
                            }
                        });
                    ui.spacing_mut().interact_size.y = prev_interact_y;
                    ui.end_row();

                    // Local Port.
                    ui.label("Local Port:");
                    ui.add(
                        crate::ui::widgets::text_edit(&mut form.local_port)
                            .desired_width(80.0),
                    );
                    ui.end_row();

                    // Remote Host.
                    ui.label("Remote Host:");
                    ui.add(
                        crate::ui::widgets::text_edit(&mut form.remote_host)
                            .desired_width(280.0),
                    );
                    ui.end_row();

                    // Remote Port.
                    ui.label("Remote Port:");
                    ui.add(
                        crate::ui::widgets::text_edit(&mut form.remote_port)
                            .desired_width(80.0),
                    );
                    ui.end_row();

                    // Label (optional).
                    ui.label("Label (opt.):");
                    ui.add(
                        crate::ui::widgets::text_edit(&mut form.label)
                            .desired_width(280.0),
                    );
                    ui.end_row();
                });

            // Status / error message.
            if !form.status_msg.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&form.status_msg)
                        .color(egui::Color32::from_rgb(255, 80, 80))
                        .italics()
                        .size(11.0),
                );
            }

            // Bottom buttons.
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let connect_enabled = !form.servers.is_empty();
                    if dialog_button_enabled(ui, "Save & Connect", connect_enabled).clicked() {
                        action = NewTunnelAction::Connect;
                    }
                    if dialog_button(ui, "Cancel").clicked() {
                        action = NewTunnelAction::Cancel;
                    }
                });
            });
        });

    action
}

fn parse_port(s: &str) -> Option<u16> {
    let p: u16 = s.trim().parse().ok()?;
    if p >= 1 { Some(p) } else { None }
}
