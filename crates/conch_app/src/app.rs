//! Main application logic for the Conch terminal emulator.
//!
//! Implements `eframe::App` and orchestrates terminal sessions, input handling,
//! SSH connections, and UI panel layout.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use alacritty_terminal::event::Event as TermEvent;
use conch_core::{config, ssh_config};
use conch_session::{SftpCmd, SftpEvent, SshSession, TunnelManager, run_sftp_worker};
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::extra_window::ExtraWindow;
use crate::icons::{Icon, IconCache};
use crate::input::ResolvedShortcuts;
use crate::mouse::{Selection, handle_terminal_mouse};
use crate::plugins::scan_plugin_dirs;
use crate::sessions::{create_local_session, load_local_entries, open_local_terminal};
use crate::ssh::{self, show_connecting_screen};
use crate::state::{AppState, Session, SessionBackend};
use crate::terminal::widget::{get_selected_text, measure_cell_size, show_terminal};
use conch_plugin::{PluginCommand, PluginMeta, PluginResponse};
use crate::ui::dialogs::new_connection::{self, DialogAction, NewConnectionForm};
use crate::ui::dialogs::plugin_dialog::{self, ActivePluginDialog};
use crate::ui::dialogs::preferences::{self, PreferencesAction, PreferencesForm};
use crate::ui::dialogs::tunnels::{self, TunnelManagerAction, TunnelManagerState};
use crate::ui::session_panel::{self, SessionPanelAction, SessionPanelState};
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

/// Result from an async SSH connection attempt.
pub(crate) enum SshConnectOutcome {
    Connected(SshSession),
    NeedsPassword(conch_session::SshConnectResult),
    WrongPassword(conch_session::SshConnectResult),
    Failed(String),
}

/// Receives the result of an async SSH connection attempt.
pub(crate) struct PendingSsh {
    pub(crate) id: Uuid,
    pub(crate) rx: std::sync::mpsc::Receiver<SshConnectOutcome>,
}

/// Display info for a pending SSH connection (shown in the connecting tab).
pub(crate) struct PendingSshInfo {
    /// Short name for the tab title and heading, e.g. "dustin-vm"
    pub(crate) label: String,
    /// Detail line, e.g. "dustin@lab.nexxuscraft.com:22"
    pub(crate) detail: String,
    /// When the connection was initiated (for the bouncing progress bar).
    pub(crate) started: Instant,
    /// If the connection failed, the error message to display.
    pub(crate) error: Option<String>,
    /// True when waiting for user to enter password.
    pub(crate) needs_password: bool,
    /// Password entry buffer for the password prompt.
    pub(crate) password_buf: String,
    /// Focus the password field on next frame.
    pub(crate) password_focus: bool,
    /// Pending auth state held while waiting for password input.
    pub(crate) pending_auth: Option<conch_session::SshConnectResult>,
}

/// A resolved plugin keybinding ready for matching.
pub(crate) struct ResolvedPluginKeybind {
    /// The parsed key binding.
    pub(crate) binding: crate::input::KeyBinding,
    /// Index into `discovered_plugins`.
    pub(crate) plugin_idx: usize,
    /// Action name (e.g. "open_panel", "run", or a custom name).
    pub(crate) action: String,
}

/// A plugin currently executing on a tokio task.
pub(crate) struct RunningPlugin {
    pub(crate) meta: PluginMeta,
    /// For panel plugins, the index into `discovered_plugins`.
    pub(crate) discovered_idx: Option<usize>,
    pub(crate) commands_rx: tokio::sync::mpsc::UnboundedReceiver<(PluginCommand, tokio::sync::mpsc::UnboundedSender<PluginResponse>)>,
    /// Queued dialog requests waiting to be shown (FIFO).
    pub(crate) pending_dialogs: Vec<(PluginCommand, tokio::sync::mpsc::UnboundedSender<PluginResponse>)>,
}

/// The top-level eframe application.
pub struct ConchApp {
    pub(crate) state: AppState,
    pub(crate) rt: Arc<Runtime>,
    pub(crate) shortcuts: ResolvedShortcuts,

    // Terminal rendering
    pub(crate) cell_width: f32,
    pub(crate) cell_height: f32,
    pub(crate) cell_size_measured: bool,
    pub(crate) last_pixels_per_point: f32,

    // Cursor blink
    pub(crate) cursor_visible: bool,
    pub(crate) last_blink: Instant,

    // Resize tracking (only send resize when dimensions actually change)
    pub(crate) last_cols: u16,
    pub(crate) last_rows: u16,

    // Mouse selection
    pub(crate) selection: Selection,

    // Async SSH connection results
    pub(crate) pending_ssh_connections: Vec<PendingSsh>,
    pub(crate) pending_ssh_info: HashMap<Uuid, PendingSshInfo>,

    // SFTP worker state (auto-spawned on SSH connect, drives sidebar remote pane)
    pub(crate) sftp_cmd_tx: Option<tokio::sync::mpsc::UnboundedSender<SftpCmd>>,
    pub(crate) sftp_result_rx: Option<std::sync::mpsc::Receiver<SftpEvent>>,
    pub(crate) sftp_session_id: Option<Uuid>,
    pub(crate) remote_home: Option<PathBuf>,
    pub(crate) last_active_tab: Option<Uuid>,
    pub(crate) transfers: Vec<sidebar::TransferStatus>,

    // Icons
    pub(crate) icon_cache: Option<IconCache>,

    // Session panel UI state (inline rename, new-folder input)
    pub(crate) session_panel_state: SessionPanelState,

    // Transient UI state
    pub(crate) use_native_menu: bool,
    /// The right sidebar was opened temporarily for quick connect (Cmd+/).
    pub(crate) quick_connect_opened_sidebar: bool,
    /// The left sidebar was opened temporarily for plugin search (Cmd+Shift+P).
    pub(crate) plugin_search_opened_sidebar: bool,
    pub(crate) plugin_search_query: String,
    pub(crate) plugin_search_focus: bool,
    pub(crate) show_about: bool,
    pub(crate) quit_requested: bool,
    pub(crate) style_applied: bool,
    pub(crate) preferences_form: Option<PreferencesForm>,

    // Tunnel management
    pub(crate) tunnel_manager: TunnelManager,
    pub(crate) tunnel_dialog: Option<TunnelManagerState>,
    /// IDs of currently active tunnels (refreshed each frame from TunnelManager).
    pub(crate) tunnel_active_ids: Vec<Uuid>,
    /// Receives results from async tunnel activation attempts.
    pub(crate) pending_tunnel_results: Vec<(Uuid, std::sync::mpsc::Receiver<Result<(), String>>)>,

    // Tab rename dialog
    pub(crate) rename_tab_id: Option<Uuid>,
    pub(crate) rename_tab_buf: String,
    pub(crate) rename_tab_focus: bool,

    // Window focus tracking (for FOCUS_IN_OUT terminal mode)
    pub(crate) window_focused: bool,

    // Extra windows (multi-window via egui viewports)
    pub(crate) extra_windows: Vec<ExtraWindow>,
    pub(crate) next_viewport_num: u32,
    /// Index of the focused extra window, or `None` if the main window is focused.
    pub(crate) focused_extra_window: Option<usize>,

    // Plugin engine
    pub(crate) discovered_plugins: Vec<PluginMeta>,
    pub(crate) running_plugins: Vec<RunningPlugin>,
    pub(crate) plugin_output_lines: Vec<String>,
    pub(crate) active_plugin_dialog: Option<ActivePluginDialog>,
    pub(crate) plugin_progress: Option<String>,
    pub(crate) pending_clipboard: Option<String>,
    pub(crate) selected_plugin: Option<usize>,

    // Panel plugins
    /// Widget lists for active panel plugins, keyed by discovered_plugins index.
    pub(crate) panel_widgets: std::collections::HashMap<usize, Vec<conch_plugin::PanelWidget>>,
    /// Panel plugin names, keyed by discovered_plugins index.
    pub(crate) panel_names: std::collections::HashMap<usize, String>,
    /// Pending button click events for panel plugins, keyed by plugin index.
    pub(crate) panel_button_events: std::collections::HashMap<usize, Vec<String>>,
    /// Response senders waiting for panel events, keyed by plugin index.
    pub(crate) panel_event_waiters: std::collections::HashMap<usize, tokio::sync::mpsc::UnboundedSender<conch_plugin::PluginResponse>>,
    /// Checkbox states in the plugin list (mirrors loaded state, user toggles before applying).
    pub(crate) pending_plugin_loads: Vec<bool>,
    /// Resolved plugin keybindings (checked after app shortcuts).
    pub(crate) plugin_keybinds: Vec<ResolvedPluginKeybind>,
    /// Loaded plugin icon textures, keyed by discovered_plugins index.
    pub(crate) plugin_icons: HashMap<usize, egui::TextureHandle>,
    /// Pending icon loads from plugins (path validated but texture not yet created — needs egui Context).
    pub(crate) pending_plugin_icons: Vec<(usize, Vec<u8>)>,

    // Notifications
    pub(crate) notifications: crate::notifications::NotificationManager,

    // IPC socket listener
    pub(crate) ipc_listener: Option<crate::ipc::IpcListener>,
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
            let loaded = &state.persistent.loaded_plugins;
            let plugins: Vec<(usize, String)> = discovered_plugins
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.plugin_type == conch_plugin::PluginType::Action && {
                        let filename = p.path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        loaded.contains(&filename)
                    }
                })
                .map(|(i, p)| (i, p.name.clone()))
                .collect();
            crate::macos_menu::setup_menu_bar(&plugins);
        }

        let mut app = Self {
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
            panel_widgets: std::collections::HashMap::new(),
            panel_names: std::collections::HashMap::new(),
            panel_button_events: std::collections::HashMap::new(),
            panel_event_waiters: std::collections::HashMap::new(),
            pending_plugin_loads: Vec::new(),
            plugin_keybinds: Vec::new(),
            plugin_icons: HashMap::new(),
            pending_plugin_icons: Vec::new(),
            notifications: crate::notifications::NotificationManager::new(),
            ipc_listener: crate::ipc::IpcListener::start(),
        };

        // Activate panel plugins that were loaded in the previous session.
        app.activate_loaded_panel_plugins();

        // Resolve plugin keybindings.
        app.resolve_plugin_keybinds();

        app
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

        // Quit when the last session exits (and no SSH connections are pending or failed).
        if self.state.sessions.is_empty()
            && self.pending_ssh_connections.is_empty()
            && self.pending_ssh_info.is_empty()
        {
            self.quit_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Poll pending SSH connections.
        let mut completed: Vec<(usize, Uuid, SshConnectOutcome)> = Vec::new();
        for (i, pending) in self.pending_ssh_connections.iter().enumerate() {
            match pending.rx.try_recv() {
                Ok(outcome) => completed.push((i, pending.id, outcome)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    log::error!("SSH connection channel dropped");
                    completed.push((i, pending.id, SshConnectOutcome::Failed("Connection channel dropped".into())));
                }
            }
        }
        // Remove in reverse order to keep indices valid.
        for (i, id, outcome) in completed.into_iter().rev() {
            self.pending_ssh_connections.remove(i);
            match outcome {
                SshConnectOutcome::Connected(mut ssh_session) => {
                self.pending_ssh_info.remove(&id);
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
                }
                SshConnectOutcome::NeedsPassword(pending_result) => {
                    // Server is reachable but needs a password — show password prompt.
                    if let Some(info) = self.pending_ssh_info.get_mut(&id) {
                        info.needs_password = true;
                        info.password_focus = true;
                        info.pending_auth = Some(pending_result);
                    }
                }
                SshConnectOutcome::WrongPassword(pending_result) => {
                    // Wrong password — prompt again.
                    if let Some(info) = self.pending_ssh_info.get_mut(&id) {
                        info.needs_password = true;
                        info.password_focus = true;
                        info.password_buf.clear();
                        info.error = Some("Incorrect password.".into());
                        info.pending_auth = Some(pending_result);
                    }
                }
                SshConnectOutcome::Failed(err) => {
                    log::error!("SSH connection failed: {err}");
                    // Connection failed — keep the tab but show the error.
                    if let Some(info) = self.pending_ssh_info.get_mut(&id) {
                        info.error = Some(err);
                    }
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

        // Flush any pending plugin icon textures.
        if !self.pending_plugin_icons.is_empty() {
            self.flush_pending_icons(ctx);
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
        // On Linux/Windows, Ctrl is the "command" modifier in egui, so Ctrl+C
        // becomes Event::Copy instead of Event::Key. For a terminal emulator
        // we need Ctrl+C to send SIGINT (0x03) to the PTY. We track it here
        // and send the control character later when forward_to_pty is known.
        let mut ctrl_c_for_pty = false;
        let mut ctrl_x_for_pty = false;
        ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Copy | egui::Event::Cut => {
                        if cfg!(target_os = "macos") {
                            // On macOS, Cmd+C/X is copy/cut (Ctrl+C goes via Event::Key)
                            copy_requested = true;
                        } else {
                            // On Linux/Windows, this is really Ctrl+C/X — send to PTY
                            match event {
                                egui::Event::Copy => ctrl_c_for_pty = true,
                                egui::Event::Cut => ctrl_x_for_pty = true,
                                _ => {}
                            }
                        }
                    }
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
                    self.save_or_update_server(entry, folder_index);
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
                    self.save_or_update_server(entry, folder_index);
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
                DialogAction::Cancel => {
                    self.state.editing_server_addr = None;
                }
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
                            // Show loaded action (non-panel) plugins
                            let loaded_actions: Vec<(usize, String)> = self
                                .discovered_plugins
                                .iter()
                                .enumerate()
                                .filter(|(_, meta)| {
                                    meta.plugin_type == conch_plugin::PluginType::Action && {
                                        let filename = meta.path.file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned();
                                        self.state.persistent.loaded_plugins.contains(&filename)
                                    }
                                })
                                .map(|(i, meta)| (i, meta.name.clone()))
                                .collect();
                            if !loaded_actions.is_empty() {
                                ui.separator();
                                let mut run_idx = None;
                                for (i, name) in &loaded_actions {
                                    if ui.button(name).clicked() {
                                        run_idx = Some(*i);
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
            let loaded_plugins = &self.state.persistent.loaded_plugins;
            let plugin_display: Vec<sidebar::PluginDisplayInfo> = self
                .discovered_plugins
                .iter()
                .map(|meta| {
                    let filename = meta.path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let is_loaded = loaded_plugins.contains(&filename);
                    sidebar::PluginDisplayInfo {
                        name: meta.name.clone(),
                        description: meta.description.clone(),
                        is_panel: meta.plugin_type == conch_plugin::PluginType::Panel,
                        is_loaded,
                    }
                })
                .collect();
            let panel_tabs: Vec<(usize, String)> = self
                .panel_names
                .iter()
                .map(|(idx, name)| (*idx, name.clone()))
                .collect();
            let action = if self.state.show_left_sidebar {
                sidebar::show_tab_strip(ctx, &mut self.state.sidebar_tab, icons, &panel_tabs, &self.plugin_icons);
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
                    &self.panel_widgets,
                    &self.panel_names,
                    &mut self.pending_plugin_loads,
                )
            } else {
                SidebarAction::None
            };
            let deferred = match &action {
                SidebarAction::RunPlugin(i) => Some(SidebarAction::RunPlugin(*i)),
                SidebarAction::RefreshPlugins => Some(SidebarAction::RefreshPlugins),
                SidebarAction::ApplyPluginChanges(indices) => {
                    Some(SidebarAction::ApplyPluginChanges(indices.clone()))
                }
                SidebarAction::PanelButtonClick { plugin_idx, button_id } => {
                    Some(SidebarAction::PanelButtonClick { plugin_idx: *plugin_idx, button_id: button_id.clone() })
                }
                SidebarAction::DeactivatePanel(i) => Some(SidebarAction::DeactivatePanel(*i)),
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
                        if info.error.is_some() {
                            Some(format!("{} (failed)", info.label))
                        } else {
                            Some(format!("{}...", info.label))
                        }
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
                    } else if let Some(info) = self.pending_ssh_info.get_mut(&id) {
                        // Connecting, password prompt, or error screen.
                        let is_connecting = info.error.is_none() && !info.needs_password;
                        match show_connecting_screen(ui, info) {
                            ssh::ConnectingScreenAction::Close => {
                                self.pending_ssh_info.remove(&id);
                                self.state.tab_order.retain(|&tab_id| tab_id != id);
                                if self.state.active_tab == Some(id) {
                                    self.state.active_tab = self.state.tab_order.last().copied();
                                }
                            }
                            ssh::ConnectingScreenAction::SubmitPassword(password) => {
                                if let Some(info) = self.pending_ssh_info.get_mut(&id) {
                                    if let Some(pending_result) = info.pending_auth.take() {
                                        // Take ownership and spawn async password auth.
                                        info.needs_password = false;
                                        info.started = Instant::now();
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        self.pending_ssh_connections.push(PendingSsh { id, rx });
                                        self.rt.spawn(async move {
                                            match pending_result {
                                                conch_session::SshConnectResult::NeedsPassword {
                                                    pending_auth, term, event_proxy, event_rx, term_config,
                                                } => {
                                                    match SshSession::try_password(
                                                        pending_auth, &password, term, event_proxy, event_rx, term_config,
                                                    ).await {
                                                        Ok(conch_session::SshPasswordResult::Connected(session)) => {
                                                            let _ = tx.send(SshConnectOutcome::Connected(session));
                                                        }
                                                        Ok(conch_session::SshPasswordResult::WrongPassword(pending)) => {
                                                            let _ = tx.send(SshConnectOutcome::WrongPassword(pending));
                                                        }
                                                        Err(e) => {
                                                            let _ = tx.send(SshConnectOutcome::Failed(format!("{e:#}")));
                                                        }
                                                    }
                                                }
                                                conch_session::SshConnectResult::Connected(_) => unreachable!(),
                                            }
                                        });
                                    }
                                }
                            }
                            ssh::ConnectingScreenAction::None => {}
                        }
                        if is_connecting {
                            ctx.request_repaint();
                        }
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

        // Drag-and-drop files → paste paths into the terminal.
        if forward_to_pty {
            let dropped = ctx.input(|i| i.raw.dropped_files.clone());
            if !dropped.is_empty() {
                if let Some(session) = self.state.active_session() {
                    let paths: Vec<String> = dropped
                        .iter()
                        .filter_map(|f| f.path.as_ref())
                        .map(|p| {
                            let s = p.to_string_lossy().into_owned();
                            // Quote paths containing spaces.
                            if s.contains(' ') {
                                format!("'{s}'")
                            } else {
                                s
                            }
                        })
                        .collect();
                    if !paths.is_empty() {
                        let text = paths.join(" ");
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
        }

        // On Linux/Windows, forward Ctrl+C (0x03) and Ctrl+X (0x18) to the PTY.
        // These were captured as Event::Copy/Cut by egui since Ctrl is the
        // "command" modifier on non-macOS platforms.
        if forward_to_pty {
            if ctrl_c_for_pty {
                if let Some(session) = self.state.active_session() {
                    session.backend.write(&[0x03]); // ETX — Ctrl+C
                }
            }
            if ctrl_x_for_pty {
                if let Some(session) = self.state.active_session() {
                    session.backend.write(&[0x18]); // CAN — Ctrl+X
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
                SidebarAction::RefreshPlugins => self.refresh_plugins(),
                SidebarAction::ApplyPluginChanges(loaded_indices) => {
                    self.apply_plugin_changes(loaded_indices);
                }
                SidebarAction::PanelButtonClick { plugin_idx, button_id } => {
                    self.send_panel_button_event(plugin_idx, button_id);
                }
                SidebarAction::DeactivatePanel(idx) => {
                    self.deactivate_panel_plugin(idx);
                }
                _ => {}
            }
        }

        // Render notification toasts (overlay, on top of everything).
        if let Some(plugin_idx) = self.notifications.show(ctx) {
            // User clicked a notification linked to a panel plugin — navigate to it.
            if !self.state.show_left_sidebar {
                self.state.show_left_sidebar = true;
                self.state.persistent.layout.left_panel_collapsed = false;
                let _ = config::save_persistent_state(&self.state.persistent);
            }
            self.state.sidebar_tab = sidebar::SidebarTab::PluginPanel(plugin_idx);
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
    /// Push an internal notification (not from a plugin).
    #[allow(dead_code)]
    pub(crate) fn notify(&mut self, body: impl Into<String>, level: conch_plugin::NotificationLevel) {
        self.notifications.push(crate::notifications::Notification::simple(
            body.into(),
            None,
            level,
            None,
            None,
        ));
    }

    /// Push an internal notification with a title.
    #[allow(dead_code)]
    pub(crate) fn notify_with_title(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        level: conch_plugin::NotificationLevel,
    ) {
        self.notifications.push(crate::notifications::Notification::simple(
            body.into(),
            Some(title.into()),
            level,
            None,
            None,
        ));
    }

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

        // Register the user's configured terminal font with egui's Monospace family.
        let font_family_name = &self.state.user_config.font.normal.family;
        if !font_family_name.is_empty() {
            if let Some(font_data) = load_system_font(font_family_name) {
                let mut fonts = egui::FontDefinitions::default();
                let key = font_family_name.clone();
                fonts
                    .font_data
                    .insert(key.clone(), egui::FontData::from_owned(font_data).into());
                // Put the user font first so it takes priority for Monospace glyphs.
                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .insert(0, key);
                ctx.set_fonts(fonts);
            } else {
                log::warn!("Could not find font '{}' on this system", font_family_name);
            }
        }

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

/// Try to load a font by family name from the system using font-kit.
/// Returns the raw font file bytes on success.
fn load_system_font(family: &str) -> Option<Vec<u8>> {
    use font_kit::family_name::FamilyName;
    use font_kit::properties::Properties;
    use font_kit::source::SystemSource;

    let source = SystemSource::new();
    let handle = source
        .select_best_match(
            &[FamilyName::Title(family.to_string())],
            &Properties::new(),
        )
        .ok()?;
    match handle {
        font_kit::handle::Handle::Path { path, .. } => std::fs::read(path).ok(),
        font_kit::handle::Handle::Memory { bytes, .. } => Some((*bytes).clone()),
    }
}

