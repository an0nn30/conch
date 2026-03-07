//! Main application logic for the Conch terminal emulator.
//!
//! Implements `eframe::App` and orchestrates terminal sessions, input handling,
//! SSH connections, and UI panel layout.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use alacritty_terminal::event::Event as TermEvent;
use conch_core::{config, ssh_config};
use conch_core::models::SavedTunnel;
use conch_session::{LocalSession, SftpCmd, SftpEvent, SshSession, TunnelManager, run_sftp_worker};
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::extra_window::ExtraWindow;
use crate::icons::{Icon, IconCache};
use crate::input::{self, ResolvedShortcuts};
use crate::mouse::{Selection, handle_terminal_mouse};
use crate::state::{AppState, Session, SessionBackend};
use crate::terminal::widget::{get_selected_text, measure_cell_size, show_terminal};
use conch_plugin::{
    PluginCommand, PluginContext, PluginMeta, PluginResponse, SessionInfoData,
    SessionTarget, discover_plugins, run_plugin,
};
use crate::ui::dialogs::new_connection::{self, DialogAction, NewConnectionForm};
use crate::ui::dialogs::plugin_dialog::{self, ActivePluginDialog, FormFieldState};
use crate::ui::dialogs::preferences::{self, PreferencesAction, PreferencesForm};
use crate::ui::dialogs::tunnels::{self, TunnelManagerAction, TunnelManagerState};
use crate::ui::file_browser::FileListEntry;
use crate::ui::session_panel::{self, ServerAddress, SessionPanelAction, SessionPanelState};
use crate::ui::sidebar::{self, SidebarAction};

/// Initial PTY dimensions before the first font-based resize.
pub(crate) const DEFAULT_COLS: u16 = 80;
pub(crate) const DEFAULT_ROWS: u16 = 24;

/// Build a `WidgetText` for a menu shortcut like "⌘T" with the ⌘ glyph
/// scaled down so it visually matches the key character height.
fn cmd_shortcut(key: &str) -> egui::WidgetText {
    use egui::text::LayoutJob;
    let key_size = 12.0;
    // The ⌘ glyph is inherently ~40% taller than digit glyphs at the same
    // point size, so we shrink it proportionally to get matching cap height.
    let cmd_size = key_size * 0.62;
    let mut job = LayoutJob::default();
    job.append(
        "⌘",
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(cmd_size),
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.append(
        key,
        1.0, // small gap between ⌘ and key
        egui::TextFormat {
            font_id: egui::FontId::proportional(key_size),
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.into()
}

/// Cursor blink interval in milliseconds.
pub(crate) const CURSOR_BLINK_MS: u128 = 500;

/// Receives the result of an async SSH connection attempt.
struct PendingSsh {
    id: Uuid,
    rx: std::sync::mpsc::Receiver<Result<SshSession, String>>,
}

/// Display info for a pending SSH connection (shown in the connecting tab).
struct PendingSshInfo {
    /// Short name for the tab title and heading, e.g. "dustin-vm"
    label: String,
    /// Detail line, e.g. "dustin@lab.nexxuscraft.com:22"
    detail: String,
    /// When the connection was initiated (for the bouncing progress bar).
    started: Instant,
}

/// A plugin currently executing on a tokio task.
struct RunningPlugin {
    meta: PluginMeta,
    commands_rx: tokio::sync::mpsc::UnboundedReceiver<(PluginCommand, tokio::sync::mpsc::UnboundedSender<PluginResponse>)>,
    /// Queued dialog requests waiting to be shown (FIFO).
    pending_dialogs: Vec<(PluginCommand, tokio::sync::mpsc::UnboundedSender<PluginResponse>)>,
}

/// The top-level eframe application.
pub struct ConchApp {
    state: AppState,
    rt: Arc<Runtime>,
    shortcuts: ResolvedShortcuts,

    // Terminal rendering
    cell_width: f32,
    cell_height: f32,
    cell_size_measured: bool,
    last_pixels_per_point: f32,

    // Cursor blink
    cursor_visible: bool,
    last_blink: Instant,

    // Resize tracking (only send resize when dimensions actually change)
    last_cols: u16,
    last_rows: u16,

    // Mouse selection
    selection: Selection,

    // Async SSH connection results
    pending_ssh_connections: Vec<PendingSsh>,
    pending_ssh_info: HashMap<Uuid, PendingSshInfo>,

    // SFTP worker state (auto-spawned on SSH connect, drives sidebar remote pane)
    sftp_cmd_tx: Option<tokio::sync::mpsc::UnboundedSender<SftpCmd>>,
    sftp_result_rx: Option<std::sync::mpsc::Receiver<SftpEvent>>,
    sftp_session_id: Option<Uuid>,
    remote_home: Option<PathBuf>,
    last_active_tab: Option<Uuid>,
    transfers: Vec<sidebar::TransferStatus>,

    // Icons
    icon_cache: Option<IconCache>,

    // Session panel UI state (inline rename, new-folder input)
    session_panel_state: SessionPanelState,

    // Transient UI state
    use_native_menu: bool,
    /// The right sidebar was opened temporarily for quick connect (Cmd+/).
    quick_connect_opened_sidebar: bool,
    /// The left sidebar was opened temporarily for plugin search (Cmd+Shift+P).
    plugin_search_opened_sidebar: bool,
    plugin_search_query: String,
    plugin_search_focus: bool,
    show_about: bool,
    quit_requested: bool,
    style_applied: bool,
    preferences_form: Option<PreferencesForm>,

    // Tunnel management
    tunnel_manager: TunnelManager,
    tunnel_dialog: Option<TunnelManagerState>,
    /// IDs of currently active tunnels (refreshed each frame from TunnelManager).
    tunnel_active_ids: Vec<Uuid>,
    /// Receives results from async tunnel activation attempts.
    pending_tunnel_results: Vec<(Uuid, std::sync::mpsc::Receiver<Result<(), String>>)>,

    // Tab rename dialog
    rename_tab_id: Option<Uuid>,
    rename_tab_buf: String,
    rename_tab_focus: bool,

    // Window focus tracking (for FOCUS_IN_OUT terminal mode)
    window_focused: bool,

    // Extra windows (multi-window via egui viewports)
    extra_windows: Vec<ExtraWindow>,
    next_viewport_num: u32,
    /// Index of the focused extra window, or `None` if the main window is focused.
    focused_extra_window: Option<usize>,

    // Plugin engine
    discovered_plugins: Vec<PluginMeta>,
    running_plugins: Vec<RunningPlugin>,
    plugin_output_lines: Vec<String>,
    active_plugin_dialog: Option<ActivePluginDialog>,
    plugin_progress: Option<String>,
    pending_clipboard: Option<String>,
    selected_plugin: Option<usize>,

    // IPC socket listener
    ipc_listener: Option<crate::ipc::IpcListener>,
}

impl ConchApp {
    pub fn new(rt: Arc<Runtime>) -> Self {
        // Migration already ran in main(); load_user_config is idempotent.
        let user_config = config::load_user_config().unwrap_or_else(|e| {
            log::error!("Failed to load config.toml, using defaults: {e:#}");
            config::UserConfig::default()
        });
        let persistent = config::load_persistent_state().unwrap_or_default();
        let sessions_config = config::load_sessions().unwrap_or_default();
        let shortcuts = ResolvedShortcuts::from_config(&user_config.conch.keyboard);

        let mut state = AppState::new(user_config, persistent, sessions_config);

        state.ssh_config_hosts = ssh_config::parse_ssh_config().unwrap_or_default();

        let initial_path = state.file_browser.local_path.clone();
        state.file_browser.local_entries = load_local_entries(&initial_path);

        let _ = open_local_terminal(&mut state, DEFAULT_COLS, DEFAULT_ROWS, 8.0, 16.0);

        // Discover plugins — check both native config dir and legacy ~/.config/conch/
        let discovered_plugins = scan_plugin_dirs();

        // Set up native macOS menu bar (if enabled in config).
        let use_native_menu = cfg!(target_os = "macos")
            && state.user_config.conch.ui.native_menu_bar;
        #[cfg(target_os = "macos")]
        if use_native_menu {
            let names: Vec<String> = discovered_plugins.iter().map(|p| p.name.clone()).collect();
            crate::macos_menu::setup_menu_bar(&names);
        }

        Self {
            state,
            rt,
            shortcuts,
            cell_width: 8.0,
            cell_height: 16.0,
            cell_size_measured: false,
            last_pixels_per_point: 0.0,
            cursor_visible: true,
            last_blink: Instant::now(),
            last_cols: DEFAULT_COLS,
            last_rows: DEFAULT_ROWS,
            selection: Selection::default(),
            pending_ssh_connections: Vec::new(),
            pending_ssh_info: HashMap::new(),
            sftp_cmd_tx: None,
            sftp_result_rx: None,
            sftp_session_id: None,
            remote_home: None,
            last_active_tab: None,
            transfers: Vec::new(),
            icon_cache: None,
            session_panel_state: SessionPanelState::default(),
            use_native_menu,
            quick_connect_opened_sidebar: false,
            plugin_search_opened_sidebar: false,
            plugin_search_query: String::new(),
            plugin_search_focus: false,
            show_about: false,
            quit_requested: false,
            style_applied: false,
            preferences_form: None,
            tunnel_manager: TunnelManager::new(),
            tunnel_dialog: None,
            tunnel_active_ids: Vec::new(),
            pending_tunnel_results: Vec::new(),
            rename_tab_id: None,
            rename_tab_buf: String::new(),
            rename_tab_focus: false,
            window_focused: true,
            extra_windows: Vec::new(),
            next_viewport_num: 1,
            focused_extra_window: None,
            discovered_plugins,
            running_plugins: Vec::new(),
            plugin_output_lines: Vec::new(),
            active_plugin_dialog: None,
            plugin_progress: None,
            pending_clipboard: None,
            selected_plugin: None,
            ipc_listener: crate::ipc::IpcListener::start(),
        }
    }

    /// Drain async event channels for all sessions and pending SSH connections.
    fn poll_events(&mut self, ctx: &egui::Context) {
        // Collect terminal events.
        let mut exited = Vec::new();
        for session in self.state.sessions.values_mut() {
            while let Ok(event) = session.event_rx.try_recv() {
                match event {
                    TermEvent::Wakeup => ctx.request_repaint(),
                    TermEvent::Title(title) => session.title = title,
                    TermEvent::Exit => exited.push(session.id),
                    _ => {}
                }
            }
        }
        for id in exited {
            self.remove_session(id);
        }

        // Quit when the last session exits (and no SSH connections are pending).
        if self.state.sessions.is_empty() && self.pending_ssh_connections.is_empty() {
            self.quit_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Poll pending SSH connections.
        let mut completed = Vec::new();
        for (i, pending) in self.pending_ssh_connections.iter().enumerate() {
            match pending.rx.try_recv() {
                Ok(Ok(ssh)) => completed.push((i, pending.id, Some(ssh))),
                Ok(Err(err)) => {
                    log::error!("SSH connection failed: {err}");
                    completed.push((i, pending.id, None));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    log::error!("SSH connection channel dropped");
                    completed.push((i, pending.id, None));
                }
            }
        }
        // Remove in reverse order to keep indices valid.
        for (i, id, ssh_opt) in completed.into_iter().rev() {
            self.pending_ssh_connections.remove(i);
            self.pending_ssh_info.remove(&id);
            if let Some(mut ssh_session) = ssh_opt {
                let event_rx = ssh_session.take_event_rx();

                // Spawn SFTP worker for the new SSH session.
                let handle = Arc::clone(ssh_session.ssh_handle());
                let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
                let (result_tx, result_rx) = std::sync::mpsc::channel();
                self.rt.spawn(run_sftp_worker(handle, cmd_rx, result_tx));

                // Request initial listing of the remote home directory.
                let _ = cmd_tx.send(SftpCmd::List(PathBuf::from(".")));

                // Tear down any previous SFTP worker.
                if let Some(old_tx) = self.sftp_cmd_tx.take() {
                    let _ = old_tx.send(SftpCmd::Shutdown);
                }
                self.sftp_cmd_tx = Some(cmd_tx);
                self.sftp_result_rx = Some(result_rx);
                self.sftp_session_id = Some(id);
                self.remote_home = None;

                let session = Session {
                    id,
                    title: "SSH".into(),
                    custom_title: None,
                    backend: SessionBackend::Ssh(ssh_session),
                    event_rx,
                };
                self.state.sessions.insert(id, session);
                // Tab already exists in tab_order from start_ssh_connect.
            } else {
                // Connection failed — remove the tab.
                self.state.tab_order.retain(|&tab_id| tab_id != id);
                if self.state.active_tab == Some(id) {
                    self.state.active_tab = self.state.tab_order.last().copied();
                }
            }
        }

        // Manage SFTP worker on tab switch.
        if self.state.active_tab != self.last_active_tab {
            self.last_active_tab = self.state.active_tab;
            if let Some(id) = self.state.active_tab {
                if let Some(session) = self.state.sessions.get(&id) {
                    match &session.backend {
                        SessionBackend::Ssh(ssh) => {
                            if self.sftp_session_id != Some(id) {
                                // Different SSH tab — tear down old worker, spawn new.
                                if let Some(old_tx) = self.sftp_cmd_tx.take() {
                                    let _ = old_tx.send(SftpCmd::Shutdown);
                                }
                                let handle = Arc::clone(ssh.ssh_handle());
                                let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
                                let (result_tx, result_rx) = std::sync::mpsc::channel();
                                self.rt.spawn(run_sftp_worker(handle, cmd_rx, result_tx));
                                let _ = cmd_tx.send(SftpCmd::List(PathBuf::from(".")));
                                self.sftp_cmd_tx = Some(cmd_tx);
                                self.sftp_result_rx = Some(result_rx);
                                self.sftp_session_id = Some(id);
                                self.remote_home = None;
                                self.state.file_browser.remote_entries.clear();
                                self.state.file_browser.remote_path = None;
                                self.transfers.clear();
                            } else if self.state.file_browser.remote_path.is_none() {
                                // Same SSH tab but remote state was cleared — re-request listing.
                                if let Some(tx) = &self.sftp_cmd_tx {
                                    let _ = tx.send(SftpCmd::List(
                                        self.remote_home.clone().unwrap_or_else(|| PathBuf::from(".")),
                                    ));
                                }
                            }
                        }
                        SessionBackend::Local(_) => {
                            // Local tab — hide remote pane but keep SFTP worker alive.
                            self.state.file_browser.remote_path = None;
                            self.state.file_browser.remote_entries.clear();
                        }
                    }
                }
            }
        }

        // Poll SFTP results into the sidebar file browser.
        if let Some(rx) = &self.sftp_result_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    SftpEvent::Listing(listing) => {
                        self.state.file_browser.remote_path_edit =
                            listing.path.to_string_lossy().into_owned();
                        self.state.file_browser.remote_path = Some(listing.path);
                        self.state.file_browser.remote_entries =
                            listing.entries.into_iter().map(Into::into).collect();
                        self.state.file_browser.remote_selected = None;
                        if self.remote_home.is_none() {
                            self.remote_home = Some(listing.home);
                        }
                    }
                    SftpEvent::TransferProgress {
                        filename,
                        bytes_transferred,
                        total_bytes,
                    } => {
                        if let Some(ts) = self
                            .transfers
                            .iter_mut()
                            .find(|t| t.filename == filename && !t.done)
                        {
                            ts.bytes_transferred = bytes_transferred;
                            ts.total_bytes = total_bytes;
                        }
                    }
                    SftpEvent::TransferComplete {
                        filename,
                        success,
                        error,
                    } => {
                        // Update matching in-progress transfer, or add a new entry.
                        if let Some(ts) = self
                            .transfers
                            .iter_mut()
                            .find(|t| t.filename == filename && !t.done)
                        {
                            ts.done = true;
                            ts.error = if success { None } else { error };
                            if success {
                                ts.bytes_transferred = ts.total_bytes;
                            }
                        } else {
                            self.transfers.push(sidebar::TransferStatus {
                                filename,
                                upload: true,
                                done: true,
                                error: if success { None } else { error },
                                bytes_transferred: 0,
                                total_bytes: 0,
                                cancel: Arc::new(AtomicBool::new(false)),
                            });
                        }
                        // Refresh both panes after a transfer completes.
                        if let Some(tx) = &self.sftp_cmd_tx {
                            if let Some(rp) = &self.state.file_browser.remote_path {
                                let _ = tx.send(SftpCmd::List(rp.clone()));
                            }
                        }
                        let local = self.state.file_browser.local_path.clone();
                        self.state.file_browser.local_entries = load_local_entries(&local);
                    }
                }
            }
        }

        // Poll running plugins.
        self.poll_plugin_events(ctx);
    }

    /// Drain commands from all running plugins and handle them.
    fn poll_plugin_events(&mut self, ctx: &egui::Context) {
        // Collect commands from running plugins into a separate vec to avoid
        // borrowing self.running_plugins while calling self.handle_plugin_command.
        let mut immediate_cmds = Vec::new();
        self.running_plugins.retain_mut(|rp| {
            loop {
                match rp.commands_rx.try_recv() {
                    Ok((cmd, resp_tx)) => {
                        if is_dialog_command(&cmd) {
                            rp.pending_dialogs.push((cmd, resp_tx));
                        } else {
                            immediate_cmds.push((cmd, resp_tx));
                        }
                        ctx.request_repaint();
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return true,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return false,
                }
            }
        });

        // Process non-dialog commands.
        for (cmd, resp_tx) in immediate_cmds {
            self.handle_plugin_command(cmd, resp_tx);
        }

        // If no dialog is currently active, promote the next pending one.
        if self.active_plugin_dialog.is_none() {
            for rp in &mut self.running_plugins {
                if !rp.pending_dialogs.is_empty() {
                    let (cmd, resp_tx) = rp.pending_dialogs.remove(0);
                    self.promote_dialog_command(cmd, resp_tx);
                    break;
                }
            }
        }
    }

    /// Handle a non-dialog plugin command immediately.
    fn handle_plugin_command(
        &mut self,
        cmd: PluginCommand,
        resp_tx: tokio::sync::mpsc::UnboundedSender<PluginResponse>,
    ) {
        match cmd {
            PluginCommand::Send { target, text } => {
                if let Some(session) = self.resolve_session(&target) {
                    session.backend.write(text.as_bytes());
                }
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::Exec { target, command } => {
                // For now, exec on local sessions runs via /bin/sh -c.
                // SSH exec would need a one-shot channel on the ssh_handle.
                // Simplified: send command + newline and return empty output.
                if let Some(session) = self.resolve_session(&target) {
                    session.backend.write(format!("{}\n", command).as_bytes());
                }
                let _ = resp_tx.send(PluginResponse::Output(String::new()));
            }
            PluginCommand::OpenSession { name } => {
                // Try to find a matching server and connect.
                let servers = self.collect_all_servers();
                if let Some(server) = servers.iter().find(|s| s.name == name || s.host == name) {
                    self.start_ssh_connect(
                        server.host.clone(),
                        server.port,
                        server.user.clone(),
                        server.identity_file.clone(),
                        server.proxy_command.clone(),
                        server.proxy_jump.clone(),
                        None,
                    );
                }
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::Clipboard(text) => {
                self.pending_clipboard = Some(text);
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::Notify(msg) => {
                log::info!("[plugin] {msg}");
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::Log(msg) => {
                log::info!("[plugin] {msg}");
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::UiAppend(text) => {
                self.plugin_output_lines.push(text);
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::UiClear => {
                self.plugin_output_lines.clear();
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::GetCurrentSession => {
                let info = self.state.active_tab.and_then(|id| {
                    self.state.sessions.get(&id).map(|s| SessionInfoData {
                        id: id.to_string(),
                        title: s.custom_title.as_ref().unwrap_or(&s.title).clone(),
                        session_type: match &s.backend {
                            SessionBackend::Local(_) => "local".into(),
                            SessionBackend::Ssh(_) => "ssh".into(),
                        },
                    })
                });
                let _ = resp_tx.send(PluginResponse::SessionInfo(info));
            }
            PluginCommand::GetAllSessions => {
                let list: Vec<SessionInfoData> = self
                    .state
                    .sessions
                    .iter()
                    .map(|(id, s)| SessionInfoData {
                        id: id.to_string(),
                        title: s.custom_title.as_ref().unwrap_or(&s.title).clone(),
                        session_type: match &s.backend {
                            SessionBackend::Local(_) => "local".into(),
                            SessionBackend::Ssh(_) => "ssh".into(),
                        },
                    })
                    .collect();
                let _ = resp_tx.send(PluginResponse::SessionList(list));
            }
            PluginCommand::GetNamedSession { name } => {
                let info = self.state.sessions.iter().find_map(|(id, s)| {
                    let title = s.custom_title.as_ref().unwrap_or(&s.title);
                    if title == &name {
                        Some(SessionInfoData {
                            id: id.to_string(),
                            title: title.clone(),
                            session_type: match &s.backend {
                                SessionBackend::Local(_) => "local".into(),
                                SessionBackend::Ssh(_) => "ssh".into(),
                            },
                        })
                    } else {
                        None
                    }
                });
                let _ = resp_tx.send(PluginResponse::SessionInfo(info));
            }
            PluginCommand::GetServers => {
                let names: Vec<String> = self
                    .collect_all_servers()
                    .iter()
                    .map(|s| s.name.clone())
                    .collect();
                let _ = resp_tx.send(PluginResponse::ServerList(names));
            }
            PluginCommand::ShowProgress { message } => {
                self.plugin_progress = Some(message);
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::HideProgress => {
                self.plugin_progress = None;
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            // Dialog commands shouldn't arrive here, but handle gracefully.
            _ => {
                let _ = resp_tx.send(PluginResponse::Ok);
            }
        }
    }

    /// Promote a dialog command into the active dialog slot.
    fn promote_dialog_command(
        &mut self,
        cmd: PluginCommand,
        resp_tx: tokio::sync::mpsc::UnboundedSender<PluginResponse>,
    ) {
        let dialog = match cmd {
            PluginCommand::ShowForm { title, fields } => {
                let field_states: Vec<FormFieldState> =
                    fields.iter().map(FormFieldState::from_field).collect();
                ActivePluginDialog::Form {
                    title,
                    fields: field_states,
                    resp_tx,
                    focus_first: true,
                }
            }
            PluginCommand::ShowPrompt { message } => ActivePluginDialog::Prompt {
                message,
                input: String::new(),
                resp_tx,
                focus_first: true,
            },
            PluginCommand::ShowConfirm { message } => ActivePluginDialog::Confirm {
                message,
                resp_tx,
            },
            PluginCommand::ShowAlert { title, message } => ActivePluginDialog::Alert {
                title,
                message,
                resp_tx,
            },
            PluginCommand::ShowError { title, message } => ActivePluginDialog::Error {
                title,
                message,
                resp_tx,
            },
            PluginCommand::ShowText { title, text } => ActivePluginDialog::Text {
                title,
                text,
                copied_at: None,
                resp_tx,
            },
            PluginCommand::ShowTable {
                title,
                columns,
                rows,
            } => ActivePluginDialog::Table {
                title,
                columns,
                rows,
                resp_tx,
            },
            _ => return,
        };
        self.active_plugin_dialog = Some(dialog);
    }

    /// Resolve a session target to a `&Session`.
    fn resolve_session(&self, target: &SessionTarget) -> Option<&Session> {
        match target {
            SessionTarget::Current => {
                self.state.active_tab.and_then(|id| self.state.sessions.get(&id))
            }
            SessionTarget::Named(name) => {
                self.state.sessions.values().find(|s| {
                    let title = s.custom_title.as_ref().unwrap_or(&s.title);
                    title == name
                })
            }
        }
    }

    /// Launch a discovered plugin by its index in `discovered_plugins`.
    fn run_plugin_by_index(&mut self, idx: usize) {
        let Some(meta) = self.discovered_plugins.get(idx).cloned() else {
            return;
        };
        let (ctx, commands_rx) = PluginContext::new();
        let path = meta.path.clone();
        self.rt.spawn(async move {
            if let Err(e) = run_plugin(&path, ctx).await {
                log::error!("Plugin '{}' failed: {e}", path.display());
            }
        });
        self.running_plugins.push(RunningPlugin {
            meta,
            commands_rx,
            pending_dialogs: Vec::new(),
        });
    }

    /// Stop a running plugin by index (drops channel, causing the Lua task to error out).
    fn stop_plugin(&mut self, idx: usize) {
        if idx < self.running_plugins.len() {
            self.running_plugins.remove(idx);
        }
    }

    /// Re-scan the plugins directory.
    fn refresh_plugins(&mut self) {
        self.discovered_plugins = scan_plugin_dirs();
    }

    /// Returns true when any modal dialog is open and should steal focus from the terminal.
    fn any_dialog_open(&self) -> bool {
        self.state.new_connection_form.is_some()
            || self.show_about
            || self.rename_tab_id.is_some()
            || self.preferences_form.is_some()
            || self.tunnel_dialog.is_some()
            || self.active_plugin_dialog.is_some()
            || self.plugin_progress.is_some()
    }

    /// Close the topmost dialog. Returns true if a dialog was closed.
    fn close_topmost_dialog(&mut self) -> bool {
        // Close in reverse visual stacking order (plugin dialogs on top, then others).
        if let Some(dialog) = self.active_plugin_dialog.take() {
            // Send a cancel/close response so the plugin coroutine doesn't hang.
            send_plugin_dialog_cancel(&dialog);
            return true;
        }
        if self.plugin_progress.is_some() {
            self.plugin_progress = None;
            return true;
        }
        if self.show_about {
            self.show_about = false;
            return true;
        }
        if self.rename_tab_id.is_some() {
            self.rename_tab_id = None;
            self.rename_tab_buf.clear();
            return true;
        }
        if self.preferences_form.is_some() {
            self.preferences_form = None;
            return true;
        }
        if self.tunnel_dialog.is_some() {
            self.tunnel_dialog = None;
            return true;
        }
        if self.state.new_connection_form.is_some() {
            self.state.new_connection_form = None;
            return true;
        }
        false
    }

    /// Process keyboard events: app shortcuts always run, PTY forwarding
    /// only when `forward_to_pty` is true (i.e. no text widget has focus).
    fn handle_keyboard(&mut self, ctx: &egui::Context, forward_to_pty: bool) {
        use alacritty_terminal::term::TermMode;

        // Read terminal mode before entering the input closure so we know
        // whether the shell has enabled application cursor mode (DECCKM).
        // Use try_lock to avoid blocking the main thread on FairMutex contention.
        let app_cursor = forward_to_pty
            && self.state.active_session().map_or(false, |s| {
                s.backend
                    .term()
                    .try_lock_unfair()
                    .map_or(false, |term| term.mode().contains(TermMode::APP_CURSOR))
            });

        ctx.input(|input| {
            for event in &input.events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        // ESC closes the topmost dialog (when no text field has focus).
                        if *key == egui::Key::Escape && !modifiers.command && !modifiers.alt && !modifiers.shift {
                            if self.close_topmost_dialog() {
                                return;
                            }
                        }

                        // Command+number → switch to tab N (checked first).
                        // Cmd on macOS, Ctrl on Linux/Windows.
                        if modifiers.command && !modifiers.alt && !modifiers.shift {
                            let tab_num = match key {
                                egui::Key::Num1 => Some(0usize),
                                egui::Key::Num2 => Some(1),
                                egui::Key::Num3 => Some(2),
                                egui::Key::Num4 => Some(3),
                                egui::Key::Num5 => Some(4),
                                egui::Key::Num6 => Some(5),
                                egui::Key::Num7 => Some(6),
                                egui::Key::Num8 => Some(7),
                                egui::Key::Num9 => Some(8),
                                _ => None,
                            };
                            if let Some(idx) = tab_num {
                                if let Some(&id) = self.state.tab_order.get(idx) {
                                    self.state.active_tab = Some(id);
                                    return;
                                }
                            }
                        }

                        // App-level configurable shortcuts.
                        if let Some(ref kb) = self.shortcuts.new_window {
                            if kb.matches(key, modifiers) {
                                self.spawn_extra_window();
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.new_tab {
                            if kb.matches(key, modifiers) { self.open_local_tab(); return; }
                        }
                        if let Some(ref kb) = self.shortcuts.close_tab {
                            if kb.matches(key, modifiers) {
                                if let Some(id) = self.state.active_tab {
                                    log::debug!("close_tab: removing session {id}");
                                    self.remove_session(id);
                                    log::debug!("close_tab: session removed, {} remaining", self.state.sessions.len());
                                    if self.state.sessions.is_empty() {
                                        log::debug!("close_tab: opening new local tab");
                                        self.open_local_tab();
                                        log::debug!("close_tab: new tab opened");
                                    }
                                }
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.toggle_left_sidebar {
                            if kb.matches(key, modifiers) { self.toggle_left_sidebar(); return; }
                        }
                        if let Some(ref kb) = self.shortcuts.toggle_right_sidebar {
                            if kb.matches(key, modifiers) { self.toggle_right_sidebar(); return; }
                        }
                        if let Some(ref kb) = self.shortcuts.new_connection {
                            if kb.matches(key, modifiers) {
                                self.state.new_connection_form =
                                    Some(NewConnectionForm::with_defaults());
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.focus_quick_connect {
                            if kb.matches(key, modifiers) {
                                if self.quick_connect_opened_sidebar && self.state.show_right_sidebar {
                                    // Toggle off: sidebar was opened by this shortcut, close it.
                                    self.state.show_right_sidebar = false;
                                    self.quick_connect_opened_sidebar = false;
                                    self.session_panel_state.quick_connect_query.clear();
                                } else if !self.state.show_right_sidebar {
                                    // Sidebar is closed — open it temporarily.
                                    self.quick_connect_opened_sidebar = true;
                                    self.state.show_right_sidebar = true;
                                    self.session_panel_state.quick_connect_focus = true;
                                } else {
                                    // Sidebar already open (by user) — just focus the search bar.
                                    self.session_panel_state.quick_connect_focus = true;
                                }
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.focus_plugin_search {
                            if kb.matches(key, modifiers) {
                                if !self.state.show_left_sidebar {
                                    self.plugin_search_opened_sidebar = true;
                                }
                                self.state.show_left_sidebar = true;
                                self.state.sidebar_tab = sidebar::SidebarTab::Plugins;
                                self.plugin_search_focus = true;
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.quit {
                            if kb.matches(key, modifiers) {
                                self.quit_requested = true;
                                return;
                            }
                        }

                        // Forward to active terminal only when no text widget has focus.
                        if forward_to_pty {
                            if let Some(bytes) = input::key_to_bytes(key, modifiers, None, &self.shortcuts, app_cursor) {
                                if let Some(session) = self.state.active_session() {
                                    // Scroll to bottom on user input (non-blocking).
                                    if let Some(mut term) = session.backend.term().try_lock_unfair() {
                                        term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                                    }
                                    session.backend.write(&bytes);
                                }
                            }
                        }
                    }
                    egui::Event::Text(text) => {
                        if forward_to_pty {
                            if let Some(session) = self.state.active_session() {
                                if let Some(mut term) = session.backend.term().try_lock_unfair() {
                                    term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                                }
                                session.backend.write(text.as_bytes());
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    /// Spawn an async SSH connection attempt on the tokio runtime.
    fn start_ssh_connect(
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

        // Build display info for the connecting tab.
        let label = host.clone();
        let detail = format!("{user}@{host}:{port}");
        self.pending_ssh_info.insert(id, PendingSshInfo {
            label,
            detail,
            started: Instant::now(),
        });

        // Create a tab immediately so the user sees the connecting screen.
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
            let result = SshSession::connect(&params, DEFAULT_COLS, DEFAULT_ROWS, term_config)
                .await
                .map_err(|e| format!("{host}: {e}"));
            let _ = tx.send(result);
        });

        self.pending_ssh_connections.push(PendingSsh { id, rx });
    }

    /// Save a server entry into the given folder (by top-level index).
    fn save_server_entry(
        &mut self,
        entry: conch_core::models::ServerEntry,
        folder_index: usize,
    ) {
        // Ensure at least one folder exists.
        if self.state.sessions_config.folders.is_empty() {
            self.state
                .sessions_config
                .folders
                .push(conch_core::models::ServerFolder::new("Servers"));
        }
        let idx = folder_index.min(self.state.sessions_config.folders.len() - 1);
        self.state.sessions_config.folders[idx].servers.push(entry);
        let _ = config::save_sessions(&self.state.sessions_config);
    }

    /// Close a session and activate the previous tab.
    fn remove_session(&mut self, id: Uuid) {
        if let Some(session) = self.state.sessions.remove(&id) {
            session.backend.shutdown();
        }
        self.state.tab_order.retain(|&tab_id| tab_id != id);
        if self.state.active_tab == Some(id) {
            self.state.active_tab = self.state.tab_order.last().copied();
        }

        // Shut down SFTP worker if it belonged to this session.
        if self.sftp_session_id == Some(id) {
            if let Some(tx) = self.sftp_cmd_tx.take() {
                let _ = tx.send(SftpCmd::Shutdown);
            }
            self.sftp_result_rx = None;
            self.sftp_session_id = None;
            self.remote_home = None;
            self.state.file_browser.remote_path = None;
            self.state.file_browser.remote_entries.clear();
            self.transfers.clear();
        }

        // Clean up pending connection info if closing a connecting tab.
        self.pending_ssh_info.remove(&id);
    }

    /// Open a new local terminal tab.
    fn open_local_tab(&mut self) {
        let _ = open_local_terminal(
            &mut self.state,
            self.last_cols,
            self.last_rows,
            self.cell_width,
            self.cell_height,
        );
    }

    /// Open a new OS window with a fresh local terminal tab.
    fn spawn_extra_window(&mut self) {
        let cwd = self.state
            .active_session()
            .and_then(|s| s.backend.child_pid())
            .and_then(conch_session::get_cwd_of_pid);
        let Some((_, session)) = create_local_session(&self.state.user_config, cwd) else {
            return;
        };
        let num = self.next_viewport_num;
        self.next_viewport_num += 1;
        let viewport_id = egui::ViewportId::from_hash_of(format!("conch_window_{num}"));
        self.extra_windows.push(ExtraWindow::new(viewport_id, session));
    }

    /// Collect all SSH server entries from sidebar folders + ssh_config hosts.
    fn collect_all_servers(&self) -> Vec<conch_core::models::ServerEntry> {
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
            // Avoid duplicates by session_key.
            if !servers.iter().any(|s| s.session_key() == host.session_key()) {
                servers.push(host.clone());
            }
        }
        servers
    }

    /// Kick off async tunnel activation (SSH connect + port forward).
    fn activate_tunnel(&mut self, tunnel: &SavedTunnel) {
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

    /// Resize all sessions if the computed grid dimensions changed.
    fn resize_sessions(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 || (cols == self.last_cols && rows == self.last_rows) {
            return;
        }
        self.last_cols = cols;
        self.last_rows = rows;
        let cw = self.cell_width as u16;
        let ch = self.cell_height as u16;
        for session in self.state.sessions.values() {
            session.backend.resize(cols, rows, cw, ch);
        }
    }
}

impl eframe::App for ConchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.style_applied {
            self.apply_initial_style(ctx);
        }

        // Measure cell size from the monospace font, and re-measure when
        // pixels_per_point changes (e.g. app launch on Retina where ppp
        // starts at 1.0 then jumps to 2.0, or moving between displays).
        let ppp = ctx.pixels_per_point();
        if !self.cell_size_measured || self.last_pixels_per_point != ppp {
            let (cw, ch) = measure_cell_size(ctx, self.state.user_config.font.size);
            let offset = &self.state.user_config.font.offset;
            if cw > 0.0 && ch > 0.0 {
                self.cell_width = (cw + offset.x).max(1.0);
                self.cell_height = (ch + offset.y).max(1.0);
                self.cell_size_measured = true;
                self.last_pixels_per_point = ppp;
                // Force a resize on the next render pass.
                self.last_cols = 0;
                self.last_rows = 0;
            }
        }

        // Cursor blink.
        let now = Instant::now();
        if now.duration_since(self.last_blink).as_millis() >= CURSOR_BLINK_MS {
            self.cursor_visible = !self.cursor_visible;
            self.last_blink = now;
            ctx.request_repaint();
        }

        self.poll_events(ctx);

        // Track window focus changes and send FOCUS_IN_OUT events to terminal.
        {
            let now_focused = ctx.input(|i| i.focused);
            if now_focused != self.window_focused {
                self.window_focused = now_focused;
                if let Some(session) = self.state.active_session() {
                    // Use try_lock to avoid blocking the main thread if the
                    // event loop is holding the FairMutex lease during PTY reads.
                    let focus_mode = session
                        .backend
                        .term()
                        .try_lock_unfair()
                        .map_or(false, |term| {
                            term.mode().contains(alacritty_terminal::term::TermMode::FOCUS_IN_OUT)
                        });
                    if focus_mode {
                        let seq = if now_focused { b"\x1b[I" } else { b"\x1b[O" };
                        session.backend.write(seq);
                    }
                }
            }
        }

        // Handle native macOS menu actions (only when native menu bar is active).
        // Route per-window actions to the focused window.
        #[cfg(target_os = "macos")]
        if self.use_native_menu {
            for action in crate::macos_menu::drain_actions() {
                use crate::macos_menu::MenuAction;
                match &action {
                    // Per-window actions: route to the focused extra window if any.
                    MenuAction::NewLocalTerminal
                    | MenuAction::ToggleLeftSidebar
                    | MenuAction::ToggleRightSidebar => {
                        if let Some(idx) = self.focused_extra_window {
                            if let Some(win) = self.extra_windows.get_mut(idx) {
                                match action {
                                    MenuAction::NewLocalTerminal => {
                                        win.open_local_tab(&self.state.user_config);
                                    }
                                    // Extra windows don't have sidebars — ignore.
                                    _ => {}
                                }
                            }
                        } else {
                            self.handle_macos_menu_action(action);
                        }
                    }
                    // Global actions always go to the main app.
                    _ => {
                        self.handle_macos_menu_action(action);
                    }
                }
            }
        }

        // Handle IPC messages from external processes.
        if let Some(ref listener) = self.ipc_listener {
            for msg in listener.drain() {
                match msg {
                    crate::ipc::IpcMessage::CreateWindow { working_directory } => {
                        let cwd = working_directory.map(std::path::PathBuf::from);
                        if let Some((_, session)) = create_local_session(&self.state.user_config, cwd) {
                            let num = self.next_viewport_num;
                            self.next_viewport_num += 1;
                            let viewport_id = egui::ViewportId::from_hash_of(format!("conch_window_{num}"));
                            self.extra_windows.push(ExtraWindow::new(viewport_id, session));
                        }
                    }
                    crate::ipc::IpcMessage::CreateTab { working_directory } => {
                        let cwd = working_directory.map(std::path::PathBuf::from);
                        if let Some((id, session)) = create_local_session(&self.state.user_config, cwd) {
                            self.state.sessions.insert(id, session);
                            self.state.tab_order.push(id);
                            self.state.active_tab = Some(id);
                        }
                    }
                }
            }
        }

        // Apply pending clipboard copy from plugins.
        if let Some(text) = self.pending_clipboard.take() {
            ctx.copy_text(text);
        }

        // Prevent egui's Tab focus-cycling from stealing Tab from the terminal.
        // Focus::begin_pass() already read RawInput and set focus_direction before
        // update() runs, so we must also undo the focus change after panels render.
        let consumed_tab_for_pty;
        {
            let no_widget_focused = !ctx.memory(|m| m.focused().is_some()) && !self.any_dialog_open();
            if no_widget_focused {
                let mut tab_bytes: Option<Vec<u8>> = None;
                ctx.input_mut(|i| {
                    i.events.retain(|e| match e {
                        egui::Event::Key {
                            key: egui::Key::Tab,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            tab_bytes = Some(if modifiers.shift {
                                b"\x1b[Z".to_vec()
                            } else {
                                b"\t".to_vec()
                            });
                            false // remove from queue
                        }
                        _ => true,
                    });
                });
                consumed_tab_for_pty = tab_bytes.is_some();
                if let Some(bytes) = tab_bytes {
                    if let Some(session) = self.state.active_session() {
                        session.backend.write(&bytes);
                    }
                }
            } else {
                consumed_tab_for_pty = false;
            }
        }

        // Collect copy/paste events before rendering panels.
        let mut copy_requested = false;
        let mut paste_text: Option<String> = None;
        ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Copy | egui::Event::Cut => copy_requested = true,
                    egui::Event::Paste(text) => paste_text = Some(text.clone()),
                    _ => {}
                }
            }
        });

        // Keyboard/paste forwarding is deferred until after all panels render,
        // so that egui's focus state reflects the current frame (see end of update).

        // Save window state before closing.
        let closing = self.quit_requested
            || ctx.input(|i| i.viewport().close_requested());
        if closing {
            self.save_window_state(ctx);
        }
        if self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // -- Dialogs (floating windows, rendered before panels) --

        if let Some(mut form) = self.state.new_connection_form.take() {
            let folder_names: Vec<String> = self
                .state
                .sessions_config
                .folders
                .iter()
                .map(|f| f.name.clone())
                .collect();
            match new_connection::show_new_connection(ctx, &mut form, &folder_names) {
                DialogAction::Save { entry, folder_index } => {
                    self.save_server_entry(entry, folder_index);
                }
                DialogAction::SaveAndConnect {
                    entry,
                    folder_index,
                    password,
                } => {
                    let host = entry.host.clone();
                    let port = entry.port;
                    let user = entry.user.clone();
                    let identity_file = entry.identity_file.clone();
                    let proxy_command = entry.proxy_command.clone();
                    let proxy_jump = entry.proxy_jump.clone();
                    self.save_server_entry(entry, folder_index);
                    self.start_ssh_connect(
                        host,
                        port,
                        user,
                        identity_file,
                        proxy_command,
                        proxy_jump,
                        password,
                    );
                }
                DialogAction::Cancel => {}
                DialogAction::None => {
                    self.state.new_connection_form = Some(form);
                }
            }
        }

        if self.show_about {
            self.show_about_dialog(ctx);
        }

        // Rename tab dialog.
        if let Some(tab_id) = self.rename_tab_id {
            let mut save = false;
            let mut cancel = false;
            egui::Window::new("Rename Tab")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    let te = ui.add(
                        crate::ui::widgets::text_edit(&mut self.rename_tab_buf)
                            .desired_width(200.0)
                            .hint_text("Tab name\u{2026}"),
                    );
                    if self.rename_tab_focus {
                        te.request_focus();
                        self.rename_tab_focus = false;
                    }
                    // Enter key confirms.
                    if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        save = true;
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::ui::widgets::dialog_button(ui, "Save").clicked() {
                                save = true;
                            }
                            if crate::ui::widgets::dialog_button(ui, "Cancel").clicked() {
                                cancel = true;
                            }
                        });
                    });
                });
            if save {
                let name = self.rename_tab_buf.trim().to_string();
                if let Some(session) = self.state.sessions.get_mut(&tab_id) {
                    session.custom_title = if name.is_empty() { None } else { Some(name) };
                }
                self.rename_tab_id = None;
                self.rename_tab_buf.clear();
            } else if cancel {
                self.rename_tab_id = None;
                self.rename_tab_buf.clear();
            }
        }

        // Preferences dialog.
        if let Some(mut form) = self.preferences_form.take() {
            match preferences::show_preferences(ctx, &mut form) {
                PreferencesAction::Save => {
                    let old_theme = self.state.user_config.colors.theme.clone();
                    form.apply_to_config(&mut self.state.user_config);
                    if let Err(e) = config::save_user_config(&self.state.user_config) {
                        log::error!("Failed to save config: {e}");
                    }
                    if self.state.user_config.colors.theme != old_theme {
                        let scheme = conch_core::color_scheme::resolve_theme(
                            &self.state.user_config.colors.theme,
                        );
                        self.state.colors =
                            crate::terminal::color::ResolvedColors::from_scheme(&scheme);
                    }
                    self.style_applied = false;
                    self.cell_size_measured = false;
                }
                PreferencesAction::Cancel => {}
                PreferencesAction::None => {
                    self.preferences_form = Some(form);
                }
            }
        }

        // Tunnel dialog.
        // Refresh active tunnel IDs (poll TunnelManager).
        if self.tunnel_dialog.is_some() {
            let tm = &self.tunnel_manager;
            let tunnels = &self.state.sessions_config.tunnels;
            let mut active = Vec::new();
            for t in tunnels {
                // Use try_lock to avoid blocking; falls back to previous state.
                let rt = &self.rt;
                if rt.block_on(tm.is_active(&t.id)) {
                    active.push(t.id);
                }
            }
            self.tunnel_active_ids = active;
        }

        // Poll pending tunnel activation results.
        self.pending_tunnel_results.retain(|(_id, rx)| {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    log::info!("Tunnel activated successfully");
                    false
                }
                Ok(Err(e)) => {
                    log::error!("Tunnel activation failed: {e}");
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
            }
        });

        if let Some(mut dialog) = self.tunnel_dialog.take() {
            let all_servers = self.collect_all_servers();
            let action = tunnels::show_tunnel_manager(
                ctx,
                &mut dialog,
                &self.state.sessions_config.tunnels,
                &self.tunnel_active_ids,
                &all_servers,
            );
            match action {
                TunnelManagerAction::NewTunnel(tunnel) => {
                    self.activate_tunnel(&tunnel);
                    self.state.sessions_config.tunnels.push(tunnel);
                    let _ = config::save_sessions(&self.state.sessions_config);
                    self.tunnel_dialog = Some(dialog);
                }
                TunnelManagerAction::Activate(id) => {
                    if let Some(tunnel) = self.state.sessions_config.tunnels.iter().find(|t| t.id == id).cloned() {
                        self.activate_tunnel(&tunnel);
                    }
                    self.tunnel_dialog = Some(dialog);
                }
                TunnelManagerAction::Stop(id) => {
                    let tm = self.tunnel_manager.clone_inner();
                    self.rt.spawn(async move {
                        tm.stop(&id).await;
                    });
                    self.tunnel_dialog = Some(dialog);
                }
                TunnelManagerAction::Delete(id) => {
                    // Stop if active, then remove from saved list.
                    let tm = self.tunnel_manager.clone_inner();
                    self.rt.spawn(async move {
                        tm.stop(&id).await;
                    });
                    self.state.sessions_config.tunnels.retain(|t| t.id != id);
                    let _ = config::save_sessions(&self.state.sessions_config);
                    self.tunnel_dialog = Some(dialog);
                }
                TunnelManagerAction::Close => {
                    // Dialog closed — don't put it back.
                }
                TunnelManagerAction::None => {
                    self.tunnel_dialog = Some(dialog);
                }
            }
        }

        // Plugin dialog (form, prompt, confirm, alert, error, text, table).
        if let Some(mut dialog) = self.active_plugin_dialog.take() {
            if plugin_dialog::show_plugin_dialog(ctx, &mut dialog) {
                // Dialog was closed (response already sent via channel).
            } else {
                self.active_plugin_dialog = Some(dialog);
            }
        }

        // Plugin progress indicator.
        if let Some(msg) = &self.plugin_progress {
            egui::Window::new("Working...")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(msg.as_str());
                    });
                });
        }

        // -- Panels --

        {
            use config::WindowDecorations;
            let decorations = self.state.user_config.window.decorations;

            // Buttonless: no title bar, so add a thin drag region at the top so the
            // user can still move the window by dragging the first terminal line.
            if decorations == WindowDecorations::Buttonless {
                let drag_h = self.cell_height.max(6.0);
                egui::TopBottomPanel::top("drag_region")
                    .exact_height(drag_h)
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let rect = ui.available_rect_before_wrap();
                        let response = ui.interact(rect, ui.id().with("drag"), egui::Sense::drag());
                        if response.drag_started() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                    });
            }

            // When using native menu bar on macOS with fullsize_content_view (Transparent
            // or Full-non-native), the in-window menu bar is not rendered, but the content
            // still extends behind the title bar area. Add a spacer to push content below
            // the ~28px title bar. Buttonless and None have no title bar, so no spacer.
            if self.use_native_menu {
                let needs_spacer = match decorations {
                    WindowDecorations::None | WindowDecorations::Buttonless => false,
                    _ => cfg!(target_os = "macos"),
                };
                if needs_spacer {
                    egui::TopBottomPanel::top("titlebar_spacer")
                        .exact_height(28.0)
                        .frame(egui::Frame::NONE)
                        .show(ctx, |_ui| {});
                }
            }
        }

        // Menu bar (File, Sessions, Tools, View, Help).
        // Shown in-window when native macOS menu bar is disabled (or on non-macOS).
        // On macOS with fullsize_content_view, the menu lives inside the title bar area.
        if !self.use_native_menu {
            // On macOS with transparent title bar, add top padding to clear the
            // title bar chrome and left padding for the traffic light buttons.
            let in_titlebar = cfg!(target_os = "macos");
            // On macOS, the title bar is ~28px. Traffic light centers are at ~14px.
            // We pad top so menu text aligns vertically with them.
            let top_pad: i8 = if in_titlebar { 7 } else { 4 };
            let bottom_pad: i8 = if in_titlebar { 6 } else { 4 };
            let left_pad: i8 = if in_titlebar { 72 } else { 8 };

            egui::TopBottomPanel::top("menu_bar")
                .frame(egui::Frame::side_top_panel(ctx.style().as_ref())
                    .inner_margin(egui::Margin { top: top_pad, bottom: bottom_pad, left: left_pad, right: 8 }))
                .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    // App title on the left (next to traffic lights).
                    if in_titlebar {
                        ui.label(egui::RichText::new("Conch").color(ui.visuals().weak_text_color()));
                        ui.add_space(4.0);
                    }

                    // Menu buttons right-aligned.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Render in reverse order (right-to-left).
                        ui.menu_button("Help", |ui| {
                            if ui.button("About Conch").clicked() {
                                self.show_about = true;
                                ui.close_menu();
                            }
                        });

                        ui.menu_button("View", |ui| {
                            let left_check = if self.state.show_left_sidebar { "✓ " } else { "   " };
                            if ui.add(egui::Button::new(format!("{left_check}Left Toolbar")).shortcut_text(cmd_shortcut("1"))).clicked() {
                                self.toggle_left_sidebar();
                                ui.close_menu();
                            }
                            let right_check = if self.state.show_right_sidebar { "✓ " } else { "   " };
                            if ui.add(egui::Button::new(format!("{right_check}Right Toolbar")).shortcut_text(cmd_shortcut("2"))).clicked() {
                                self.toggle_right_sidebar();
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Preferences...").clicked() {
                                self.preferences_form =
                                    Some(PreferencesForm::from_config(&self.state.user_config));
                                ui.close_menu();
                            }
                        });

                        ui.menu_button("Tools", |ui| {
                            if ui.button("SSH Tunnels...").clicked() {
                                self.tunnel_dialog = Some(TunnelManagerState::new());
                                ui.close_menu();
                            }
                            if !self.discovered_plugins.is_empty() {
                                ui.separator();
                                let mut run_idx = None;
                                for (i, meta) in self.discovered_plugins.iter().enumerate() {
                                    if ui.button(&meta.name).clicked() {
                                        run_idx = Some(i);
                                        ui.close_menu();
                                    }
                                }
                                if let Some(idx) = run_idx {
                                    self.run_plugin_by_index(idx);
                                }
                            }
                        });

                        ui.menu_button("Sessions", |ui| {
                            if ui.add(egui::Button::new("New Local Terminal").shortcut_text(cmd_shortcut("T"))).clicked() {
                                self.open_local_tab();
                                ui.close_menu();
                            }
                            if ui.add(egui::Button::new("New SSH Session...").shortcut_text(cmd_shortcut("N"))).clicked() {
                                self.state.new_connection_form =
                                    Some(NewConnectionForm::with_defaults());
                                ui.close_menu();
                            }
                        });

                        ui.menu_button("File", |ui| {
                            if ui.add(egui::Button::new("New Connection...").shortcut_text(cmd_shortcut("N"))).clicked() {
                                self.state.new_connection_form =
                                    Some(NewConnectionForm::with_defaults());
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.add(egui::Button::new("Quit Conch").shortcut_text(cmd_shortcut("Q"))).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                ui.close_menu();
                            }
                        });
                    });
                });
            });
        } // end if !use_native_menu

        // Left sidebar: narrow tab strip + resizable content panel.
        let (sidebar_action, deferred_plugin_action) = {
            let icons = self.icon_cache.as_ref();
            let plugin_display: Vec<sidebar::PluginDisplayInfo> = self
                .discovered_plugins
                .iter()
                .map(|meta| {
                    let is_running = self
                        .running_plugins
                        .iter()
                        .any(|rp| rp.meta.path == meta.path);
                    sidebar::PluginDisplayInfo {
                        name: meta.name.clone(),
                        description: meta.description.clone(),
                        is_running,
                    }
                })
                .collect();
            let action = if self.state.show_left_sidebar {
                sidebar::show_tab_strip(ctx, &mut self.state.sidebar_tab, icons);
                sidebar::show_sidebar_content(
                    ctx,
                    &self.state.sidebar_tab,
                    &mut self.state.file_browser,
                    icons,
                    &plugin_display,
                    &self.plugin_output_lines,
                    &mut self.selected_plugin,
                    &self.transfers,
                    &mut self.plugin_search_query,
                    &mut self.plugin_search_focus,
                )
            } else {
                SidebarAction::None
            };
            let deferred = match &action {
                SidebarAction::RunPlugin(i) => Some(SidebarAction::RunPlugin(*i)),
                SidebarAction::StopPlugin(i) => Some(SidebarAction::StopPlugin(*i)),
                SidebarAction::RefreshPlugins => Some(SidebarAction::RefreshPlugins),
                _ => None,
            };
            (action, deferred)
        };
        self.handle_sidebar_action(sidebar_action);

        // Right sidebar (session / server tree).
        let mut panel_action = SessionPanelAction::None;
        if self.state.show_right_sidebar {
            let icons = self.icon_cache.as_ref();
            egui::SidePanel::right("right_sidebar")
                .resizable(true)
                .default_width(220.0)
                .width_range(100.0..=400.0)
                .show(ctx, |ui| {
                    panel_action = session_panel::show_session_panel(
                        ui,
                        &self.state.sessions_config.folders,
                        &self.state.ssh_config_hosts,
                        icons,
                        &mut self.session_panel_state,
                    );
                });
        }
        // Escape in the quick connect search bar → restore sidebar to previous state.
        if self.session_panel_state.dismissed {
            self.session_panel_state.dismissed = false;
            if self.quick_connect_opened_sidebar {
                self.state.show_right_sidebar = false;
                self.quick_connect_opened_sidebar = false;
            }
        }
        self.handle_session_panel_action(panel_action);

        // Tab bar — only shown when multiple tabs are open.
        if self.state.tab_order.len() > 1 {
        egui::TopBottomPanel::top("tab_bar")
            .exact_height(28.0)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let panel_rect = ui.available_rect_before_wrap();
                let painter = ui.painter_at(panel_rect);
                let style = ui.style();
                let base_bg = style.visuals.panel_fill;
                let darker_bg = sidebar::darken_color(base_bg, 18);
                let accent_color = egui::Color32::from_rgb(47, 101, 202);
                let text_color = style.visuals.text_color();
                let dim_text = style.visuals.weak_text_color();
                let font_id = egui::FontId::new(13.0, egui::FontFamily::Proportional);

                const TAB_MAX_W: f32 = 140.0;
                let tab_h = panel_rect.height();

                // Fill entire strip with darker background.
                painter.rect_filled(panel_rect, 0.0, darker_bg);

                let mut switch_to = None;
                let mut close_id = None;
                let mut x = panel_rect.min.x;

                let tab_count = self.state.tab_order.len();
                // Equal width, capped at TAB_MAX_W
                let tab_w = if tab_count > 0 {
                    TAB_MAX_W.min(panel_rect.width() / (tab_count as f32 + 1.0))
                } else {
                    TAB_MAX_W
                };

                let hint_font = egui::FontId::new(9.0, egui::FontFamily::Proportional);
                let mut rename_id = None;

                for (tab_idx, &id) in self.state.tab_order.iter().enumerate() {
                    // Determine the tab title — either from the session or from pending info.
                    let tab_title_str: Option<String> = if let Some(session) = self.state.sessions.get(&id) {
                        Some(session.custom_title.as_deref().unwrap_or(&session.title).to_string())
                    } else if let Some(info) = self.pending_ssh_info.get(&id) {
                        Some(format!("{}...", info.label))
                    } else {
                        None
                    };
                    if let Some(title) = tab_title_str {
                        let selected = self.state.active_tab == Some(id);
                        let tab_rect = egui::Rect::from_min_size(
                            egui::Pos2::new(x, panel_rect.min.y),
                            egui::Vec2::new(tab_w, tab_h),
                        );

                        // Background: selected gets lighter panel fill.
                        if selected {
                            painter.rect_filled(tab_rect, 0.0, base_bg);
                            // Accent bar on bottom edge.
                            let accent_rect = egui::Rect::from_min_size(
                                egui::Pos2::new(tab_rect.min.x, tab_rect.max.y - 3.0),
                                egui::Vec2::new(tab_w, 3.0),
                            );
                            painter.rect_filled(accent_rect, 0.0, accent_color);
                        }

                        // Separator line between tabs.
                        if x > panel_rect.min.x {
                            painter.line_segment(
                                [
                                    egui::Pos2::new(x, panel_rect.min.y + 4.0),
                                    egui::Pos2::new(x, panel_rect.max.y - 4.0),
                                ],
                                egui::Stroke::new(
                                    1.0,
                                    style.visuals.widgets.noninteractive.bg_stroke.color,
                                ),
                            );
                        }

                        // Close button icon on the right side.
                        let close_size = 14.0;
                        let close_pad = 4.0;
                        let close_x = tab_rect.max.x - close_size - close_pad;
                        let close_y = tab_rect.center().y - close_size / 2.0;
                        let close_rect = egui::Rect::from_min_size(
                            egui::Pos2::new(close_x - 2.0, close_y - 2.0),
                            egui::Vec2::new(close_size + 4.0, close_size + 4.0),
                        );
                        if let Some(tex_id) = self.icon_cache.as_ref().and_then(|ic| ic.texture_id(Icon::TabClose)) {
                            painter.image(
                                tex_id,
                                egui::Rect::from_min_size(
                                    egui::Pos2::new(close_x, close_y),
                                    egui::Vec2::new(close_size, close_size),
                                ),
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }

                        // Tab number hint (right of title, left of close button).
                        let hint_text = format!("{}", tab_idx + 1);
                        let hint_galley = painter.layout_no_wrap(
                            hint_text,
                            hint_font.clone(),
                            style.visuals.weak_text_color(),
                        );
                        let hint_w = hint_galley.size().x;
                        let hint_x = close_x - hint_w - 4.0;
                        let hint_y = tab_rect.center().y - hint_galley.size().y / 2.0;
                        painter.galley(
                            egui::Pos2::new(hint_x, hint_y),
                            hint_galley,
                            style.visuals.weak_text_color(),
                        );

                        // Tab label — pixel-based truncation with "…".
                        let label_color = if selected { text_color } else { dim_text };
                        let left_pad = 6.0;
                        let max_text_w = hint_x - tab_rect.min.x - left_pad - 2.0;

                        let full_galley = painter.layout_no_wrap(
                            title.to_string(),
                            font_id.clone(),
                            label_color,
                        );

                        let galley = if full_galley.size().x > max_text_w && max_text_w > 0.0 {
                            // Binary search for the longest prefix that fits with "…".
                            let ellipsis_w = painter.layout_no_wrap(
                                "\u{2026}".to_string(),
                                font_id.clone(),
                                label_color,
                            ).size().x;
                            let target_w = max_text_w - ellipsis_w;
                            let mut end = title.len();
                            while end > 0 {
                                let truncated = &title[..end];
                                let g = painter.layout_no_wrap(
                                    truncated.to_string(),
                                    font_id.clone(),
                                    label_color,
                                );
                                if g.size().x <= target_w {
                                    break;
                                }
                                // Step back by one char.
                                end = truncated.floor_char_boundary(end.saturating_sub(1));
                            }
                            let mut truncated = title[..end].to_string();
                            truncated.push('\u{2026}');
                            painter.layout_no_wrap(truncated, font_id.clone(), label_color)
                        } else {
                            full_galley
                        };

                        let text_pos = egui::Pos2::new(
                            tab_rect.min.x + left_pad,
                            tab_rect.center().y - galley.size().y / 2.0,
                        );
                        painter.galley(text_pos, galley, label_color);

                        // Click detection: check pointer position manually to
                        // avoid overlapping-interact issues in egui.
                        let tab_resp = ui.interact(
                            tab_rect,
                            ui.id().with(("tab_select", id)),
                            egui::Sense::click(),
                        );
                        if tab_resp.clicked() {
                            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                                if close_rect.contains(pos) {
                                    close_id = Some(id);
                                } else {
                                    switch_to = Some(id);
                                }
                            }
                        }
                        // Right-click context menu for rename.
                        tab_resp.context_menu(|ui| {
                            if ui.button("Rename Tab").clicked() {
                                rename_id = Some(id);
                                ui.close_menu();
                            }
                        });

                        x += tab_w;
                    }
                }

                // Open rename dialog if requested from context menu.
                if let Some(id) = rename_id {
                    if let Some(session) = self.state.sessions.get(&id) {
                        self.rename_tab_buf = session.custom_title.clone()
                            .unwrap_or_else(|| session.title.clone());
                        self.rename_tab_id = Some(id);
                        self.rename_tab_focus = true;
                    }
                }

                // "+" button after tabs.
                let plus_rect = egui::Rect::from_min_size(
                    egui::Pos2::new(x + 4.0, panel_rect.min.y),
                    egui::Vec2::new(24.0, tab_h),
                );
                let plus_galley = painter.layout_no_wrap(
                    "+".to_string(),
                    font_id.clone(),
                    dim_text,
                );
                painter.galley(
                    egui::Pos2::new(
                        plus_rect.center().x - plus_galley.size().x / 2.0,
                        plus_rect.center().y - plus_galley.size().y / 2.0,
                    ),
                    plus_galley,
                    dim_text,
                );
                let plus_resp = ui.interact(
                    plus_rect,
                    ui.id().with("tab_plus"),
                    egui::Sense::click(),
                );
                if plus_resp.clicked() {
                    self.open_local_tab();
                }

                if let Some(id) = switch_to {
                    self.state.active_tab = Some(id);
                }
                if let Some(id) = close_id {
                    self.remove_session(id);
                    if self.state.sessions.is_empty() {
                        self.open_local_tab();
                    }
                }
            });
        } // end tab bar (multi-tab only)

        // Central panel (active terminal or connecting screen).
        let font_size = self.state.user_config.font.size;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                if let Some(id) = self.state.active_tab {
                    if let Some(session) = self.state.sessions.get(&id) {
                        let term = session.backend.term().clone();
                        let (response, size_info) = show_terminal(
                            ui,
                            &term,
                            self.cell_width,
                            self.cell_height,
                            &self.state.colors,
                            font_size,
                            self.cursor_visible,
                            self.selection.normalized(),
                        );

                        {
                            let cell_height = self.cell_height;
                            let write_fn = |data: &[u8]| {
                                if let Some(s) = self.state.active_session() {
                                    s.backend.write(data);
                                }
                            };
                            handle_terminal_mouse(
                                ctx,
                                &response,
                                &size_info,
                                &mut self.selection,
                                &term,
                                &write_fn,
                                cell_height,
                            );
                        }

                        if copy_requested {
                            if let Some((start, end)) = self.selection.normalized() {
                                let text = get_selected_text(&term, start, end);
                                if !text.is_empty() {
                                    ui.ctx().copy_text(text);
                                }
                            }
                        }

                        self.resize_sessions(
                            size_info.columns() as u16,
                            size_info.rows() as u16,
                        );
                    } else if let Some(info) = self.pending_ssh_info.get(&id) {
                        // Connecting screen.
                        show_connecting_screen(ui, info);
                        ctx.request_repaint();
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("No terminal open. Press Ctrl+T to open one.");
                    });
                }
            });

        // Now that all panels have been laid out, check if a text-entry
        // widget (TextEdit) has focus. The terminal no longer grabs egui
        // focus, so `focused().is_some()` means a real text widget is active.
        //
        // If we consumed Tab for the PTY, undo any focus that egui's
        // begin_frame focus-direction cycling caused during widget layout.
        if consumed_tab_for_pty {
            if let Some(id) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
        }
        let forward_to_pty = !ctx.memory(|m| m.focused().is_some()) && !self.any_dialog_open();

        if let Some(text) = paste_text {
            if forward_to_pty {
                if let Some(session) = self.state.active_session() {
                    let bracketed = session
                        .backend
                        .term()
                        .try_lock_unfair()
                        .map_or(false, |term| {
                            term.mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
                        });
                    if bracketed {
                        session.backend.write(b"\x1b[200~");
                        session.backend.write(text.as_bytes());
                        session.backend.write(b"\x1b[201~");
                    } else {
                        session.backend.write(text.as_bytes());
                    }
                }
            }
        }

        self.handle_keyboard(ctx, forward_to_pty);

        // Handle deferred plugin sidebar actions (after all borrows are released).
        if let Some(action) = deferred_plugin_action {
            match action {
                SidebarAction::RunPlugin(idx) => {
                    self.run_plugin_by_index(idx);
                    if self.plugin_search_opened_sidebar {
                        self.state.show_left_sidebar = false;
                        self.plugin_search_opened_sidebar = false;
                    }
                }
                SidebarAction::StopPlugin(idx) => {
                    if let Some(meta) = self.discovered_plugins.get(idx) {
                        let path = meta.path.clone();
                        if let Some(pos) = self.running_plugins.iter().position(|rp| rp.meta.path == path) {
                            self.stop_plugin(pos);
                        }
                    }
                    if self.plugin_search_opened_sidebar {
                        self.state.show_left_sidebar = false;
                        self.plugin_search_opened_sidebar = false;
                    }
                }
                SidebarAction::RefreshPlugins => self.refresh_plugins(),
                _ => {}
            }
        }

        let window_title = self.state.active_session()
            .map(|s| {
                let name = s.custom_title.as_ref().unwrap_or(&s.title);
                format!("{name} — Conch")
            })
            .unwrap_or_else(|| "Conch".into());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(window_title));
        // Poll for async events (SSH, plugins, tunnels) at ~10 Hz when idle.
        // Active terminal output triggers immediate repaints via Wakeup events,
        // and cursor blink already requests repaint every 500ms.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        // Keep window state in memory each frame so on_exit can persist it.
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.state.persistent.layout.window_width = rect.width();
            self.state.persistent.layout.window_height = rect.height();
        }
        self.state.persistent.layout.zoom_factor = ctx.zoom_factor();

        // Render extra windows via show_viewport_immediate.
        // We take the vec out to satisfy the borrow checker — the closure
        // borrows each ExtraWindow mutably while we read shared state from self.
        let mut extra = std::mem::take(&mut self.extra_windows);
        let mut focused_extra: Option<usize> = None;
        {
            let user_config = &self.state.user_config;
            let colors = &self.state.colors;
            let shortcuts = &self.shortcuts;
            let icon_cache = &self.icon_cache;

            for (idx, win) in extra.iter_mut().enumerate() {
                if win.should_close {
                    continue;
                }
                let builder = egui::ViewportBuilder::default()
                    .with_title(&win.title)
                    .with_inner_size([800.0, 600.0]);
                let vid = win.viewport_id;
                ctx.show_viewport_immediate(vid, builder, |vp_ctx, _class| {
                    win.update(vp_ctx, user_config, colors, shortcuts, icon_cache);
                });
                if win.is_focused {
                    focused_extra = Some(idx);
                }
            }
        }
        self.focused_extra_window = focused_extra;
        // Remove closed windows and put the rest back.
        extra.retain(|w| !w.should_close);
        self.extra_windows = extra;
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Shut down sessions in extra windows.
        for win in &self.extra_windows {
            for session in win.sessions.values() {
                session.backend.shutdown();
            }
        }
        let _ = config::save_persistent_state(&self.state.persistent);
    }
}

impl ConchApp {
    /// Persist current window size and zoom factor to state.toml.
    fn save_window_state(&mut self, ctx: &egui::Context) {
        // Read the current inner rect from the viewport.
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.state.persistent.layout.window_width = rect.width();
            self.state.persistent.layout.window_height = rect.height();
        }
        self.state.persistent.layout.zoom_factor = ctx.zoom_factor();
        let _ = config::save_persistent_state(&self.state.persistent);
    }

    #[cfg(target_os = "macos")]
    fn handle_macos_menu_action(
        &mut self,
        action: crate::macos_menu::MenuAction,
    ) {
        use crate::macos_menu::MenuAction;
        match action {
            MenuAction::NewConnection | MenuAction::NewSshSession => {
                self.state.new_connection_form =
                    Some(NewConnectionForm::with_defaults());
            }
            MenuAction::NewWindow => {
                self.spawn_extra_window();
            }
            MenuAction::NewLocalTerminal => {
                self.open_local_tab();
            }
            MenuAction::SshTunnels => {
                self.tunnel_dialog = Some(TunnelManagerState::new());
            }
            MenuAction::ToggleLeftSidebar => {
                self.toggle_left_sidebar();
            }
            MenuAction::ToggleRightSidebar => {
                self.toggle_right_sidebar();
            }
            MenuAction::Preferences => {
                self.preferences_form =
                    Some(PreferencesForm::from_config(&self.state.user_config));
            }
            MenuAction::AboutConch => {
                self.show_about = true;
            }
            MenuAction::RunPlugin(idx) => {
                self.run_plugin_by_index(idx);
            }
        }
    }

    fn toggle_left_sidebar(&mut self) {
        self.state.show_left_sidebar = !self.state.show_left_sidebar;
        self.state.persistent.layout.left_panel_collapsed = !self.state.show_left_sidebar;
        let _ = config::save_persistent_state(&self.state.persistent);
    }

    fn toggle_right_sidebar(&mut self) {
        self.state.show_right_sidebar = !self.state.show_right_sidebar;
        self.state.persistent.layout.right_panel_collapsed = !self.state.show_right_sidebar;
        let _ = config::save_persistent_state(&self.state.persistent);
    }

    /// Show the About Conch dialog.
    fn show_about_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("About Conch")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("Conch");
                    ui.label("Version 0.2");
                    ui.add_space(4.0);
                    ui.label("A cross-platform SSH terminal emulator.");
                    ui.add_space(8.0);
                    if crate::ui::widgets::dialog_button(ui, "OK").clicked() {
                        self.show_about = false;
                    }
                });
            });
    }

    fn handle_sidebar_action(&mut self, action: SidebarAction) {
        match action {
            SidebarAction::NavigateLocal(path) => {
                let old = self.state.file_browser.local_path.clone();
                self.state.file_browser.local_back_stack.push(old);
                self.state.file_browser.local_forward_stack.clear();
                self.state.file_browser.local_entries = load_local_entries(&path);
                self.state.file_browser.local_path_edit = path.to_string_lossy().into_owned();
                self.state.file_browser.local_path = path;
                self.state.file_browser.local_selected = None;
            }
            SidebarAction::GoBackLocal => {
                if let Some(prev) = self.state.file_browser.local_back_stack.pop() {
                    let current = self.state.file_browser.local_path.clone();
                    self.state.file_browser.local_forward_stack.push(current);
                    self.state.file_browser.local_entries = load_local_entries(&prev);
                    self.state.file_browser.local_path_edit = prev.to_string_lossy().into_owned();
                    self.state.file_browser.local_path = prev;
                    self.state.file_browser.local_selected = None;
                }
            }
            SidebarAction::GoForwardLocal => {
                if let Some(next) = self.state.file_browser.local_forward_stack.pop() {
                    let current = self.state.file_browser.local_path.clone();
                    self.state.file_browser.local_back_stack.push(current);
                    self.state.file_browser.local_entries = load_local_entries(&next);
                    self.state.file_browser.local_path_edit = next.to_string_lossy().into_owned();
                    self.state.file_browser.local_path = next;
                    self.state.file_browser.local_selected = None;
                }
            }
            SidebarAction::RefreshLocal => {
                let path = self.state.file_browser.local_path.clone();
                self.state.file_browser.local_entries = load_local_entries(&path);
                self.state.file_browser.local_selected = None;
            }
            SidebarAction::GoHomeLocal => {
                let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
                let old = self.state.file_browser.local_path.clone();
                self.state.file_browser.local_back_stack.push(old);
                self.state.file_browser.local_forward_stack.clear();
                self.state.file_browser.local_entries = load_local_entries(&home);
                self.state.file_browser.local_path_edit = home.to_string_lossy().into_owned();
                self.state.file_browser.local_path = home;
                self.state.file_browser.local_selected = None;
            }
            SidebarAction::SelectFile(path) => {
                log::info!("File selected: {}", path.display());
            }
            SidebarAction::NavigateRemote(path) => {
                if let Some(old) = self.state.file_browser.remote_path.clone() {
                    self.state.file_browser.remote_back_stack.push(old);
                    self.state.file_browser.remote_forward_stack.clear();
                }
                if let Some(tx) = &self.sftp_cmd_tx {
                    let _ = tx.send(SftpCmd::List(path));
                }
            }
            SidebarAction::GoBackRemote => {
                if let Some(prev) = self.state.file_browser.remote_back_stack.pop() {
                    if let Some(current) = self.state.file_browser.remote_path.clone() {
                        self.state.file_browser.remote_forward_stack.push(current);
                    }
                    if let Some(tx) = &self.sftp_cmd_tx {
                        let _ = tx.send(SftpCmd::List(prev));
                    }
                }
            }
            SidebarAction::GoForwardRemote => {
                if let Some(next) = self.state.file_browser.remote_forward_stack.pop() {
                    if let Some(current) = self.state.file_browser.remote_path.clone() {
                        self.state.file_browser.remote_back_stack.push(current);
                    }
                    if let Some(tx) = &self.sftp_cmd_tx {
                        let _ = tx.send(SftpCmd::List(next));
                    }
                }
            }
            SidebarAction::RefreshRemote => {
                if let Some(tx) = &self.sftp_cmd_tx {
                    if let Some(rp) = &self.state.file_browser.remote_path {
                        let _ = tx.send(SftpCmd::List(rp.clone()));
                    }
                }
            }
            SidebarAction::GoHomeRemote => {
                if let Some(home) = self.remote_home.clone() {
                    if let Some(old) = self.state.file_browser.remote_path.clone() {
                        self.state.file_browser.remote_back_stack.push(old);
                        self.state.file_browser.remote_forward_stack.clear();
                    }
                    if let Some(tx) = &self.sftp_cmd_tx {
                        let _ = tx.send(SftpCmd::List(home));
                    }
                }
            }
            SidebarAction::Upload { local_path, remote_dir } => {
                let filename = local_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let cancel = Arc::new(AtomicBool::new(false));
                self.transfers.push(sidebar::TransferStatus {
                    filename,
                    upload: true,
                    done: false,
                    error: None,
                    bytes_transferred: 0,
                    total_bytes: 0,
                    cancel: cancel.clone(),
                });
                if let Some(tx) = &self.sftp_cmd_tx {
                    let _ = tx.send(SftpCmd::Upload { local_path, remote_dir, cancel });
                }
            }
            SidebarAction::Download { remote_path, local_dir } => {
                let filename = remote_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let cancel = Arc::new(AtomicBool::new(false));
                self.transfers.push(sidebar::TransferStatus {
                    filename,
                    upload: false,
                    done: false,
                    error: None,
                    bytes_transferred: 0,
                    total_bytes: 0,
                    cancel: cancel.clone(),
                });
                if let Some(tx) = &self.sftp_cmd_tx {
                    let _ = tx.send(SftpCmd::Download { remote_path, local_dir, cancel });
                }
            }
            SidebarAction::CancelTransfer(filename) => {
                if let Some(ts) = self
                    .transfers
                    .iter_mut()
                    .find(|t| t.filename == filename && !t.done)
                {
                    ts.cancel.store(true, Ordering::Relaxed);
                }
            }
            SidebarAction::RunPlugin(_)
            | SidebarAction::StopPlugin(_)
            | SidebarAction::RefreshPlugins
            | SidebarAction::None => {}
        }
    }

    fn handle_session_panel_action(&mut self, action: SessionPanelAction) {
        match action {
            SessionPanelAction::Connect(req) => {
                self.start_ssh_connect(
                    req.host,
                    req.port,
                    req.user,
                    req.identity_file,
                    req.proxy_command,
                    req.proxy_jump,
                    req.password,
                );
                if self.quick_connect_opened_sidebar {
                    self.state.show_right_sidebar = false;
                    self.quick_connect_opened_sidebar = false;
                }
            }
            SessionPanelAction::CreateFolder { parent_path, name } => {
                let folder = conch_core::models::ServerFolder::new(name);
                if parent_path.is_empty() {
                    self.state.sessions_config.folders.push(folder);
                } else if let Some(parent) = find_folder_mut(&mut self.state.sessions_config.folders, &parent_path) {
                    parent.subfolders.push(folder);
                }
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::RenameFolder { path, new_name } => {
                if let Some(f) = find_folder_mut(&mut self.state.sessions_config.folders, &path) {
                    f.name = new_name;
                }
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::DeleteFolder { path } => {
                delete_folder(&mut self.state.sessions_config.folders, &path);
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::CreateServer { folder_path } => {
                let entry = conch_core::models::ServerEntry {
                    name: "New Server".into(),
                    host: String::new(),
                    port: 22,
                    user: String::new(),
                    identity_file: None,
                    proxy_command: None,
                    proxy_jump: None,
                    startup_command: None,
                    session_key: None,
                    from_ssh_config: false,
                };
                if folder_path.is_empty() {
                    if self.state.sessions_config.folders.is_empty() {
                        self.state.sessions_config.folders.push(
                            conch_core::models::ServerFolder::new("Servers"),
                        );
                    }
                    self.state.sessions_config.folders[0].servers.push(entry);
                } else if let Some(f) = find_folder_mut(&mut self.state.sessions_config.folders, &folder_path) {
                    f.servers.push(entry);
                }
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::RenameServer { addr, new_name } => {
                if let Some(server) = find_server_mut(&mut self.state.sessions_config.folders, &addr) {
                    server.name = new_name;
                }
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::DeleteServer { addr } => {
                delete_server(&mut self.state.sessions_config.folders, &addr);
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::EditServer { .. } => {}
            SessionPanelAction::OpenNewConnectionDialog => {
                self.state.new_connection_form =
                    Some(NewConnectionForm::with_defaults());
            }
            SessionPanelAction::None => {}
        }
    }

    /// Apply the initial visual style, load icons, and set up platform-specific
    /// window decorations. Called once on the first frame.
    fn apply_initial_style(&mut self, ctx: &egui::Context) {
        match self.state.user_config.colors.appearance_mode.as_str() {
            "light" => {
                ctx.set_visuals(egui::Visuals::light());
                ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Light);
                ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Light));
            }
            "system" => {
                ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::System);
                ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::SystemDefault));
            }
            _ => {
                ctx.set_visuals(egui::Visuals::dark());
                ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
                ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
            }
        }
        let mut style = (*ctx.style()).clone();
        style.visuals.window_corner_radius = egui::CornerRadius::ZERO;
        style.visuals.menu_corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.open.corner_radius = egui::CornerRadius::ZERO;

        if style.visuals.dark_mode {
            let fg = egui::Color32::from_gray(240);
            style.visuals.widgets.noninteractive.fg_stroke.color = fg;
            style.visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_gray(220);
            style.visuals.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;
            style.visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;
            style.visuals.widgets.open.fg_stroke.color = fg;

            let border = egui::Color32::from_gray(80);
            let border_focus = egui::Color32::from_gray(140);
            style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, border);
            style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, border_focus);
            style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, border_focus);
        } else {
            let base = egui::Color32::from_gray(0xF2);
            let accent = egui::Color32::WHITE;

            style.visuals.panel_fill = base;
            style.visuals.window_fill = base;
            style.visuals.extreme_bg_color = accent;
            style.visuals.faint_bg_color = egui::Color32::from_gray(0xE8);

            style.visuals.widgets.noninteractive.bg_fill = base;
            style.visuals.widgets.inactive.bg_fill = accent;
            style.visuals.widgets.hovered.bg_fill = egui::Color32::from_gray(0xE8);
            style.visuals.widgets.active.bg_fill = egui::Color32::from_gray(0xDD);
            style.visuals.widgets.open.bg_fill = accent;

            let fg = egui::Color32::from_gray(10);
            style.visuals.widgets.noninteractive.fg_stroke.color = fg;
            style.visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_gray(30);
            style.visuals.widgets.hovered.fg_stroke.color = egui::Color32::BLACK;
            style.visuals.widgets.active.fg_stroke.color = egui::Color32::BLACK;
            style.visuals.widgets.open.fg_stroke.color = fg;

            let border = egui::Color32::from_gray(180);
            let border_focus = egui::Color32::from_gray(120);
            style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, border);
            style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, border_focus);
            style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, border_focus);
        }

        ctx.set_style(style);
        self.icon_cache = Some(IconCache::load(ctx));

        let zoom = self.state.persistent.layout.zoom_factor;
        if zoom > 0.0 && zoom != 1.0 {
            ctx.set_zoom_factor(zoom);
        }

        #[cfg(target_os = "macos")]
        if !self.use_native_menu {
            crate::macos_menu::set_titlebar_transparent();
        }

        #[cfg(target_os = "macos")]
        crate::macos_menu::set_tabbing_identifier("com.conch.terminal");

        self.style_applied = true;
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Scan for plugins in the native config dir and the legacy `~/.config/conch/` dir.
/// Deduplicates by filename so a plugin present in both locations is only listed once.
fn scan_plugin_dirs() -> Vec<PluginMeta> {
    let mut plugins = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    // Primary: native config dir (~/Library/Application Support/conch/ on macOS)
    let native_dir = config::config_dir().join("plugins");
    if let Ok(found) = discover_plugins(&native_dir) {
        for p in found {
            let key = p.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            seen_names.insert(key);
            plugins.push(p);
        }
    }

    // Legacy: ~/.config/conch/plugins/ (shared with Java version)
    if let Some(home) = std::env::var_os("HOME") {
        let legacy_dir = PathBuf::from(home).join(".config/conch/plugins");
        if legacy_dir != native_dir {
            if let Ok(found) = discover_plugins(&legacy_dir) {
                for p in found {
                    let key = p.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    if !seen_names.contains(&key) {
                        seen_names.insert(key);
                        plugins.push(p);
                    }
                }
            }
        }
    }

    plugins
}

fn is_dialog_command(cmd: &PluginCommand) -> bool {
    matches!(
        cmd,
        PluginCommand::ShowForm { .. }
            | PluginCommand::ShowPrompt { .. }
            | PluginCommand::ShowConfirm { .. }
            | PluginCommand::ShowAlert { .. }
            | PluginCommand::ShowError { .. }
            | PluginCommand::ShowText { .. }
            | PluginCommand::ShowTable { .. }
    )
}

/// Build an `alacritty_terminal::term::Config` from the user's cursor settings.
fn build_term_config(cfg: &config::CursorConfig) -> alacritty_terminal::term::Config {
    use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};

    fn parse_style(s: &config::CursorStyleConfig) -> CursorStyle {
        let shape = match s.shape.to_lowercase().as_str() {
            "underline" => CursorShape::Underline,
            "beam" | "ibeam" => CursorShape::Beam,
            _ => CursorShape::Block,
        };
        CursorStyle { shape, blinking: s.blinking }
    }

    let mut tc = alacritty_terminal::term::Config::default();
    tc.default_cursor_style = parse_style(&cfg.style);
    tc.vi_mode_cursor_style = cfg.vi_mode_style.as_ref().map(parse_style);
    tc
}

/// Create a local terminal session without inserting it into any state.
///
/// If `working_directory` is `Some`, the new shell starts in that directory.
/// Otherwise it falls back to the user's home directory.
pub(crate) fn create_local_session(
    user_config: &config::UserConfig,
    working_directory: Option<std::path::PathBuf>,
) -> Option<(Uuid, Session)> {
    let id = Uuid::new_v4();
    let shell_cfg = &user_config.terminal.shell;
    let shell = if shell_cfg.program.is_empty() {
        None
    } else {
        Some(alacritty_terminal::tty::Shell::new(
            shell_cfg.program.clone(),
            shell_cfg.args.clone(),
        ))
    };
    let term_config = build_term_config(&user_config.terminal.cursor);
    match LocalSession::new(
        DEFAULT_COLS, DEFAULT_ROWS, 8, 16,
        shell, &user_config.terminal.env, term_config,
        working_directory,
    ) {
        Ok(mut local) => {
            let event_rx = local.take_event_rx();
            let session = Session {
                id,
                title: "Local".into(),
                custom_title: None,
                backend: SessionBackend::Local(local),
                event_rx,
            };
            Some((id, session))
        }
        Err(e) => {
            log::error!("Failed to open local terminal: {e}");
            None
        }
    }
}

fn open_local_terminal(
    state: &mut AppState,
    last_cols: u16,
    last_rows: u16,
    cell_width: f32,
    cell_height: f32,
) -> Option<(Uuid, u32)> {
    // Inherit CWD from the active session's child process.
    let cwd = state
        .active_session()
        .and_then(|s| s.backend.child_pid())
        .and_then(conch_session::get_cwd_of_pid);
    let (id, session) = create_local_session(&state.user_config, cwd)?;
    let child_pid = match &session.backend {
        SessionBackend::Local(local) => local.child_pid(),
        _ => 0,
    };
    // Resize the new session to match the current window dimensions
    // so it doesn't start at the default size.
    if last_cols > 0 && last_rows > 0 {
        session.backend.resize(last_cols, last_rows, cell_width as u16, cell_height as u16);
    }
    state.sessions.insert(id, session);
    state.tab_order.push(id);
    state.active_tab = Some(id);
    Some((id, child_pid))
}

/// Read directory entries for the file browser sidebar.
/// Walk a path of folder names and return a mutable reference to the target folder.
fn find_folder_mut<'a>(
    folders: &'a mut Vec<conch_core::models::ServerFolder>,
    path: &[String],
) -> Option<&'a mut conch_core::models::ServerFolder> {
    if path.is_empty() {
        return None;
    }
    let first = &path[0];
    let rest = &path[1..];
    let folder = folders.iter_mut().find(|f| &f.name == first)?;
    if rest.is_empty() {
        Some(folder)
    } else {
        find_folder_mut(&mut folder.subfolders, rest)
    }
}

/// Remove a folder identified by its name path from the tree.
fn delete_folder(folders: &mut Vec<conch_core::models::ServerFolder>, path: &[String]) {
    if path.is_empty() {
        return;
    }
    if path.len() == 1 {
        folders.retain(|f| f.name != path[0]);
    } else if let Some(parent) = find_folder_mut(folders, &path[..path.len() - 1]) {
        let target = &path[path.len() - 1];
        parent.subfolders.retain(|f| &f.name != target);
    }
}

/// Find a mutable reference to a server entry by its address.
fn find_server_mut<'a>(
    folders: &'a mut Vec<conch_core::models::ServerFolder>,
    addr: &ServerAddress,
) -> Option<&'a mut conch_core::models::ServerEntry> {
    let folder = find_folder_mut(folders, &addr.folder_path)?;
    folder.servers.get_mut(addr.index)
}

/// Remove a server entry by its address.
fn delete_server(
    folders: &mut Vec<conch_core::models::ServerFolder>,
    addr: &ServerAddress,
) {
    if let Some(folder) = find_folder_mut(folders, &addr.folder_path) {
        if addr.index < folder.servers.len() {
            folder.servers.remove(addr.index);
        }
    }
}

fn load_local_entries(path: &std::path::Path) -> Vec<FileListEntry> {
    let Ok(read_dir) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut entries: Vec<FileListEntry> = read_dir
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            Some(FileListEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

/// Render the "Connecting to..." screen with a bouncing progress indicator.
fn show_connecting_screen(ui: &mut egui::Ui, info: &PendingSshInfo) {
    let rect = ui.available_rect_before_wrap();

    // Light background fill.
    let bg = if ui.visuals().dark_mode {
        egui::Color32::from_gray(30)
    } else {
        egui::Color32::from_gray(241)
    };
    ui.painter().rect_filled(rect, 0.0, bg);

    let center = rect.center();

    // "Connecting to <label>..."
    let heading = format!("Connecting to {}\u{2026}", info.label);
    let heading_galley = ui.painter().layout_no_wrap(
        heading,
        egui::FontId::new(28.0, egui::FontFamily::Proportional),
        if ui.visuals().dark_mode {
            egui::Color32::WHITE
        } else {
            egui::Color32::BLACK
        },
    );
    let heading_pos = egui::Pos2::new(
        center.x - heading_galley.size().x / 2.0,
        center.y - 40.0,
    );
    ui.painter().galley(heading_pos, heading_galley, egui::Color32::PLACEHOLDER);

    // Detail line: "user@host:port"
    let detail_galley = ui.painter().layout_no_wrap(
        info.detail.clone(),
        egui::FontId::new(16.0, egui::FontFamily::Proportional),
        if ui.visuals().dark_mode {
            egui::Color32::from_gray(200)
        } else {
            egui::Color32::from_gray(40)
        },
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

    // Track background.
    let track_color = if ui.visuals().dark_mode {
        egui::Color32::from_gray(60)
    } else {
        egui::Color32::from_gray(210)
    };
    ui.painter().rect_filled(bar_rect, bar_h / 2.0, track_color);

    // Bouncing indicator.
    let elapsed = info.started.elapsed().as_secs_f32();
    let cycle = 1.8; // seconds for a full bounce cycle
    let t = (elapsed % cycle) / cycle; // 0..1
    // Ping-pong: 0→1→0
    let pos_t = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
    // Ease in-out for smooth motion.
    let eased = pos_t * pos_t * (3.0 - 2.0 * pos_t);
    let indicator_w = bar_w * 0.15;
    let indicator_x = bar_rect.min.x + eased * (bar_w - indicator_w);
    let indicator_rect = egui::Rect::from_min_size(
        egui::Pos2::new(indicator_x, bar_y),
        egui::Vec2::new(indicator_w, bar_h),
    );
    let accent = egui::Color32::from_rgb(66, 133, 244); // Google blue
    ui.painter().rect_filled(indicator_rect, bar_h / 2.0, accent);
}

/// Send a cancel/close response for a plugin dialog so the plugin coroutine doesn't hang.
fn send_plugin_dialog_cancel(dialog: &ActivePluginDialog) {
    match dialog {
        ActivePluginDialog::Form { resp_tx, .. } => {
            let _ = resp_tx.send(PluginResponse::FormResult(None));
        }
        ActivePluginDialog::Prompt { resp_tx, .. } => {
            let _ = resp_tx.send(PluginResponse::Ok);
        }
        ActivePluginDialog::Confirm { resp_tx, .. } => {
            let _ = resp_tx.send(PluginResponse::Bool(false));
        }
        ActivePluginDialog::Alert { resp_tx, .. }
        | ActivePluginDialog::Error { resp_tx, .. }
        | ActivePluginDialog::Text { resp_tx, .. }
        | ActivePluginDialog::Table { resp_tx, .. } => {
            let _ = resp_tx.send(PluginResponse::Ok);
        }
    }
}
