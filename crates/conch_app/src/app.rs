//! Main application logic for the Conch terminal emulator.
//!
//! Implements `eframe::App` and orchestrates terminal sessions, input handling,
//! SSH connections, and UI panel layout.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::Event as TermEvent;
use conch_core::{config, ssh_config};
use conch_core::models::SavedTunnel;
use conch_session::shell_integration;
use conch_session::{LocalSession, SftpCmd, SftpListing, SshSession, TunnelManager, run_sftp_worker};
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::icons::{Icon, IconCache};
use crate::input::{self, ResolvedShortcuts};
use crate::state::{AppState, Session, SessionBackend};
use crate::terminal::widget::{get_selected_text, measure_cell_size, pixel_to_cell, show_terminal};
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
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

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
const CURSOR_BLINK_MS: u128 = 500;

/// Receives the result of an async SSH connection attempt.
struct PendingSsh {
    id: Uuid,
    rx: std::sync::mpsc::Receiver<Result<SshSession, String>>,
}

/// Tracks two-phase SSH shell integration injection.
struct SshInjectionState {
    connect_time: Instant,
    phase1_sent: bool,
}

/// Mouse text selection state for the active terminal.
#[derive(Default)]
struct Selection {
    /// Cell coordinate where the drag began.
    start: Option<(usize, usize)>,
    /// Cell coordinate where the drag currently ends.
    end: Option<(usize, usize)>,
    /// Whether a drag is in progress.
    active: bool,
}

impl Selection {
    /// Return the selection with start <= end in row-major order, or `None` if empty.
    fn normalized(&self) -> Option<((usize, usize), (usize, usize))> {
        let s = self.start?;
        let e = self.end?;
        if s == e {
            return None;
        }
        if (s.1, s.0) <= (e.1, e.0) {
            Some((s, e))
        } else {
            Some((e, s))
        }
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
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

    // SFTP worker state
    sftp_cmd_tx: Option<tokio::sync::mpsc::UnboundedSender<SftpCmd>>,
    sftp_result_rx: Option<std::sync::mpsc::Receiver<SftpListing>>,
    sftp_session_id: Option<Uuid>,
    remote_home: Option<PathBuf>,

    // CWD tracking per SSH session (OSC 7)
    ssh_cwd_receivers: HashMap<Uuid, std::sync::mpsc::Receiver<String>>,
    session_last_cwd: HashMap<Uuid, PathBuf>,
    last_active_tab: Option<Uuid>,

    // Two-phase SSH shell integration injection:
    // Phase 1 (400ms): send `stty -echo` to suppress echo
    // Phase 2 (600ms): send function definition + `stty echo` + cleanup escapes
    ssh_pending_injection: HashMap<Uuid, SshInjectionState>,

    // Local session CWD polling via macOS proc_pidinfo
    local_pids: HashMap<Uuid, u32>,
    local_last_cwd: HashMap<Uuid, PathBuf>,
    last_cwd_poll: Instant,

    // Icons
    icon_cache: Option<IconCache>,

    // Session panel UI state (inline rename, new-folder input)
    session_panel_state: SessionPanelState,

    // Transient UI state
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

    // Plugin engine
    discovered_plugins: Vec<PluginMeta>,
    running_plugins: Vec<RunningPlugin>,
    plugin_output_lines: Vec<String>,
    active_plugin_dialog: Option<ActivePluginDialog>,
    plugin_progress: Option<String>,
    pending_clipboard: Option<String>,
    selected_plugin: Option<usize>,
}

impl ConchApp {
    pub fn new(rt: Arc<Runtime>) -> Self {
        // Run legacy migration before loading.
        config::migrate_if_needed();

        let user_config = config::load_user_config().unwrap_or_default();
        let persistent = config::load_persistent_state().unwrap_or_default();
        let shortcuts = ResolvedShortcuts::from_config(&user_config.keyboard);

        let mut state = AppState::new(user_config, persistent);

        state.ssh_config_hosts = ssh_config::parse_ssh_config().unwrap_or_default();

        let initial_path = state.file_browser.local_path.clone();
        state.file_browser.local_entries = load_local_entries(&initial_path);

        let mut local_pids = HashMap::new();
        if let Some((id, pid)) = open_local_terminal(&mut state) {
            local_pids.insert(id, pid);
        }

        // Discover plugins — check both native config dir and legacy ~/.config/conch/
        let discovered_plugins = scan_plugin_dirs();

        // Set up native macOS menu bar.
        #[cfg(target_os = "macos")]
        {
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
            cursor_visible: true,
            last_blink: Instant::now(),
            last_cols: DEFAULT_COLS,
            last_rows: DEFAULT_ROWS,
            selection: Selection::default(),
            pending_ssh_connections: Vec::new(),
            sftp_cmd_tx: None,
            sftp_result_rx: None,
            sftp_session_id: None,
            remote_home: None,
            ssh_cwd_receivers: HashMap::new(),
            session_last_cwd: HashMap::new(),
            last_active_tab: None,
            ssh_pending_injection: HashMap::new(),
            local_pids,
            local_last_cwd: HashMap::new(),
            last_cwd_poll: Instant::now(),
            icon_cache: None,
            session_panel_state: SessionPanelState::default(),
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
            discovered_plugins,
            running_plugins: Vec::new(),
            plugin_output_lines: Vec::new(),
            active_plugin_dialog: None,
            plugin_progress: None,
            pending_clipboard: None,
            selected_plugin: None,
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

        // Auto-open a local terminal if all sessions closed.
        if self.state.sessions.is_empty() {
            self.open_local_tab();
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
            if let Some(mut ssh_session) = ssh_opt {
                // Spawn SFTP worker for this SSH session.
                let handle = Arc::clone(ssh_session.ssh_handle());
                // Shut down any existing SFTP worker.
                if let Some(tx) = self.sftp_cmd_tx.take() {
                    let _ = tx.send(SftpCmd::Shutdown);
                }
                let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
                let (result_tx, result_rx) = std::sync::mpsc::channel();
                self.rt.spawn(run_sftp_worker(handle, cmd_rx, result_tx));
                self.sftp_cmd_tx = Some(cmd_tx);
                self.sftp_result_rx = Some(result_rx);
                self.sftp_session_id = Some(id);

                // Take the CWD receiver for OSC 7 tracking.
                if let Some(cwd_rx) = ssh_session.take_cwd_rx() {
                    self.ssh_cwd_receivers.insert(id, cwd_rx);
                }

                // Schedule two-phase shell integration injection.
                self.ssh_pending_injection.insert(id, SshInjectionState {
                    connect_time: Instant::now(),
                    phase1_sent: false,
                });

                let event_rx = ssh_session.take_event_rx();
                let session = Session {
                    id,
                    title: "SSH".into(),
                    custom_title: None,
                    backend: SessionBackend::Ssh(ssh_session),
                    event_rx,
                };
                self.state.sessions.insert(id, session);
                self.state.tab_order.push(id);
                self.state.active_tab = Some(id);
            }
        }

        // Poll SFTP results.
        if let Some(rx) = &self.sftp_result_rx {
            while let Ok(listing) = rx.try_recv() {
                self.remote_home = Some(listing.home);
                let path_str = listing.path.to_string_lossy().into_owned();
                self.state.file_browser.remote_path = Some(listing.path);
                self.state.file_browser.remote_path_edit = path_str;
                self.state.file_browser.remote_entries =
                    listing.entries.into_iter().map(Into::into).collect();
            }
        }

        // Two-phase SSH shell integration injection.
        // Phase 1 (400ms): `stty -echo` to suppress terminal echo
        // Phase 2 (600ms): function definition + `stty echo` + printf cleanup
        const PHASE1_DELAY_MS: u128 = 400;
        const PHASE2_DELAY_MS: u128 = 600;
        let now = Instant::now();
        let mut phase1_ids = Vec::new();
        let mut phase2_ids = Vec::new();
        for (id, inj) in &self.ssh_pending_injection {
            let elapsed = now.duration_since(inj.connect_time).as_millis();
            if !inj.phase1_sent && elapsed >= PHASE1_DELAY_MS {
                phase1_ids.push(*id);
            } else if inj.phase1_sent && elapsed >= PHASE2_DELAY_MS {
                phase2_ids.push(*id);
            }
        }
        for id in phase1_ids {
            if let Some(session) = self.state.sessions.get(&id) {
                session.backend.write(b"stty -echo\n");
            }
            if let Some(inj) = self.ssh_pending_injection.get_mut(&id) {
                inj.phase1_sent = true;
            }
        }
        for id in phase2_ids {
            self.ssh_pending_injection.remove(&id);
            if let Some(session) = self.state.sessions.get(&id) {
                session.backend.write(
                    shell_integration::ssh_osc7_injection().as_bytes(),
                );
            }
        }

        // Poll local session CWD via macOS proc_pidinfo (~1 second interval).
        #[cfg(target_os = "macos")]
        {
            const CWD_POLL_INTERVAL_MS: u128 = 1000;
            if now.duration_since(self.last_cwd_poll).as_millis() >= CWD_POLL_INTERVAL_MS {
                self.last_cwd_poll = now;
                if let Some(id) = self.state.active_tab {
                    if let Some(&pid) = self.local_pids.get(&id) {
                        if let Some(cwd) = shell_integration::get_process_cwd(pid) {
                            let changed = self
                                .local_last_cwd
                                .get(&id)
                                .map_or(true, |prev| *prev != cwd);
                            if changed {
                                self.state.file_browser.local_entries =
                                    load_local_entries(&cwd);
                                self.state.file_browser.local_path_edit =
                                    cwd.to_string_lossy().into_owned();
                                self.state.file_browser.local_path = cwd.clone();
                                self.local_last_cwd.insert(id, cwd);
                            }
                        }
                    }
                }
            }
        }

        // Poll CWD updates from OSC 7 scanning for the active SSH session.
        if let Some(id) = self.state.active_tab {
            if let Some(rx) = self.ssh_cwd_receivers.get(&id) {
                let mut latest_cwd = None;
                while let Ok(cwd_str) = rx.try_recv() {
                    latest_cwd = Some(cwd_str);
                }
                if let Some(cwd_str) = latest_cwd {
                    let cwd = PathBuf::from(&cwd_str);
                    self.session_last_cwd.insert(id, cwd.clone());
                    if let Some(tx) = &self.sftp_cmd_tx {
                        let _ = tx.send(SftpCmd::List(cwd));
                    }
                }
            }
        }

        // Detect tab switches and update SFTP worker / remote pane accordingly.
        if self.state.active_tab != self.last_active_tab {
            self.last_active_tab = self.state.active_tab;
            if let Some(id) = self.state.active_tab {
                // Check if this is an SSH session (borrow ends before mutations below).
                let ssh_handle = self.state.sessions.get(&id).and_then(|s| match &s.backend {
                    SessionBackend::Ssh(ssh) => Some(Arc::clone(ssh.ssh_handle())),
                    _ => None,
                });
                if let Some(handle) = ssh_handle {
                    if self.sftp_session_id != Some(id) {
                        // Shut down old SFTP worker, start new one for this session.
                        if let Some(tx) = self.sftp_cmd_tx.take() {
                            let _ = tx.send(SftpCmd::Shutdown);
                        }
                        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
                        let (result_tx, result_rx) = std::sync::mpsc::channel();
                        self.rt.spawn(run_sftp_worker(handle, cmd_rx, result_tx));
                        if let Some(cwd) = self.session_last_cwd.get(&id) {
                            let _ = cmd_tx.send(SftpCmd::List(cwd.clone()));
                        }
                        self.sftp_cmd_tx = Some(cmd_tx);
                        self.sftp_result_rx = Some(result_rx);
                        self.sftp_session_id = Some(id);
                    }
                } else {
                    // Local session — clear remote pane.
                    self.state.file_browser.remote_path = None;
                    self.state.file_browser.remote_entries.clear();
                    self.state.file_browser.remote_path_edit.clear();
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
                }
            }
            PluginCommand::ShowPrompt { message } => ActivePluginDialog::Prompt {
                message,
                input: String::new(),
                resp_tx,
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

    /// Process keyboard events: app shortcuts always run, PTY forwarding
    /// only when `forward_to_pty` is true (i.e. no text widget has focus).
    fn handle_keyboard(&mut self, ctx: &egui::Context, forward_to_pty: bool) {
        ctx.input(|input| {
            for event in &input.events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
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
                        if let Some(ref kb) = self.shortcuts.new_tab {
                            if kb.matches(key, modifiers) { self.open_local_tab(); return; }
                        }
                        if let Some(ref kb) = self.shortcuts.close_tab {
                            if kb.matches(key, modifiers) {
                                if let Some(id) = self.state.active_tab {
                                    self.remove_session(id);
                                    if self.state.sessions.is_empty() {
                                        self.open_local_tab();
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
                                self.state.show_right_sidebar = true;
                                self.session_panel_state.quick_connect_focus = true;
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
                            if let Some(bytes) = input::key_to_bytes(key, modifiers, None, &self.shortcuts) {
                                if let Some(session) = self.state.active_session() {
                                    session.backend.write(&bytes);
                                }
                            }
                        }
                    }
                    egui::Event::Text(text) => {
                        if forward_to_pty {
                            if let Some(session) = self.state.active_session() {
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
    ) {
        let id = Uuid::new_v4();
        let (tx, rx) = std::sync::mpsc::channel();

        self.rt.spawn(async move {
            let params = conch_session::ConnectParams {
                host: host.clone(),
                port,
                user,
                identity_file: identity_file.map(std::path::PathBuf::from),
                password: None,
                proxy_command,
                proxy_jump,
            };
            let result = SshSession::connect(&params, DEFAULT_COLS, DEFAULT_ROWS)
                .await
                .map_err(|e| format!("{host}: {e}"));
            let _ = tx.send(result);
        });

        self.pending_ssh_connections.push(PendingSsh { id, rx });
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

        // Clean up CWD tracking for this session.
        self.ssh_cwd_receivers.remove(&id);
        self.session_last_cwd.remove(&id);
        self.ssh_pending_injection.remove(&id);
        self.local_pids.remove(&id);
        self.local_last_cwd.remove(&id);

        // Clean up SFTP worker if it belonged to this session.
        if self.sftp_session_id == Some(id) {
            if let Some(tx) = self.sftp_cmd_tx.take() {
                let _ = tx.send(SftpCmd::Shutdown);
            }
            self.sftp_result_rx = None;
            self.sftp_session_id = None;
            self.remote_home = None;
            self.state.file_browser.remote_path = None;
            self.state.file_browser.remote_entries.clear();
            self.state.file_browser.remote_path_edit.clear();
        }
    }

    /// Open a new local terminal tab and track its child PID for CWD polling.
    fn open_local_tab(&mut self) {
        if let Some((id, pid)) = open_local_terminal(&mut self.state) {
            self.local_pids.insert(id, pid);
        }
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
        collect_from_folders(&self.state.persistent.folders, &mut servers);
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
        // Apply custom style: sharp corners everywhere.
        if !self.style_applied {
            if let Some((_name, font_data)) =
                crate::fonts::load_system_ui_font(&self.state.user_config.font.ui_family)
            {
                let mut font_defs = egui::FontDefinitions::default();
                font_defs.font_data.insert(
                    "system_ui".to_owned(),
                    egui::FontData::from_owned(font_data).into(),
                );
                font_defs
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .insert(0, "system_ui".to_owned());
                ctx.set_fonts(font_defs);
            }

            ctx.set_visuals(egui::Visuals::dark());
            ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
            let mut style = (*ctx.style()).clone();
            style.visuals.window_corner_radius = egui::CornerRadius::ZERO;
            style.visuals.menu_corner_radius = egui::CornerRadius::ZERO;
            style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::ZERO;
            style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
            style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
            style.visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
            style.visuals.widgets.open.corner_radius = egui::CornerRadius::ZERO;
            ctx.set_style(style);
            self.icon_cache = Some(IconCache::load(ctx));
            self.style_applied = true;
        }

        // Measure cell size from the monospace font on the first frame.
        if !self.cell_size_measured {
            let (cw, ch) = measure_cell_size(ctx, self.state.user_config.font.size);
            if cw > 0.0 && ch > 0.0 {
                self.cell_width = cw;
                self.cell_height = ch;
                self.cell_size_measured = true;
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

        // Handle native macOS menu actions.
        #[cfg(target_os = "macos")]
        {
            for action in crate::macos_menu::drain_actions() {
                self.handle_macos_menu_action(action, ctx);
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
            let no_widget_focused = !ctx.memory(|m| m.focused().is_some());
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

        if self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // -- Dialogs (floating windows, rendered before panels) --

        if let Some(mut form) = self.state.new_connection_form.take() {
            match new_connection::show_new_connection(ctx, &mut form) {
                DialogAction::Connect {
                    host,
                    port,
                    user,
                    identity_file,
                } => self.start_ssh_connect(host, port, user, identity_file, None, None),
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
                        if ui.button("Save").clicked() {
                            save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
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
            let tunnels = &self.state.persistent.tunnels;
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
                &self.state.persistent.tunnels,
                &self.tunnel_active_ids,
                &all_servers,
            );
            match action {
                TunnelManagerAction::NewTunnel(tunnel) => {
                    self.activate_tunnel(&tunnel);
                    self.state.persistent.tunnels.push(tunnel);
                    let _ = config::save_persistent_state(&self.state.persistent);
                    self.tunnel_dialog = Some(dialog);
                }
                TunnelManagerAction::Activate(id) => {
                    if let Some(tunnel) = self.state.persistent.tunnels.iter().find(|t| t.id == id).cloned() {
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
                    self.state.persistent.tunnels.retain(|t| t.id != id);
                    let _ = config::save_persistent_state(&self.state.persistent);
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

        // Menu bar (File, Sessions, Tools, View, Help).
        // On macOS, menus are in the native menu bar. On other platforms, show in-window.
        #[cfg(not(target_os = "macos"))]
        egui::TopBottomPanel::top("menu_bar")
            .frame(egui::Frame::side_top_panel(ctx.style().as_ref())
                .inner_margin(egui::Margin { top: 4, bottom: 4, ..egui::Margin::same(8) }))
            .show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
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

                ui.menu_button("Help", |ui| {
                    if ui.button("About Conch").clicked() {
                        self.show_about = true;
                        ui.close_menu();
                    }
                });
            });
        });

        // Left sidebar: narrow tab strip + resizable content panel.
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
        let sidebar_action = if self.state.show_left_sidebar {
            sidebar::show_tab_strip(ctx, &mut self.state.sidebar_tab, icons);
            sidebar::show_sidebar_content(
                ctx,
                &self.state.sidebar_tab,
                &mut self.state.file_browser,
                icons,
                &plugin_display,
                &self.plugin_output_lines,
                &mut self.selected_plugin,
            )
        } else {
            SidebarAction::None
        };
        // Extract plugin-related actions before consuming sidebar_action in the match.
        let deferred_plugin_action = match &sidebar_action {
            SidebarAction::RunPlugin(i) => Some(SidebarAction::RunPlugin(*i)),
            SidebarAction::StopPlugin(i) => Some(SidebarAction::StopPlugin(*i)),
            SidebarAction::RefreshPlugins => Some(SidebarAction::RefreshPlugins),
            _ => None,
        };
        match sidebar_action {
            SidebarAction::NavigateLocal(path) => {
                let old = self.state.file_browser.local_path.clone();
                self.state.file_browser.local_back_stack.push(old);
                self.state.file_browser.local_forward_stack.clear();
                self.state.file_browser.local_entries = load_local_entries(&path);
                self.state.file_browser.local_path_edit = path.to_string_lossy().into_owned();
                self.state.file_browser.local_path = path;
            }
            SidebarAction::GoBackLocal => {
                if let Some(prev) = self.state.file_browser.local_back_stack.pop() {
                    let current = self.state.file_browser.local_path.clone();
                    self.state.file_browser.local_forward_stack.push(current);
                    self.state.file_browser.local_entries = load_local_entries(&prev);
                    self.state.file_browser.local_path_edit = prev.to_string_lossy().into_owned();
                    self.state.file_browser.local_path = prev;
                }
            }
            SidebarAction::GoForwardLocal => {
                if let Some(next) = self.state.file_browser.local_forward_stack.pop() {
                    let current = self.state.file_browser.local_path.clone();
                    self.state.file_browser.local_back_stack.push(current);
                    self.state.file_browser.local_entries = load_local_entries(&next);
                    self.state.file_browser.local_path_edit = next.to_string_lossy().into_owned();
                    self.state.file_browser.local_path = next;
                }
            }
            SidebarAction::RefreshLocal => {
                let path = self.state.file_browser.local_path.clone();
                self.state.file_browser.local_entries = load_local_entries(&path);
            }
            SidebarAction::GoHomeLocal => {
                let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
                let old = self.state.file_browser.local_path.clone();
                self.state.file_browser.local_back_stack.push(old);
                self.state.file_browser.local_forward_stack.clear();
                self.state.file_browser.local_entries = load_local_entries(&home);
                self.state.file_browser.local_path_edit = home.to_string_lossy().into_owned();
                self.state.file_browser.local_path = home;
            }
            SidebarAction::SelectFile(path) => {
                log::info!("File selected: {}", path.display());
            }
            SidebarAction::NavigateRemote(path) => {
                if let Some(tx) = &self.sftp_cmd_tx {
                    if let Some(old) = self.state.file_browser.remote_path.clone() {
                        self.state.file_browser.remote_back_stack.push(old);
                        self.state.file_browser.remote_forward_stack.clear();
                    }
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
                    if let Some(path) = self.state.file_browser.remote_path.clone() {
                        let _ = tx.send(SftpCmd::List(path));
                    }
                }
            }
            SidebarAction::GoHomeRemote => {
                if let Some(tx) = &self.sftp_cmd_tx {
                    if let Some(home) = self.remote_home.clone() {
                        if let Some(old) = self.state.file_browser.remote_path.clone() {
                            self.state.file_browser.remote_back_stack.push(old);
                            self.state.file_browser.remote_forward_stack.clear();
                        }
                        let _ = tx.send(SftpCmd::List(home));
                    }
                }
            }
            SidebarAction::RunPlugin(_)
            | SidebarAction::StopPlugin(_)
            | SidebarAction::RefreshPlugins
            | SidebarAction::None => {
                // Plugin actions are deferred; None is a no-op.
            }
        }

        // Right sidebar (session / server tree).
        let mut panel_action = SessionPanelAction::None;
        if self.state.show_right_sidebar {
            egui::SidePanel::right("right_sidebar")
                .resizable(true)
                .default_width(220.0)
                .min_width(100.0)
                .show(ctx, |ui| {
                    panel_action = session_panel::show_session_panel(
                        ui,
                        &self.state.persistent.folders,
                        &self.state.ssh_config_hosts,
                        icons,
                        &mut self.session_panel_state,
                    );
                });
        }
        match panel_action {
            SessionPanelAction::Connect(req) => {
                self.start_ssh_connect(
                    req.host,
                    req.port,
                    req.user,
                    req.identity_file,
                    req.proxy_command,
                    req.proxy_jump,
                );
            }
            SessionPanelAction::CreateFolder { parent_path, name } => {
                let folder = conch_core::models::ServerFolder::new(name);
                if parent_path.is_empty() {
                    self.state.persistent.folders.push(folder);
                } else {
                    if let Some(parent) = find_folder_mut(&mut self.state.persistent.folders, &parent_path) {
                        parent.subfolders.push(folder);
                    }
                }
                let _ = config::save_persistent_state(&self.state.persistent);
            }
            SessionPanelAction::RenameFolder { path, new_name } => {
                if let Some(f) = find_folder_mut(&mut self.state.persistent.folders, &path) {
                    f.name = new_name;
                }
                let _ = config::save_persistent_state(&self.state.persistent);
            }
            SessionPanelAction::DeleteFolder { path } => {
                delete_folder(&mut self.state.persistent.folders, &path);
                let _ = config::save_persistent_state(&self.state.persistent);
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
                    // Create a default folder if none exist, then add entry.
                    if self.state.persistent.folders.is_empty() {
                        self.state.persistent.folders.push(
                            conch_core::models::ServerFolder::new("Servers"),
                        );
                    }
                    self.state.persistent.folders[0].servers.push(entry);
                } else if let Some(f) = find_folder_mut(&mut self.state.persistent.folders, &folder_path) {
                    f.servers.push(entry);
                }
                let _ = config::save_persistent_state(&self.state.persistent);
            }
            SessionPanelAction::RenameServer { addr, new_name } => {
                if let Some(server) = find_server_mut(&mut self.state.persistent.folders, &addr) {
                    server.name = new_name;
                }
                let _ = config::save_persistent_state(&self.state.persistent);
            }
            SessionPanelAction::DeleteServer { addr } => {
                delete_server(&mut self.state.persistent.folders, &addr);
                let _ = config::save_persistent_state(&self.state.persistent);
            }
            SessionPanelAction::EditServer { .. } => {
                // Stub — will open edit dialog in a future change.
            }
            SessionPanelAction::None => {}
        }

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
                    if let Some(session) = self.state.sessions.get(&id) {
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
                        let title = session.custom_title.as_deref().unwrap_or(&session.title);
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

        // Central panel (active terminal).
        let font_size = self.state.user_config.font.size;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                if let Some(id) = self.state.active_tab {
                    if let Some(_session) = self.state.sessions.get(&id) {
                        let term = self.state.sessions.get(&id).unwrap().backend.term().clone();
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

                        self.handle_mouse_selection(&response, &size_info);

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
        let forward_to_pty = !ctx.memory(|m| m.focused().is_some());

        if let Some(text) = paste_text {
            if forward_to_pty {
                if let Some(session) = self.state.active_session() {
                    session.backend.write(text.as_bytes());
                }
            }
        }

        self.handle_keyboard(ctx, forward_to_pty);

        // Handle deferred plugin sidebar actions (after all borrows are released).
        if let Some(action) = deferred_plugin_action {
            match action {
                SidebarAction::RunPlugin(idx) => self.run_plugin_by_index(idx),
                SidebarAction::StopPlugin(idx) => {
                    if let Some(meta) = self.discovered_plugins.get(idx) {
                        let path = meta.path.clone();
                        if let Some(pos) = self.running_plugins.iter().position(|rp| rp.meta.path == path) {
                            self.stop_plugin(pos);
                        }
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
        ctx.request_repaint();
    }
}

impl ConchApp {
    #[cfg(target_os = "macos")]
    fn handle_macos_menu_action(
        &mut self,
        action: crate::macos_menu::MenuAction,
        ctx: &egui::Context,
    ) {
        use crate::macos_menu::MenuAction;
        match action {
            MenuAction::NewConnection | MenuAction::NewSshSession => {
                self.state.new_connection_form =
                    Some(NewConnectionForm::with_defaults());
            }
            MenuAction::Quit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
                    if ui.button("OK").clicked() {
                        self.show_about = false;
                    }
                });
            });
    }

    /// Update text selection state from mouse drag events.
    fn handle_mouse_selection(
        &mut self,
        response: &egui::Response,
        size_info: &crate::terminal::size_info::SizeInfo,
    ) {
        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                let cell = pixel_to_cell(pos, response.rect.min, size_info);
                self.selection.start = Some(cell);
                self.selection.end = Some(cell);
                self.selection.active = true;
            }
        }
        if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                self.selection.end = Some(pixel_to_cell(pos, response.rect.min, size_info));
            }
        }
        if response.drag_stopped() {
            self.selection.active = false;
        }
        if response.clicked() {
            self.selection.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Spawn a new local PTY session and add it to the app state.
/// Returns `(session_id, child_pid)` on success for CWD tracking.
/// Returns true if a `PluginCommand` is a blocking dialog that needs UI rendering.
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

fn open_local_terminal(state: &mut AppState) -> Option<(Uuid, u32)> {
    let id = Uuid::new_v4();
    match LocalSession::new(DEFAULT_COLS, DEFAULT_ROWS, 8, 16) {
        Ok(mut local) => {
            let child_pid = local.child_pid();
            let event_rx = local.take_event_rx();
            let session = Session {
                id,
                title: "Local".into(),
                custom_title: None,
                backend: SessionBackend::Local(local),
                event_rx,
            };
            state.sessions.insert(id, session);
            state.tab_order.push(id);
            state.active_tab = Some(id);
            Some((id, child_pid))
        }
        Err(e) => {
            log::error!("Failed to open local terminal: {e}");
            None
        }
    }
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
