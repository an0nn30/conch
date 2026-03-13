//! Main application struct and egui update loop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use conch_core::config;
use conch_plugin::bus::PluginBus;
use conch_plugin::native::manager::NativePluginManager;
use conch_plugin_sdk::PanelLocation;
use egui::ViewportCommand;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::extra_window::ExtraWindow;
use crate::host::bridge::{self, PanelRegistry, SessionRegistry};
use crate::host::dialogs::{self, DialogState};
use crate::host::plugin_manager_ui::PluginManagerState;
use crate::input::ResolvedShortcuts;
use crate::ipc::{IpcListener, IpcMessage};
use crate::menu_bar::MenuBarState;
use crate::mouse::Selection;
use crate::platform::PlatformCapabilities;
use crate::sessions::create_local_session;
use crate::state::AppState;
use crate::terminal::color::ResolvedColors;
use crate::terminal::widget::{self, TerminalFrameCache};
use crate::watcher::{FileChangeKind, FileWatcher};

/// Cursor blink interval in milliseconds.
const CURSOR_BLINK_MS: u128 = 500;

pub struct ConchApp {
    pub(crate) state: AppState,
    pub(crate) shortcuts: ResolvedShortcuts,
    pub(crate) selection: Selection,

    // Terminal rendering state.
    pub(crate) cell_width: f32,
    pub(crate) cell_height: f32,
    pub(crate) cell_size_measured: bool,
    pub(crate) last_pixels_per_point: f32,
    pub(crate) last_cols: u16,
    pub(crate) last_rows: u16,
    pub(crate) cursor_visible: bool,
    pub(crate) last_blink: Instant,
    pub(crate) terminal_frame_cache: TerminalFrameCache,

    // UI chrome.
    pub(crate) tab_bar_state: crate::tab_bar::TabBarState,
    pub(crate) menu_bar_state: MenuBarState,
    pub(crate) context_menu_state: crate::context_menu::ContextMenuState,
    pub(crate) platform: PlatformCapabilities,

    // Plugin system.
    pub(crate) show_plugin_manager: bool,
    pub(crate) plugin_manager: PluginManagerState,
    pub(crate) plugin_bus: Arc<PluginBus>,
    pub(crate) panel_registry: Arc<Mutex<PanelRegistry>>,
    pub(crate) native_plugin_mgr: NativePluginManager,
    /// Pending render responses from plugin threads (plugin_name → receiver).
    pub(crate) render_pending: HashMap<String, oneshot::Receiver<String>>,
    /// Cached widget JSON per plugin name (for rendering between polls).
    pub(crate) render_cache: HashMap<String, String>,
    /// Mutable text input state for plugin panels (keyed by widget id).
    pub(crate) plugin_text_state: HashMap<String, String>,
    /// Panel visibility toggles.
    pub(crate) left_panel_visible: bool,
    pub(crate) right_panel_visible: bool,
    pub(crate) bottom_panel_visible: bool,
    /// Active panel tab per location (handle of the selected panel).
    pub(crate) active_panel_tab: HashMap<PanelLocation, u64>,

    // Multi-window.
    pub(crate) extra_windows: Vec<ExtraWindow>,
    pub(crate) next_viewport_num: u32,

    // Host dialogs (plugin → UI thread).
    pub(crate) dialog_state: DialogState,

    // Plugin session registry (pending open/close from plugins).
    pub(crate) session_registry: Arc<Mutex<SessionRegistry>>,

    // Icons.
    pub(crate) icon_cache: Option<crate::icons::IconCache>,

    // System.
    pub(crate) ipc_listener: Option<IpcListener>,
    pub(crate) file_watcher: Option<FileWatcher>,
    pub(crate) has_ever_had_session: bool,
    pub(crate) quit_requested: bool,
    pub(crate) rt: Arc<tokio::runtime::Runtime>,
}

impl ConchApp {
    pub fn new(rt: Arc<tokio::runtime::Runtime>) -> Self {
        let user_config = config::load_user_config().unwrap_or_else(|e| {
            log::error!("Failed to load config: {e:#}");
            config::UserConfig::default()
        });
        let persistent = config::load_persistent_state().unwrap_or_default();

        let shortcuts = ResolvedShortcuts::from_config(&user_config.conch.keyboard);
        let platform = PlatformCapabilities::current();
        let menu_bar_state = MenuBarState::new(user_config.conch.ui.native_menu_bar, &platform);
        let state = AppState::new(user_config, persistent);

        let ipc_listener = IpcListener::start();
        let file_watcher = FileWatcher::start();

        // Plugin infrastructure.
        let plugin_bus = Arc::new(PluginBus::new());
        let panel_registry = Arc::new(Mutex::new(PanelRegistry::new()));
        let (dialog_tx, dialog_state) = dialogs::dialog_channel();
        let session_registry = Arc::new(Mutex::new(SessionRegistry::new()));
        bridge::init_bridge(
            Arc::clone(&plugin_bus),
            Arc::clone(&panel_registry),
            dialog_tx,
            Arc::clone(&session_registry),
        );
        let host_api = bridge::build_host_api();
        let native_plugin_mgr = NativePluginManager::new(Arc::clone(&plugin_bus), host_api);

        let mut app = Self {
            state,
            shortcuts,
            selection: Selection::default(),
            cell_width: 0.0,
            cell_height: 0.0,
            cell_size_measured: false,
            last_pixels_per_point: 0.0,
            last_cols: 0,
            last_rows: 0,
            cursor_visible: true,
            last_blink: Instant::now(),
            terminal_frame_cache: TerminalFrameCache::default(),
            tab_bar_state: crate::tab_bar::TabBarState::default(),
            menu_bar_state,
            context_menu_state: crate::context_menu::ContextMenuState::default(),
            platform,
            show_plugin_manager: false,
            plugin_manager: PluginManagerState::default(),
            plugin_bus,
            panel_registry,
            native_plugin_mgr,
            render_pending: HashMap::new(),
            render_cache: HashMap::new(),
            plugin_text_state: HashMap::new(),
            left_panel_visible: true,
            right_panel_visible: true,
            bottom_panel_visible: true,
            active_panel_tab: HashMap::new(),
            extra_windows: Vec::new(),
            next_viewport_num: 1,
            dialog_state,
            session_registry,
            icon_cache: None,
            ipc_listener,
            file_watcher,
            has_ever_had_session: false,
            quit_requested: false,
            rt,
        };

        // Discover plugins and auto-load previously enabled ones.
        app.discover_plugins();
        app.auto_load_plugins();

        app
    }

    /// Build a `ViewportBuilder` for extra windows matching main window decorations.
    pub(crate) fn build_extra_viewport(&self) -> egui::ViewportBuilder {
        let decorations = self.platform.effective_decorations(
            self.state.user_config.window.decorations,
        );
        crate::build_viewport(
            egui::ViewportBuilder::default().with_inner_size([800.0, 600.0]),
            decorations,
            &self.platform,
        )
    }

    /// Open a new OS window with a fresh local terminal tab.
    pub(crate) fn spawn_extra_window(&mut self) {
        let cwd = self.state
            .active_session()
            .and_then(|s| s.child_pid())
            .and_then(conch_pty::get_cwd_of_pid);
        let Some((_, session)) = create_local_session(&self.state.user_config, cwd) else {
            return;
        };
        let num = self.next_viewport_num;
        self.next_viewport_num += 1;
        let viewport_id = egui::ViewportId::from_hash_of(format!("conch_window_{num}"));
        let builder = self.build_extra_viewport();
        self.extra_windows.push(ExtraWindow::new(viewport_id, builder, session));
    }

    /// Poll terminal events for all main-window sessions.
    fn poll_events(&mut self) {
        let mut exited_sessions = Vec::new();

        for (id, session) in &mut self.state.sessions {
            while let Ok(event) = session.event_rx.try_recv() {
                match event {
                    alacritty_terminal::event::Event::Title(title) => {
                        if session.custom_title.is_none() {
                            session.title = title;
                        }
                    }
                    alacritty_terminal::event::Event::Exit => {
                        exited_sessions.push(*id);
                    }
                    _ => {}
                }
            }
        }

        for id in exited_sessions {
            self.remove_session(id);
        }
    }

    /// Handle file watcher events.
    fn handle_file_changes(&mut self, ctx: &egui::Context) {
        let Some(watcher) = &mut self.file_watcher else { return };
        let changes = watcher.poll();
        for change in changes {
            match change.kind {
                FileChangeKind::Config => {
                    log::info!("Config file changed, reloading...");
                    if let Ok(new_config) = config::load_user_config() {
                        self.shortcuts = ResolvedShortcuts::from_config(&new_config.conch.keyboard);
                        let scheme = conch_core::color_scheme::resolve_theme(&new_config.colors.theme);
                        self.state.colors = ResolvedColors::from_scheme(&scheme);
                        self.state.theme = crate::ui_theme::UiTheme::from_colors(&self.state.colors, new_config.colors.appearance_mode);
                        self.state.theme_dirty = true;
                        crate::apply_appearance_mode(ctx, new_config.colors.appearance_mode);
                        self.menu_bar_state.update_mode(new_config.conch.ui.native_menu_bar, &self.platform);
                        self.state.user_config = new_config;
                    }
                }
                FileChangeKind::Themes => {
                    log::info!("Themes changed, reloading...");
                    let scheme = conch_core::color_scheme::resolve_theme(&self.state.user_config.colors.theme);
                    self.state.colors = ResolvedColors::from_scheme(&scheme);
                    self.state.theme = crate::ui_theme::UiTheme::from_colors(&self.state.colors, self.state.user_config.colors.appearance_mode);
                    self.state.theme_dirty = true;
                }
            }
        }
    }

    /// Scan for native and Lua plugins and populate the plugin manager UI.
    ///
    /// Uses `[conch.plugins].search_paths` from config.toml. When empty, falls
    /// back to built-in defaults for development and the user plugin directory.
    /// Handle IPC messages from external processes.
    fn handle_ipc(&mut self) {
        let Some(listener) = &self.ipc_listener else { return };
        for msg in listener.drain() {
            match msg {
                IpcMessage::CreateWindow { working_directory } => {
                    let cwd = working_directory.map(std::path::PathBuf::from);
                    if let Some((_, session)) = create_local_session(&self.state.user_config, cwd) {
                        let num = self.next_viewport_num;
                        self.next_viewport_num += 1;
                        let viewport_id = egui::ViewportId::from_hash_of(format!("conch_window_{num}"));
                        let builder = self.build_extra_viewport();
                        self.extra_windows.push(ExtraWindow::new(viewport_id, builder, session));
                    }
                }
                IpcMessage::CreateTab { working_directory } => {
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

    /// Drain pending session open/close requests from plugins.
    fn drain_pending_sessions(&mut self) {
        let mut registry = self.session_registry.lock();
        let pending: Vec<_> = registry.pending_open.drain(..).collect();
        let closing: Vec<_> = registry.pending_close.drain(..).collect();
        let status_updates: Vec<_> = registry.pending_status.drain(..).collect();
        drop(registry);

        // Process session opens.
        for mut ps in pending {
            let id = uuid::Uuid::new_v4();
            let event_rx = ps.bridge.take_event_rx();
            let session = crate::state::Session {
                id,
                title: ps.title,
                custom_title: None,
                backend: crate::state::SessionBackend::Plugin {
                    bridge: ps.bridge,
                    vtable: ps.vtable,
                    backend_handle: ps.backend_handle,
                },
                event_rx,
                status: conch_plugin_sdk::SessionStatus::Connecting,
                status_detail: None,
                connect_started: Some(std::time::Instant::now()),
            };

            if self.last_cols > 0 && self.last_rows > 0 {
                session.resize(
                    self.last_cols, self.last_rows,
                    self.cell_width as u16, self.cell_height as u16,
                );
            }
            self.state.sessions.insert(id, session);
            self.state.tab_order.push(id);
            self.state.active_tab = Some(id);
            self.has_ever_had_session = true;
        }

        // Process session closes.
        for handle in closing {
            let id = self.state.sessions.iter().find_map(|(id, s)| {
                if let crate::state::SessionBackend::Plugin { bridge, .. } = &s.backend {
                    if bridge.handle == handle {
                        return Some(*id);
                    }
                }
                None
            });
            if let Some(id) = id {
                self.remove_session(id);
            }
        }

        // Process status updates.
        for update in status_updates {
            let id = self.state.sessions.iter().find_map(|(id, s)| {
                if let crate::state::SessionBackend::Plugin { bridge, .. } = &s.backend {
                    if bridge.handle == update.handle {
                        return Some(*id);
                    }
                }
                None
            });
            if let Some(id) = id {
                if let Some(session) = self.state.sessions.get_mut(&id) {
                    session.status = update.status;
                    session.status_detail = update.detail;
                }
            }
        }
    }
}

impl eframe::App for ConchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request continuous repainting for terminal output and cursor blink.
        ctx.request_repaint();

        // Measure font cell size (and re-measure on DPI changes).
        let ppp = ctx.pixels_per_point();
        if !self.cell_size_measured || (ppp - self.last_pixels_per_point).abs() > 0.001 {
            let font_size = self.state.user_config.font.size;
            let (cw, ch) = widget::measure_cell_size(ctx, font_size);
            self.cell_width = cw;
            self.cell_height = ch;
            self.cell_size_measured = true;
            self.last_pixels_per_point = ppp;
        }

        // Cursor blink.
        if self.last_blink.elapsed().as_millis() > CURSOR_BLINK_MS {
            self.cursor_visible = !self.cursor_visible;
            self.last_blink = Instant::now();
        }

        // Poll events.
        self.poll_events();
        self.handle_file_changes(ctx);
        self.handle_ipc();
        self.poll_plugin_renders();
        self.drain_pending_sessions();

        // Show plugin dialogs (form, confirm, prompt, alert, error).
        self.dialog_state.show(ctx);

        // Determine whether the main window should hide or close.
        let mut main_visible = !self.state.sessions.is_empty();

        // If the user clicks close on the main window while extra windows exist,
        // hide it instead of quitting.
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.extra_windows.is_empty() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            // Shut down main-window sessions.
            let ids: Vec<_> = self.state.tab_order.clone();
            for id in ids {
                self.remove_session(id);
            }
            self.has_ever_had_session = true;
            main_visible = false;
        }

        // When the last main-window tab closes, either hide or close.
        if self.state.sessions.is_empty() {
            if !self.has_ever_had_session {
                self.open_local_tab();
                self.has_ever_had_session = true;
                main_visible = true;
            } else {
                main_visible = false;
            }
        }

        // Show/hide the main viewport (extra windows are independent).
        if !main_visible {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }

        // Main-window copy/paste handling.
        if main_visible {
            let copy_requested = ctx.input(|i| {
                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
            });
            if copy_requested {
                if let Some((start, end)) = self.selection.normalized() {
                    if let Some(session) = self.state.active_session() {
                        let text = widget::get_selected_text(session.term(), start, end);
                        if !text.is_empty() {
                            ctx.copy_text(text);
                        }
                    }
                }
            }

            let paste_text: Option<String> = ctx.input(|i| {
                i.events.iter().find_map(|e| {
                    if let egui::Event::Paste(text) = e { Some(text.clone()) } else { None }
                })
            });
            if let Some(text) = paste_text {
                if let Some(session) = self.state.active_session() {
                    session.write(text.as_bytes());
                }
            }
        }

        // ── Render extra windows ──
        let effective_decorations = self.render_extra_windows(ctx);

        // If the main window is hidden and all extra windows are closed, quit.
        if !main_visible && self.extra_windows.is_empty() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        // Skip main-window UI rendering when hidden.
        if !main_visible {
            return;
        }

        // ── Apply centralized UI theme (only when changed) ──
        if self.state.theme_dirty {
            self.state.theme.apply(ctx);
            crate::host::bridge::update_theme_json(&self.state.theme);
            self.state.theme_dirty = false;
        }
        let bg_color = self.state.theme.bg;

        // Buttonless: no native title bar, so add a thin drag region at the top
        // so the user can still move the window.
        if effective_decorations == config::WindowDecorations::Buttonless {
            let drag_h = self.cell_height.max(6.0);
            egui::TopBottomPanel::top("drag_region")
                .exact_height(drag_h)
                .frame(egui::Frame::NONE.fill(self.state.theme.bg_with_alpha(180)))
                .show(ctx, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    let response = ui.interact(rect, ui.id().with("drag"), egui::Sense::drag());
                    if response.drag_started() {
                        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                    }
                });
        }

        // Tab bar at the top (only when more than one tab).
        for action in crate::tab_bar::show(ctx, &self.state, &mut self.tab_bar_state) {
            match action {
                crate::tab_bar::TabBarAction::SwitchTo(id) => {
                    self.state.active_tab = Some(id);
                }
                crate::tab_bar::TabBarAction::Close(id) => {
                    self.remove_session(id);
                }
            }
        }

        // Menu bar.
        if let Some(action) = crate::menu_bar::show(ctx, &mut self.menu_bar_state) {
            crate::menu_bar::handle_action(action, ctx, self);
        }

        // Plugin manager window (floating, toggled via View menu).
        if self.show_plugin_manager {
            let theme = self.state.theme.clone();
            let pm_actions = crate::host::plugin_manager_ui::show_plugin_manager_window(
                ctx,
                &mut self.show_plugin_manager,
                &mut self.plugin_manager,
                &theme,
            );
            for pm_action in pm_actions {
                self.handle_plugin_manager_action(pm_action);
            }
        }

        // Lazy-init icon cache on first frame (needs egui context for textures).
        if self.icon_cache.is_none() {
            self.icon_cache = Some(crate::icons::IconCache::load(ctx));
        }

        // Render plugin panels (side panels, bottom panels).
        self.render_plugin_panels(ctx);

        // Central panel: terminal.
        let mut pending_resize: Option<(u16, u16)> = None;
        let mut context_action: Option<crate::menu_bar::MenuAction> = None;

        let mut close_tab_requested = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg_color))
            .show(ctx, |ui| {
                if let Some(session) = self.state.active_tab.and_then(|id| self.state.sessions.get(&id)) {
                    match session.status {
                        conch_plugin_sdk::SessionStatus::Connecting => {
                            show_connecting_screen(ui, &session.title, session.status_detail.as_deref(), session.connect_started);
                        }
                        conch_plugin_sdk::SessionStatus::Error => {
                            let detail = session.status_detail.clone().unwrap_or_default();
                            if show_error_screen(ui, &session.title, &detail) {
                                close_tab_requested = true;
                            }
                        }
                        conch_plugin_sdk::SessionStatus::Connected => {
                            let sel = self.selection.normalized();
                            let term = session.term();
                            let (response, size_info) = widget::show_terminal(
                                ui,
                                term,
                                self.cell_width,
                                self.cell_height,
                                &self.state.colors,
                                self.state.user_config.font.size,
                                self.cursor_visible,
                                sel,
                                &mut self.terminal_frame_cache,
                            );

                            pending_resize = Some((size_info.columns() as u16, size_info.rows() as u16));

                            // Check mouse mode for context menu suppression.
                            let mouse_mode = term
                                .try_lock_unfair()
                                .map(|t| t.mode().intersects(alacritty_terminal::term::TermMode::MOUSE_MODE))
                                .unwrap_or(false);

                            // Mouse handling.
                            crate::mouse::handle_terminal_mouse(
                                ctx,
                                &response,
                                &size_info,
                                &mut self.selection,
                                term,
                                &|bytes| session.write(bytes),
                                self.cell_height,
                                self.state.user_config.terminal.scroll_sensitivity,
                            );

                            // Context menu (suppressed in mouse mode for tmux compatibility).
                            let has_selection = self.selection.normalized().is_some();
                            context_action = crate::context_menu::show(
                                &response,
                                &mut self.context_menu_state,
                                mouse_mode,
                                has_selection,
                            );
                        }
                    }
                }
            });

        // Handle close-tab request from error screen.
        if close_tab_requested {
            if let Some(id) = self.state.active_tab {
                self.remove_session(id);
            }
        }

        // Handle context menu action outside the panel closure.
        if let Some(action) = context_action {
            crate::menu_bar::handle_action(action, ctx, self);
        }

        // Resize sessions after releasing the panel borrow.
        if let Some((cols, rows)) = pending_resize {
            self.resize_sessions(cols, rows);
        }

        // Keyboard handling — forward to PTY unless a dialog is consuming input.
        let forward_to_pty = !self.dialog_state.is_active();
        self.handle_keyboard(ctx, forward_to_pty);

        // Quit handling.
        if self.quit_requested {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        // Update window title from active session.
        if let Some(session) = self.state.active_session() {
            let title = format!("{} — Conch", session.display_title());
            ctx.send_viewport_cmd(ViewportCommand::Title(title));
        }

        // Save window size on each frame (debounced by OS).
        let rect = ctx.input(|i| i.screen_rect());
        if rect.width() > 100.0 && rect.height() > 100.0 {
            self.state.persistent.layout.window_width = rect.width();
            self.state.persistent.layout.window_height = rect.height();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_loaded_plugins();
        self.native_plugin_mgr.shutdown_all();
        let _ = config::save_persistent_state(&self.state.persistent);
    }
}

// ---------------------------------------------------------------------------
// Connecting / Error screens for plugin sessions
// ---------------------------------------------------------------------------

/// Render a "Connecting to..." screen with a bouncing progress indicator.
fn show_connecting_screen(
    ui: &mut egui::Ui,
    title: &str,
    detail: Option<&str>,
    started: Option<std::time::Instant>,
) {
    let rect = ui.available_rect_before_wrap();
    let bg = if ui.visuals().dark_mode {
        egui::Color32::from_gray(30)
    } else {
        egui::Color32::from_gray(241)
    };
    ui.painter().rect_filled(rect, 0.0, bg);

    let center = rect.center();

    let heading = format!("Connecting to {title}\u{2026}");
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

    if let Some(detail) = detail {
        let detail_galley = ui.painter().layout_no_wrap(
            detail.to_string(),
            egui::FontId::new(16.0, egui::FontFamily::Proportional),
            if ui.visuals().dark_mode { egui::Color32::from_gray(200) } else { egui::Color32::from_gray(40) },
        );
        let detail_pos = egui::Pos2::new(
            center.x - detail_galley.size().x / 2.0,
            center.y + 5.0,
        );
        ui.painter().galley(detail_pos, detail_galley, egui::Color32::PLACEHOLDER);
    }

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

    let elapsed = started
        .map(|s| s.elapsed().as_secs_f32())
        .unwrap_or(0.0);
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
}

/// Render a connection error screen. Returns `true` if the user clicked "Close Tab".
fn show_error_screen(ui: &mut egui::Ui, title: &str, error: &str) -> bool {
    let rect = ui.available_rect_before_wrap();
    let bg = if ui.visuals().dark_mode {
        egui::Color32::from_gray(30)
    } else {
        egui::Color32::from_gray(241)
    };
    ui.painter().rect_filled(rect, 0.0, bg);

    let center = rect.center();
    let content_width = (rect.width() * 0.7).min(600.0);
    let content_rect = egui::Rect::from_center_size(
        center,
        egui::Vec2::new(content_width, rect.height() * 0.8),
    );

    let mut close = false;
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(
                    egui::RichText::new(format!("Connection to {title} failed"))
                        .size(24.0)
                        .color(egui::Color32::from_rgb(220, 50, 50)),
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
                    close = true;
                }
            });
            ui.add_space(12.0);
        });
    });

    close
}
