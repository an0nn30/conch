//! SSH connection initiation, tunnel activation, and connecting screen UI.

use std::time::Instant;

use conch_core::models::SavedTunnel;
use conch_session::SshSession;
use uuid::Uuid;

use crate::app::{ConchApp, PendingSsh, PendingSshInfo, SshConnectOutcome, DEFAULT_COLS, DEFAULT_ROWS};
use crate::sessions::build_term_config;

impl ConchApp {
    /// Spawn an async SSH connection attempt on the tokio runtime.
    pub(crate) fn start_ssh_connect(
        &mut self,
        host: String,
        port: u16,
        user: String,
        identity_file: Option<String>,
        proxy_command: Option<String>,
        proxy_jump: Option<String>,
        password: Option<String>,
    ) {
        let id = Uuid::new_v4();
        let (tx, rx) = std::sync::mpsc::channel();

        let label = host.clone();
        let detail = format!("{user}@{host}:{port}");
        self.pending_ssh_info.insert(id, PendingSshInfo {
            label,
            detail,
            started: Instant::now(),
            error: None,
            needs_password: false,
            password_buf: String::new(),
            password_focus: false,
            pending_auth: None,
        });

        self.state.tab_order.push(id);
        self.state.active_tab = Some(id);

        let host_clone = host.clone();
        let term_config = build_term_config(&self.state.user_config.terminal.cursor);
        self.rt.spawn(async move {
            let params = conch_session::ConnectParams {
                host: host_clone,
                port,
                user,
                identity_file: identity_file.map(std::path::PathBuf::from),
                password,
                proxy_command,
                proxy_jump,
            };
            let outcome = match SshSession::connect(&params, DEFAULT_COLS, DEFAULT_ROWS, term_config).await {
                Ok(conch_session::SshConnectResult::Connected(session)) => {
                    SshConnectOutcome::Connected(session)
                }
                Ok(result @ conch_session::SshConnectResult::NeedsPassword { .. }) => {
                    SshConnectOutcome::NeedsPassword(result)
                }
                Err(e) => {
                    let ssh_auth_sock = std::env::var("SSH_AUTH_SOCK")
                        .unwrap_or_else(|_| "(not set)".into());
                    let home = dirs::home_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(not set)".into());
                    let key_status = [
                        "id_ed25519", "id_ecdsa", "id_rsa",
                    ]
                    .iter()
                    .map(|name| {
                        let path = dirs::home_dir()
                            .unwrap_or_default()
                            .join(format!(".ssh/{name}"));
                        let exists = if path.exists() { "found" } else { "missing" };
                        format!("  ~/.ssh/{name}: {exists}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                    SshConnectOutcome::Failed(format!(
                        "{host}: {e:#}\n\n\
                         --- Diagnostics ---\n\
                         SSH_AUTH_SOCK: {ssh_auth_sock}\n\
                         HOME: {home}\n\
                         Keys:\n{key_status}"
                    ))
                }
            };
            let _ = tx.send(outcome);
        });

        self.pending_ssh_connections.push(PendingSsh { id, rx });
    }

    /// Collect all SSH server entries from sidebar folders + ssh_config hosts.
    pub(crate) fn collect_all_servers(&self) -> Vec<conch_core::models::ServerEntry> {
        let mut servers = Vec::new();
        fn collect_from_folders(
            folders: &[conch_core::models::ServerFolder],
            out: &mut Vec<conch_core::models::ServerEntry>,
        ) {
            for folder in folders {
                out.extend(folder.servers.iter().cloned());
                collect_from_folders(&folder.subfolders, out);
            }
        }
        collect_from_folders(&self.state.sessions_config.folders, &mut servers);
        for host in &self.state.ssh_config_hosts {
            if !servers.iter().any(|s| s.session_key() == host.session_key()) {
                servers.push(host.clone());
            }
        }
        servers
    }

    /// Kick off async tunnel activation (SSH connect + port forward).
    pub(crate) fn activate_tunnel(&mut self, tunnel: &SavedTunnel) {
        let tunnel = tunnel.clone();
        let servers = self.collect_all_servers();
        log::info!(
            "activate_tunnel: looking for session_key='{}' among {} servers",
            tunnel.session_key,
            servers.len(),
        );
        for s in &servers {
            log::debug!("  available server: '{}' key='{}'", s.name, s.session_key());
        }
        let server = servers.into_iter().find(|s| s.session_key() == tunnel.session_key);
        let Some(server) = server else {
            log::error!(
                "No matching server for tunnel session_key '{}'. \
                 Check that the server is configured in the sidebar or ssh_config.",
                tunnel.session_key,
            );
            return;
        };
        log::info!(
            "activate_tunnel: matched server '{}' ({}@{}:{}), connecting for tunnel {} (:{} -> {}:{})",
            server.name, server.user, server.host, server.port,
            tunnel.id, tunnel.local_port, tunnel.remote_host, tunnel.remote_port,
        );
        let tm = self.tunnel_manager.clone_inner();
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending_tunnel_results.push((tunnel.id, rx));

        self.rt.spawn(async move {
            let params = conch_session::ConnectParams::from(&server);
            log::info!(
                "activate_tunnel[{}]: SSH connecting to {}@{}:{} ...",
                tunnel.id, params.user, params.host, params.port,
            );
            let result = async {
                let handle = conch_session::connect_tunnel(&params).await
                    .map_err(|e| format!("SSH connect failed for {}@{}:{}: {e}", params.user, params.host, params.port))?;
                log::info!(
                    "activate_tunnel[{}]: SSH connected, starting local forward 127.0.0.1:{} -> {}:{} ...",
                    tunnel.id, tunnel.local_port, tunnel.remote_host, tunnel.remote_port,
                );
                tm.start_local_forward(
                    tunnel.id,
                    handle,
                    tunnel.local_port,
                    tunnel.remote_host.clone(),
                    tunnel.remote_port,
                ).await.map_err(|e| format!("Port forward failed: {e}"))
            }.await;
            match &result {
                Ok(()) => log::info!("activate_tunnel[{}]: tunnel active and listening", tunnel.id),
                Err(e) => log::error!("activate_tunnel[{}]: failed: {e}", tunnel.id),
            }
            let _ = tx.send(result);
        });
    }
}

/// Action from the connecting/error/password screen.
pub(crate) enum ConnectingScreenAction {
    None,
    Close,
    SubmitPassword(String),
}

/// Render the "Connecting to..." screen with a bouncing progress indicator,
/// password prompt, or error screen.
pub(crate) fn show_connecting_screen(ui: &mut egui::Ui, info: &mut PendingSshInfo) -> ConnectingScreenAction {
    let rect = ui.available_rect_before_wrap();

    let bg = if ui.visuals().dark_mode {
        egui::Color32::from_gray(30)
    } else {
        egui::Color32::from_gray(241)
    };
    ui.painter().rect_filled(rect, 0.0, bg);

    let center = rect.center();

    // --- Password prompt (server reachable, needs password) ---
    if info.needs_password {
        let content_width = (rect.width() * 0.7).min(500.0);
        let content_rect = egui::Rect::from_center_size(
            center,
            egui::Vec2::new(content_width, rect.height() * 0.5),
        );
        let mut action = ConnectingScreenAction::None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(
                    egui::RichText::new(format!("Password required for {}", info.label))
                        .size(22.0),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&info.detail)
                        .size(14.0)
                        .color(if ui.visuals().dark_mode {
                            egui::Color32::from_gray(160)
                        } else {
                            egui::Color32::from_gray(80)
                        }),
                );
                // Show error message (e.g. "Incorrect password.")
                if let Some(err) = &info.error {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(err)
                            .size(13.0)
                            .color(egui::Color32::from_rgb(220, 50, 50)),
                    );
                }
                ui.add_space(16.0);
                let pw_resp = ui.add(
                    crate::ui::widgets::text_edit(&mut info.password_buf)
                        .password(true)
                        .desired_width(300.0)
                        .hint_text("Password"),
                );
                if info.password_focus {
                    pw_resp.request_focus();
                    info.password_focus = false;
                }
                let enter_pressed = pw_resp.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can_submit = !info.password_buf.is_empty();
                    if ui.add_enabled(can_submit, egui::Button::new("Connect")).clicked()
                        || (enter_pressed && can_submit)
                    {
                        info.error = None;
                        action = ConnectingScreenAction::SubmitPassword(
                            info.password_buf.clone(),
                        );
                    }
                    if ui.button("Cancel").clicked() {
                        action = ConnectingScreenAction::Close;
                    }
                });
            });
        });
        return action;
    }

    // --- Error screen ---
    if let Some(error) = &info.error {
        let content_width = (rect.width() * 0.7).min(600.0);
        let content_rect = egui::Rect::from_center_size(
            center,
            egui::Vec2::new(content_width, rect.height() * 0.8),
        );
        let mut action = ConnectingScreenAction::None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new(format!("Connection to {} failed", info.label))
                            .size(24.0)
                            .color(egui::Color32::from_rgb(220, 50, 50)),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&info.detail)
                            .size(14.0)
                            .color(if ui.visuals().dark_mode {
                                egui::Color32::from_gray(160)
                            } else {
                                egui::Color32::from_gray(80)
                            }),
                    );
                    ui.add_space(16.0);
                });

                let error_color = if ui.visuals().dark_mode {
                    egui::Color32::from_gray(180)
                } else {
                    egui::Color32::from_gray(60)
                };
                ui.label(
                    egui::RichText::new(error)
                        .size(13.0)
                        .family(egui::FontFamily::Monospace)
                        .color(error_color),
                );

                ui.add_space(16.0);
                ui.vertical_centered(|ui| {
                    if ui.button("Close Tab").clicked() {
                        action = ConnectingScreenAction::Close;
                    }
                });
                ui.add_space(12.0);
            });
        });
        return action;
    }

    let heading = format!("Connecting to {}\u{2026}", info.label);
    let heading_galley = ui.painter().layout_no_wrap(
        heading,
        egui::FontId::new(28.0, egui::FontFamily::Proportional),
        if ui.visuals().dark_mode { egui::Color32::WHITE } else { egui::Color32::BLACK },
    );
    let heading_pos = egui::Pos2::new(
        center.x - heading_galley.size().x / 2.0,
        center.y - 40.0,
    );
    ui.painter().galley(heading_pos, heading_galley, egui::Color32::PLACEHOLDER);

    let detail_galley = ui.painter().layout_no_wrap(
        info.detail.clone(),
        egui::FontId::new(16.0, egui::FontFamily::Proportional),
        if ui.visuals().dark_mode { egui::Color32::from_gray(200) } else { egui::Color32::from_gray(40) },
    );
    let detail_pos = egui::Pos2::new(
        center.x - detail_galley.size().x / 2.0,
        center.y + 5.0,
    );
    ui.painter().galley(detail_pos, detail_galley, egui::Color32::PLACEHOLDER);

    // Bouncing progress bar.
    let bar_w = 400.0_f32.min(rect.width() * 0.6);
    let bar_h = 6.0;
    let bar_y = center.y + 50.0;
    let bar_rect = egui::Rect::from_min_size(
        egui::Pos2::new(center.x - bar_w / 2.0, bar_y),
        egui::Vec2::new(bar_w, bar_h),
    );

    let track_color = if ui.visuals().dark_mode {
        egui::Color32::from_gray(60)
    } else {
        egui::Color32::from_gray(210)
    };
    ui.painter().rect_filled(bar_rect, bar_h / 2.0, track_color);

    let elapsed = info.started.elapsed().as_secs_f32();
    let cycle = 1.8;
    let t = (elapsed % cycle) / cycle;
    let pos_t = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
    let eased = pos_t * pos_t * (3.0 - 2.0 * pos_t);
    let indicator_w = bar_w * 0.15;
    let indicator_x = bar_rect.min.x + eased * (bar_w - indicator_w);
    let indicator_rect = egui::Rect::from_min_size(
        egui::Pos2::new(indicator_x, bar_y),
        egui::Vec2::new(indicator_w, bar_h),
    );
    let accent = egui::Color32::from_rgb(66, 133, 244);
    ui.painter().rect_filled(indicator_rect, bar_h / 2.0, accent);

    ConnectingScreenAction::None
}
