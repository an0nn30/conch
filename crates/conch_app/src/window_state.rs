//! Per-window state and shared app state for the unified window architecture.
//!
//! `WindowState` — all state unique to a single OS window.
//! `SharedAppState` — global state shared across all windows (Arc-wrapped).
//! `SharedConfig` — read-mostly configuration, theme, shortcuts.
//! `WindowAction` — cross-cutting actions sent from windows to the coordinator.
//! `render_window()` — THE single rendering path for ALL windows.
//! `handle_menu_action()` — free function replacing menu_bar::handle_action.
//! `handle_keyboard()` — free function replacing shortcuts.rs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use conch_core::config;
use conch_plugin::bus::PluginBus;
use conch_plugin_sdk::PanelLocation;
use parking_lot::Mutex;
use uuid::Uuid;

use crate::context_menu::ContextMenuState;
use crate::host::bridge::{PanelRegistry, SessionRegistry};
use crate::host::dialogs::DialogState;
use crate::icons::IconCache;
use crate::input::{self, ResolvedPluginKeybind, ResolvedShortcuts};
use crate::menu_bar::{MenuAction, MenuBarState};
use crate::mouse::Selection;
use crate::notifications::NotificationManager;
use crate::platform::PlatformCapabilities;
use crate::sessions::create_local_session;
use crate::state::Session;
use crate::tab_bar::{self, TabBarState};
use crate::terminal::color::ResolvedColors;
use crate::terminal::widget::{self, TerminalFrameCache};
use crate::ui_theme::UiTheme;

/// Cursor blink interval in milliseconds.
const CURSOR_BLINK_MS: u128 = 500;

// ── SharedConfig ──

/// Read-mostly configuration state updated on config/theme reload.
pub(crate) struct SharedConfig {
    pub user_config: config::UserConfig,
    pub persistent: config::PersistentState,
    pub colors: ResolvedColors,
    pub theme: UiTheme,
    pub theme_dirty: bool,
    pub shortcuts: ResolvedShortcuts,
    pub plugin_keybindings: Vec<ResolvedPluginKeybind>,
    pub plugin_keybindings_version: u64,
    /// Monotonically increasing version bumped whenever the theme changes.
    /// Windows compare this to `last_theme_version` to know when to re-apply.
    pub theme_version: u64,
}

// ── SharedAppState ──

/// Global state shared across all windows.
///
/// All fields use interior-mutability wrappers (`Mutex`) for `Send + Sync`
/// compatibility with deferred viewport closures. Since all viewports render
/// on the same thread, there is no actual lock contention.
pub(crate) struct SharedAppState {
    /// Configuration, theme, colors, shortcuts.
    pub config: Mutex<SharedConfig>,
    /// Plugin publish/subscribe event bus.
    pub plugin_bus: Arc<PluginBus>,
    /// Registered plugin panels (location + name).
    pub panel_registry: Arc<Mutex<PanelRegistry>>,
    /// Pending session open/close from plugins.
    pub session_registry: Arc<Mutex<SessionRegistry>>,
    /// Cached widget JSON per plugin name.
    pub render_cache: Mutex<HashMap<String, String>>,
    /// Plugin dialog state (per-viewport).
    pub dialog_state: Mutex<DialogState>,
    /// Toast notification manager.
    pub notifications: Mutex<NotificationManager>,
    /// Icon cache (lazy-initialized).
    pub icon_cache: Mutex<Option<IconCache>>,
    /// Menu bar rendering state.
    pub menu_bar_state: Mutex<MenuBarState>,
    /// Plugin manager UI state.
    pub plugin_manager: Mutex<crate::host::plugin_manager_ui::PluginManagerState>,
    /// Platform capabilities (immutable).
    pub platform: PlatformCapabilities,
}

// ── WindowAction ──

/// Actions that windows send to the coordinator (`ConchApp::update()`).
///
/// Windows can't mutate `ConchApp` directly (deferred viewport callbacks
/// are `Fn`, not `FnMut`). Cross-cutting operations go through this channel.
pub(crate) enum WindowAction {
    SpawnNewWindow,
    Quit,
    PluginAction(crate::host::plugin_manager_ui::PluginManagerAction),
    WindowClosed(egui::ViewportId),
    /// Snapshot the full layout state from the focused window so it persists
    /// across sessions.  Sent every frame from whichever window has focus.
    SaveLayoutState {
        window_width: f32,
        window_height: f32,
        zoom_factor: f32,
        left_panel_width: Option<f32>,
        right_panel_width: Option<f32>,
        bottom_panel_height: Option<f32>,
        left_panel_visible: bool,
        right_panel_visible: bool,
        bottom_panel_visible: bool,
        status_bar_visible: bool,
    },
    PublishTabChanged {
        is_ssh: bool,
        session_id: Option<u64>,
    },
}

// ── WindowState ──

/// Per-window state shared by main and extra windows.
pub(crate) struct WindowState {
    // ── Sessions / tabs ──
    pub sessions: HashMap<Uuid, Session>,
    pub tab_order: Vec<Uuid>,
    pub active_tab: Option<Uuid>,

    // ── Terminal rendering ──
    pub cell_width: f32,
    pub cell_height: f32,
    pub cell_size_measured: bool,
    pub last_pixels_per_point: f32,
    pub last_cols: u16,
    pub last_rows: u16,
    pub cursor_visible: bool,
    pub last_blink: Instant,
    pub frame_cache: TerminalFrameCache,
    pub selection: Selection,

    // ── UI chrome ──
    pub tab_bar_state: TabBarState,
    pub context_menu_state: ContextMenuState,
    pub show_plugin_manager: bool,
    pub left_panel_visible: bool,
    pub right_panel_visible: bool,
    pub bottom_panel_visible: bool,
    pub show_status_bar: bool,
    /// Mutable text input state for plugin panels (keyed by widget id).
    pub plugin_text_state: HashMap<String, String>,
    /// Active panel tab per location (handle of the selected panel).
    pub active_panel_tab: HashMap<PanelLocation, u64>,

    // ── Viewport info ──
    pub viewport_id: egui::ViewportId,
    pub viewport_builder: Option<egui::ViewportBuilder>,
    pub title: String,
    pub should_close: bool,
    /// Theme version that was last applied to this window's egui context.
    pub last_theme_version: u64,
    /// Whether this window had OS focus during the last frame.
    pub has_focus: bool,
    /// Pending actions to send to the coordinator.
    pub pending_actions: Vec<WindowAction>,
    /// Menu actions routed from the native menu bar.
    pub pending_menu_actions: Vec<MenuAction>,
}

impl WindowState {
    /// Create a new window state for a given viewport, restoring panel
    /// visibility from the persisted layout.
    pub fn new(viewport_id: egui::ViewportId, layout: &config::LayoutConfig) -> Self {
        Self {
            sessions: HashMap::new(),
            tab_order: Vec::new(),
            active_tab: None,
            cell_width: 0.0,
            cell_height: 0.0,
            cell_size_measured: false,
            last_pixels_per_point: 0.0,
            last_cols: 0,
            last_rows: 0,
            cursor_visible: true,
            last_blink: Instant::now(),
            frame_cache: TerminalFrameCache::default(),
            selection: Selection::default(),
            tab_bar_state: TabBarState::default(),
            context_menu_state: ContextMenuState::default(),
            show_plugin_manager: false,
            left_panel_visible: layout.left_panel_visible,
            right_panel_visible: layout.right_panel_visible,
            bottom_panel_visible: layout.bottom_panel_visible,
            show_status_bar: layout.status_bar_visible,
            plugin_text_state: HashMap::new(),
            active_panel_tab: HashMap::new(),
            viewport_id,
            viewport_builder: None,
            title: String::new(),
            should_close: false,
            last_theme_version: 0,
            has_focus: false,
            pending_actions: Vec::new(),
            pending_menu_actions: Vec::new(),
        }
    }

    /// Create a new window state with an initial session.
    pub fn with_session(
        viewport_id: egui::ViewportId,
        viewport_builder: egui::ViewportBuilder,
        session: Session,
        layout: &config::LayoutConfig,
    ) -> Self {
        let mut state = Self::new(viewport_id, layout);
        let id = session.id;
        state.title = session.display_title().to_string();
        state.viewport_builder = Some(viewport_builder);
        state.sessions.insert(id, session);
        state.tab_order.push(id);
        state.active_tab = Some(id);
        state
    }

    /// Get the currently active session, if any.
    pub fn active_session(&self) -> Option<&Session> {
        self.active_tab.and_then(|id| self.sessions.get(&id))
    }

    /// Get a mutable reference to the active session.
    pub fn active_session_mut(&mut self) -> Option<&mut Session> {
        self.active_tab.and_then(|id| self.sessions.get_mut(&id))
    }

    /// Open a new local terminal tab, inheriting the CWD from the active session.
    pub fn open_local_tab(&mut self, user_config: &config::UserConfig) {
        let cwd = self.active_tab
            .and_then(|id| self.sessions.get(&id))
            .and_then(|s| s.child_pid())
            .and_then(conch_pty::get_cwd_of_pid);
        if let Some((id, session)) = create_local_session(user_config, cwd) {
            if self.last_cols > 0 && self.last_rows > 0 {
                session.resize(
                    self.last_cols, self.last_rows,
                    self.cell_width as u16, self.cell_height as u16,
                );
            }
            self.sessions.insert(id, session);
            self.tab_order.push(id);
            self.active_tab = Some(id);
        }
    }

    /// Remove a session by ID, triggering the close animation.
    pub fn remove_session(&mut self, id: Uuid) {
        let title = self.sessions.get(&id)
            .map(|s| s.display_title().to_string())
            .unwrap_or_default();
        let index = self.tab_order.iter().position(|&t| t == id).unwrap_or(0);
        self.tab_bar_state.begin_close(id, title, index);

        if let Some(session) = self.sessions.remove(&id) {
            session.shutdown();
        }
        self.tab_order.retain(|&tab_id| tab_id != id);
        if self.active_tab == Some(id) {
            self.active_tab = self.tab_order.last().copied();
        }
    }

    /// Resize all sessions if the computed grid dimensions changed.
    pub fn resize_sessions(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 || (cols == self.last_cols && rows == self.last_rows) {
            return;
        }
        self.last_cols = cols;
        self.last_rows = rows;
        let cw = self.cell_width as u16;
        let ch = self.cell_height as u16;
        for session in self.sessions.values() {
            session.resize(cols, rows, cw, ch);
        }
    }

    /// Toggle zen mode: hide/show panels and status bar.
    pub fn toggle_zen_mode(&mut self) {
        if self.left_panel_visible || self.right_panel_visible || self.show_status_bar {
            self.left_panel_visible = false;
            self.right_panel_visible = false;
            self.show_status_bar = false;
        } else {
            self.left_panel_visible = true;
            self.right_panel_visible = true;
            self.show_status_bar = true;
        }
    }
}

// ── render_window ──

/// The single rendering path for ALL windows (main and extra).
///
/// This replaces both the main window rendering in `app.rs update()` AND
/// `ExtraWindow::update_deferred()`.
pub(crate) fn render_window(
    ctx: &egui::Context,
    win: &mut WindowState,
    shared: &SharedAppState,
) {
    use egui::ViewportCommand;
    use std::time::Duration;

    // 1. Clear pending actions from previous frame.
    win.pending_actions.clear();

    // 2. Track OS focus.
    win.has_focus = ctx.input(|i| i.focused);

    // 3. Strip Tab key events unless a dialog is open for this viewport.
    //
    // For the root viewport, raw_input_hook already stripped Tab before
    // egui processed it.  For immediate viewports, egui's begin_pass()
    // has already consumed the Tab event and set focus_direction in Memory.
    // We must ALSO clear any focus that Tab triggered, so the menu bar
    // doesn't steal focus from the terminal.
    if !shared.dialog_state.lock().is_active_for(win.viewport_id) {
        let mut tab_bytes: Option<Vec<u8>> = None;
        ctx.input_mut(|input| {
            input.events.retain(|e| match e {
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
                    false
                }
                _ => true,
            });
        });
        if let Some(bytes) = tab_bytes {
            // Undo the focus navigation that egui's begin_pass() already
            // applied from this Tab event.  Without this, the menu bar in
            // extra windows (show_viewport_immediate) gains focus because
            // raw_input_hook only fires for the root viewport.
            if let Some(focused_id) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(focused_id));
            }
            if let Some(session) = win.active_session() {
                session.write(&bytes);
            }
        }
    }

    // 4. Lock config. Process pending menu actions.
    let cfg = shared.config.lock();
    {
        let actions = std::mem::take(&mut win.pending_menu_actions);
        for action in actions {
            handle_menu_action(action, ctx, win, &cfg, shared);
        }
    }

    // 5. Apply theme if version changed, and restore persisted zoom on first frame.
    if win.last_theme_version != cfg.theme_version {
        cfg.theme.apply(ctx);
        crate::apply_appearance_mode(ctx, cfg.user_config.colors.appearance_mode);
        crate::host::bridge::update_theme_json(&cfg.theme);
        // On first frame (version 0 → N), restore persisted zoom factor.
        if win.last_theme_version == 0 {
            let zoom = cfg.persistent.layout.zoom_factor;
            if zoom > 0.0 && (zoom - ctx.pixels_per_point()).abs() > 0.01 {
                ctx.set_pixels_per_point(zoom);
            }
        }
        win.last_theme_version = cfg.theme_version;
    }

    // 6. Measure cell size (re-measure on DPI change).
    let ppp = ctx.pixels_per_point();
    if !win.cell_size_measured || (ppp - win.last_pixels_per_point).abs() > 0.001 {
        let font_size = cfg.user_config.font.size;
        let (cw, ch) = widget::measure_cell_size(ctx, font_size);
        win.cell_width = cw;
        win.cell_height = ch;
        win.cell_size_measured = true;
        win.last_pixels_per_point = ppp;
    }

    // 7. Cursor blink (500ms interval) — only toggle when focused.
    let mut needs_repaint = false;
    if win.has_focus {
        let elapsed = win.last_blink.elapsed().as_millis();
        if elapsed >= CURSOR_BLINK_MS {
            win.cursor_visible = !win.cursor_visible;
            win.last_blink = Instant::now();
            needs_repaint = true;
        }
    }

    // 8. Poll session events (title changes, exit, wakeup).
    let mut exited_sessions = Vec::new();
    for (id, session) in &mut win.sessions {
        while let Ok(event) = session.event_rx.try_recv() {
            match event {
                alacritty_terminal::event::Event::Wakeup => {
                    needs_repaint = true;
                }
                alacritty_terminal::event::Event::Title(t) => {
                    if session.custom_title.is_none() {
                        session.title = t;
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
        win.sessions.remove(&id);
        win.tab_order.retain(|&tab_id| tab_id != id);
        if win.active_tab == Some(id) {
            win.active_tab = win.tab_order.last().copied();
        }
    }

    // 9. Mark window for closing if no sessions remain.
    // Do NOT send ViewportCommand::Close here — the coordinator decides
    // whether to actually close, hide, or exit based on how many windows
    // are still open.  This keeps all windows equal.
    if win.sessions.is_empty() {
        win.should_close = true;
        drop(cfg);
        return;
    }

    // 10. Handle OS close request (user clicked X).
    // Cancel the OS-level close and just mark ourselves for closing.
    // The coordinator will handle removal / app exit.
    if ctx.input(|i| i.viewport().close_requested()) {
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        for (_, session) in &win.sessions {
            session.shutdown();
        }
        win.should_close = true;
        drop(cfg);
        return;
    }

    // 11. Copy/Paste handling.
    let copy_requested = ctx.input(|i| {
        i.events.iter().any(|e| matches!(e, egui::Event::Copy))
    });
    if copy_requested {
        if let Some((start, end)) = win.selection.normalized() {
            if let Some(session) = win.active_session() {
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
        if let Some(session) = win.active_session() {
            session.write(text.as_bytes());
        }
    }

    // 12. Lazy-init icon cache on first frame.
    {
        let mut ic = shared.icon_cache.lock();
        if ic.is_none() {
            *ic = Some(IconCache::load(ctx));
        }
    }

    // 13. Compute effective decorations from platform + config.
    let effective_decorations = shared.platform.effective_decorations(
        cfg.user_config.window.decorations,
    );

    let bg_color = cfg.theme.bg;
    let theme_clone = cfg.theme.clone();
    let colors_clone = cfg.colors.clone();
    let font_size = cfg.user_config.font.size;
    let scroll_sensitivity = cfg.user_config.terminal.scroll_sensitivity;
    let shortcuts = cfg.shortcuts.clone();
    let plugin_keybindings = cfg.plugin_keybindings.clone();
    let layout = cfg.persistent.layout.clone();

    // 14. Buttonless drag region.
    if effective_decorations == config::WindowDecorations::Buttonless {
        let drag_h = win.cell_height.max(6.0);
        egui::TopBottomPanel::top("drag_region")
            .exact_height(drag_h)
            .frame(egui::Frame::NONE.fill(theme_clone.bg_with_alpha(180)))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let response = ui.interact(rect, ui.id().with("drag"), egui::Sense::drag());
                if response.drag_started() {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
            });
    }

    // 15. macOS fullsize_content_view titlebar spacer.
    if effective_decorations == config::WindowDecorations::Full
        && cfg!(target_os = "macos")
    {
        let title_bar_h = 34.0;
        egui::TopBottomPanel::top("titlebar_spacer")
            .exact_height(title_bar_h)
            .frame(egui::Frame::NONE.fill(theme_clone.surface))
            .show(ctx, |_ui| {});
    }

    // 16. In-window menu bar (egui_menu::show_with_id with viewport-unique ID).
    let show_in_window_menu = shared.menu_bar_state.lock().is_in_window();
    if show_in_window_menu {
        let menu_id = egui::Id::new("menu_bar").with(win.viewport_id);
        if let Some(action) = crate::menu_bar::egui_menu::show_with_id(ctx, menu_id) {
            handle_menu_action(action, ctx, win, &cfg, shared);
        }
    }

    // Native menu bar — only drain if this window has focus.
    if win.has_focus && !show_in_window_menu {
        if let Some(action) = crate::menu_bar::show(ctx, &mut *shared.menu_bar_state.lock()) {
            handle_menu_action(action, ctx, win, &cfg, shared);
        }
    }

    // 17. Plugin manager floating window.
    if win.show_plugin_manager {
        let pm_actions = crate::host::plugin_manager_ui::show_plugin_manager_window(
            ctx,
            &mut win.show_plugin_manager,
            &mut *shared.plugin_manager.lock(),
            &cfg.theme,
        );
        for pm_action in pm_actions {
            win.pending_actions.push(WindowAction::PluginAction(pm_action));
        }
    }

    // 18. Drop config lock before dialog/notification locks.
    drop(cfg);

    // 19. Plugin dialogs.
    shared.dialog_state.lock().show(ctx, win.viewport_id);

    // 20. Status bar.
    if win.show_status_bar {
        crate::host::plugin_panels::render_status_bar(ctx, &theme_clone);
    }

    // 21. Plugin panels.
    let panel_sizes = {
        let render_cache = shared.render_cache.lock();
        let icon_cache = shared.icon_cache.lock();
        let left_w = if layout.left_panel_width > 0.0 { layout.left_panel_width } else { 240.0 };
        let right_w = if layout.right_panel_width > 0.0 { layout.right_panel_width } else { 240.0 };
        let bottom_h = if layout.bottom_panel_height > 0.0 { layout.bottom_panel_height } else { 180.0 };
        crate::host::plugin_panels::render_plugin_panels_for_ctx(
            ctx,
            &shared.panel_registry,
            &shared.plugin_bus,
            &render_cache,
            &mut win.plugin_text_state,
            &mut win.active_panel_tab,
            win.left_panel_visible,
            win.right_panel_visible,
            win.bottom_panel_visible,
            &theme_clone,
            icon_cache.as_ref(),
            left_w,
            right_w,
            bottom_h,
            win.viewport_id,
        )
    };

    // If window has focus, snapshot the full layout state so it persists.
    if win.has_focus {
        let rect = ctx.input(|i| i.screen_rect());
        win.pending_actions.push(WindowAction::SaveLayoutState {
            window_width: rect.width(),
            window_height: rect.height(),
            zoom_factor: ctx.pixels_per_point(),
            left_panel_width: panel_sizes.left_width,
            right_panel_width: panel_sizes.right_width,
            bottom_panel_height: panel_sizes.bottom_height,
            left_panel_visible: win.left_panel_visible,
            right_panel_visible: win.right_panel_visible,
            bottom_panel_visible: win.bottom_panel_visible,
            status_bar_visible: win.show_status_bar,
        });
    }

    // 22. Tab bar.
    {
        let tabs: Vec<(Uuid, String)> = win.tab_order.iter().map(|&id| {
            let title = win.sessions.get(&id)
                .map(|s| s.display_title().to_string())
                .unwrap_or_default();
            (id, title)
        }).collect();
        for action in tab_bar::show_for(ctx, &tabs, win.active_tab, &theme_clone, &mut win.tab_bar_state) {
            match action {
                tab_bar::TabBarAction::SwitchTo(id) => {
                    win.active_tab = Some(id);
                }
                tab_bar::TabBarAction::Close(id) => {
                    win.remove_session(id);
                }
            }
        }
    }

    // 23. Central panel: terminal rendering + mouse handling.
    let mut pending_resize: Option<(u16, u16)> = None;
    let mut close_tab_requested = false;
    let mut context_action: Option<MenuAction> = None;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(bg_color))
        .show(ctx, |ui| {
            if let Some(session) = win.active_tab.and_then(|id| win.sessions.get_mut(&id)) {
                match session.status {
                    conch_plugin_sdk::SessionStatus::Connecting => {
                        let icon_cache = shared.icon_cache.lock();
                        let action = crate::app::show_connecting_screen(
                            ui,
                            &session.title,
                            session.status_detail.as_deref(),
                            session.connect_started,
                            session.prompt.as_mut(),
                            icon_cache.as_ref(),
                        );
                        drop(icon_cache);
                        match action {
                            crate::app::ConnectingAction::Accept => {
                                if let Some(prompt) = session.prompt.take() {
                                    if let Some(reply) = prompt.reply {
                                        let _ = reply.send(Some("true".to_string()));
                                    }
                                }
                            }
                            crate::app::ConnectingAction::Reject => {
                                if let Some(prompt) = session.prompt.take() {
                                    if let Some(reply) = prompt.reply {
                                        let _ = reply.send(None);
                                    }
                                }
                            }
                            crate::app::ConnectingAction::SubmitPassword(pw) => {
                                if let Some(prompt) = session.prompt.take() {
                                    if let Some(reply) = prompt.reply {
                                        let _ = reply.send(Some(pw));
                                    }
                                }
                            }
                            crate::app::ConnectingAction::None => {}
                        }
                    }
                    conch_plugin_sdk::SessionStatus::Error => {
                        let detail = session.status_detail.clone().unwrap_or_default();
                        if crate::app::show_error_screen(ui, &session.title, &detail) {
                            close_tab_requested = true;
                        }
                    }
                    conch_plugin_sdk::SessionStatus::Connected => {
                        let sel = win.selection.normalized();
                        let term = session.term();
                        let (response, size_info) = widget::show_terminal(
                            ui,
                            term,
                            win.cell_width,
                            win.cell_height,
                            &colors_clone,
                            font_size,
                            win.cursor_visible,
                            sel,
                            &mut win.frame_cache,
                        );

                        pending_resize = Some((size_info.columns() as u16, size_info.rows() as u16));

                        let mouse_mode = term
                            .try_lock_unfair()
                            .map(|t| t.mode().intersects(alacritty_terminal::term::TermMode::MOUSE_MODE))
                            .unwrap_or(false);

                        crate::mouse::handle_terminal_mouse(
                            ctx,
                            &response,
                            &size_info,
                            &mut win.selection,
                            term,
                            &|bytes| session.write(bytes),
                            win.cell_height,
                            scroll_sensitivity,
                        );

                        let has_selection = win.selection.normalized().is_some();
                        context_action = crate::context_menu::show(
                            &response,
                            &mut win.context_menu_state,
                            mouse_mode,
                            has_selection,
                        );
                    }
                }
            }
        });

    // 24. Context menu action handling.
    if let Some(action) = context_action {
        let cfg = shared.config.lock();
        handle_menu_action(action, ctx, win, &cfg, shared);
    }

    if close_tab_requested {
        if let Some(id) = win.active_tab {
            win.remove_session(id);
        }
    }

    // 25. Resize sessions.
    if let Some((cols, rows)) = pending_resize {
        win.resize_sessions(cols, rows);
    }

    // 26. Keyboard handling.
    handle_keyboard(ctx, win, &shortcuts, &plugin_keybindings, shared);

    // 27. Update window title — only send when changed to avoid triggering
    // a repaint loop (send_viewport_cmd always calls request_repaint).
    if let Some(session) = win.active_session() {
        let new_title = session.display_title().to_string();
        if new_title != win.title {
            win.title = new_title;
            ctx.send_viewport_cmd(ViewportCommand::Title(format!("{} — Conch", &win.title)));
        }
    }

    // 28. Toast notifications.
    {
        let mut notifs = shared.notifications.lock();
        if notifs.has_active() {
            needs_repaint = true;
        }
        notifs.show(ctx);
    }

    // 29. Schedule next repaint only when needed.
    if needs_repaint {
        ctx.request_repaint();
    } else if win.has_focus {
        let elapsed = win.last_blink.elapsed().as_millis() as u64;
        let remaining = (CURSOR_BLINK_MS as u64).saturating_sub(elapsed);
        ctx.request_repaint_after(Duration::from_millis(remaining.max(16)));
    }
    // When unfocused and nothing changed: no repaint requested → true idle.
}

// ── handle_menu_action ──

/// Handle a menu bar action, mutating per-window state as needed.
///
/// Free function replacing `menu_bar::handle_action` and
/// `ExtraWindow::handle_menu_action_deferred`.
pub(crate) fn handle_menu_action(
    action: MenuAction,
    ctx: &egui::Context,
    win: &mut WindowState,
    cfg: &SharedConfig,
    shared: &SharedAppState,
) {
    use egui::ViewportCommand;

    match action {
        MenuAction::NewTab => {
            win.open_local_tab(&cfg.user_config);
        }
        MenuAction::NewWindow => {
            win.pending_actions.push(WindowAction::SpawnNewWindow);
        }
        MenuAction::CloseTab => {
            if let Some(id) = win.active_tab {
                win.remove_session(id);
            }
        }
        MenuAction::Quit => {
            win.pending_actions.push(WindowAction::Quit);
        }
        MenuAction::Copy => {
            if let Some((start, end)) = win.selection.normalized() {
                if let Some(session) = win.active_session() {
                    let text = widget::get_selected_text(session.term(), start, end);
                    if !text.is_empty() {
                        ctx.copy_text(text);
                    }
                }
            }
        }
        MenuAction::Paste => {
            ctx.send_viewport_cmd(ViewportCommand::RequestPaste);
        }
        MenuAction::ZenMode => {
            win.toggle_zen_mode();
        }
        MenuAction::ZoomIn => {
            let current = ctx.pixels_per_point();
            ctx.set_pixels_per_point(current + 0.5);
        }
        MenuAction::ZoomOut => {
            let current = ctx.pixels_per_point();
            ctx.set_pixels_per_point((current - 0.5).max(0.5));
        }
        MenuAction::ZoomReset => {
            ctx.set_pixels_per_point(1.0);
        }
        MenuAction::PluginManager => {
            win.show_plugin_manager = !win.show_plugin_manager;
        }
        MenuAction::PluginAction { plugin_name, action } => {
            crate::host::bridge::set_event_viewport(&plugin_name, win.viewport_id);
            let event = conch_plugin_sdk::PluginEvent::MenuAction { action: action.clone() };
            if let Ok(json) = serde_json::to_string(&event) {
                if let Some(sender) = shared.plugin_bus.sender_for(&plugin_name) {
                    log::info!("Dispatching menu action '{action}' to plugin '{plugin_name}'");
                    let _ = sender.try_send(conch_plugin::bus::PluginMail::WidgetEvent { json });
                } else {
                    log::warn!("No bus sender found for plugin '{plugin_name}' (menu action '{action}')");
                }
            }
        }
        MenuAction::SelectAll => {
            // TODO: implement select-all for terminal content
        }
    }
}

// ── handle_keyboard ──

/// Process keyboard events: app shortcuts, plugin keybindings, and PTY forwarding.
///
/// Free function replacing `ConchApp::handle_keyboard` and
/// `ExtraWindow::handle_keyboard_deferred`.
pub(crate) fn handle_keyboard(
    ctx: &egui::Context,
    win: &mut WindowState,
    shortcuts: &ResolvedShortcuts,
    plugin_keybindings: &[ResolvedPluginKeybind],
    shared: &SharedAppState,
) {
    use alacritty_terminal::term::TermMode;

    let app_cursor = win.active_session().map_or(false, |s| {
        s.term()
            .try_lock_unfair()
            .map_or(false, |term| term.mode().contains(TermMode::APP_CURSOR))
    });

    let events: Vec<egui::Event> = ctx.input(|i| i.events.clone());

    for event in &events {
        match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                // Cmd+1-9 -> switch tab.
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
                        if let Some(&id) = win.tab_order.get(idx) {
                            win.active_tab = Some(id);
                            continue;
                        }
                    }
                }

                // App-level configurable shortcuts.
                if let Some(ref kb) = shortcuts.new_window {
                    if kb.matches(key, modifiers) {
                        win.pending_actions.push(WindowAction::SpawnNewWindow);
                        continue;
                    }
                }
                if let Some(ref kb) = shortcuts.new_tab {
                    if kb.matches(key, modifiers) {
                        let user_config = shared.config.lock().user_config.clone();
                        win.open_local_tab(&user_config);
                        continue;
                    }
                }
                if let Some(ref kb) = shortcuts.close_tab {
                    if kb.matches(key, modifiers) {
                        if let Some(id) = win.active_tab {
                            win.remove_session(id);
                        }
                        continue;
                    }
                }
                if let Some(ref kb) = shortcuts.quit {
                    if kb.matches(key, modifiers) {
                        win.pending_actions.push(WindowAction::Quit);
                        continue;
                    }
                }
                if let Some(ref kb) = shortcuts.toggle_left_panel {
                    if kb.matches(key, modifiers) {
                        win.left_panel_visible = !win.left_panel_visible;
                        continue;
                    }
                }
                if let Some(ref kb) = shortcuts.toggle_right_panel {
                    if kb.matches(key, modifiers) {
                        win.right_panel_visible = !win.right_panel_visible;
                        continue;
                    }
                }
                if let Some(ref kb) = shortcuts.toggle_bottom_panel {
                    if kb.matches(key, modifiers) {
                        win.bottom_panel_visible = !win.bottom_panel_visible;
                        continue;
                    }
                }
                if let Some(ref kb) = shortcuts.zen_mode {
                    if kb.matches(key, modifiers) {
                        win.toggle_zen_mode();
                        continue;
                    }
                }

                // Plugin-registered global keybindings.
                let mut plugin_handled = false;
                for pkb in plugin_keybindings {
                    if pkb.binding.matches(key, modifiers) {
                        crate::host::bridge::set_event_viewport(
                            &pkb.plugin_name,
                            win.viewport_id,
                        );
                        let event = conch_plugin_sdk::PluginEvent::MenuAction {
                            action: pkb.action.clone(),
                        };
                        if let Ok(json) = serde_json::to_string(&event) {
                            if let Some(sender) = shared.plugin_bus.sender_for(&pkb.plugin_name) {
                                let _ = sender.try_send(conch_plugin::bus::PluginMail::WidgetEvent { json });
                            }
                        }
                        plugin_handled = true;
                        break;
                    }
                }
                if plugin_handled {
                    continue;
                }

                // Ctrl+Shift+C for copy on non-macOS.
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::C {
                    if let Some((start, end)) = win.selection.normalized() {
                        if let Some(session) = win.active_session() {
                            let text = widget::get_selected_text(session.term(), start, end);
                            if !text.is_empty() {
                                ctx.copy_text(text);
                            }
                        }
                    }
                    continue;
                }

                // Forward to active terminal via key_to_bytes.
                if let Some(bytes) = input::key_to_bytes(key, modifiers, None, shortcuts, app_cursor, plugin_keybindings) {
                    if let Some(session) = win.active_session() {
                        if let Some(mut term) = session.term().try_lock_unfair() {
                            term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                        }
                        session.write(&bytes);
                    }
                }
            }
            egui::Event::Text(text) => {
                if let Some(session) = win.active_session() {
                    if let Some(mut term) = session.term().try_lock_unfair() {
                        term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                    }
                    session.write(text.as_bytes());
                }
            }
            _ => {}
        }
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_state() -> WindowState {
        let layout = config::LayoutConfig::default();
        WindowState::new(egui::ViewportId::from_hash_of("test"), &layout)
    }

    #[test]
    fn new_window_state_defaults() {
        let ws = make_test_state();
        assert!(ws.sessions.is_empty());
        assert!(ws.tab_order.is_empty());
        assert!(ws.active_tab.is_none());
        assert!(ws.left_panel_visible);
        assert!(ws.right_panel_visible);
        assert!(ws.bottom_panel_visible);
        assert!(ws.show_status_bar);
        assert!(!ws.should_close);
        assert!(!ws.has_focus);
    }

    #[test]
    fn active_session_returns_none_when_empty() {
        let ws = make_test_state();
        assert!(ws.active_session().is_none());
    }

    #[test]
    fn toggle_zen_mode_hides_panels_and_status_bar() {
        let mut ws = make_test_state();
        assert!(ws.left_panel_visible);
        assert!(ws.right_panel_visible);
        assert!(ws.show_status_bar);

        ws.toggle_zen_mode();
        assert!(!ws.left_panel_visible);
        assert!(!ws.right_panel_visible);
        assert!(!ws.show_status_bar);
    }

    #[test]
    fn toggle_zen_mode_restores_panels_and_status_bar() {
        let mut ws = make_test_state();
        ws.toggle_zen_mode(); // hide
        ws.toggle_zen_mode(); // restore

        assert!(ws.left_panel_visible);
        assert!(ws.right_panel_visible);
        assert!(ws.show_status_bar);
    }

    #[test]
    fn toggle_zen_mode_partial_visibility_hides_all() {
        let mut ws = make_test_state();
        ws.left_panel_visible = false;
        ws.right_panel_visible = false;
        ws.show_status_bar = true;

        ws.toggle_zen_mode();
        assert!(!ws.left_panel_visible);
        assert!(!ws.right_panel_visible);
        assert!(!ws.show_status_bar);
    }

    #[test]
    fn restores_panel_visibility_from_layout() {
        let mut layout = config::LayoutConfig::default();
        layout.left_panel_visible = false;
        layout.right_panel_visible = false;
        layout.bottom_panel_visible = true;
        layout.status_bar_visible = false;
        let ws = WindowState::new(egui::ViewportId::from_hash_of("test"), &layout);
        assert!(!ws.left_panel_visible);
        assert!(!ws.right_panel_visible);
        assert!(ws.bottom_panel_visible);
        assert!(!ws.show_status_bar);
    }

    #[test]
    fn remove_session_from_empty_is_safe() {
        let mut ws = make_test_state();
        ws.remove_session(Uuid::new_v4());
        assert!(ws.sessions.is_empty());
    }

    #[test]
    fn resize_sessions_ignores_zero_dimensions() {
        let mut ws = make_test_state();
        ws.resize_sessions(0, 24);
        assert_eq!(ws.last_cols, 0);
        assert_eq!(ws.last_rows, 0);
    }

    #[test]
    fn resize_sessions_ignores_unchanged_dimensions() {
        let mut ws = make_test_state();
        ws.last_cols = 80;
        ws.last_rows = 24;
        ws.resize_sessions(80, 24);
        assert_eq!(ws.last_cols, 80);
        assert_eq!(ws.last_rows, 24);
    }

    #[test]
    fn shared_config_round_trip() {
        let user_config = config::UserConfig::default();
        let persistent = config::PersistentState::default();
        let scheme = conch_core::color_scheme::resolve_theme(&user_config.colors.theme);
        let colors = ResolvedColors::from_scheme(&scheme);
        let theme = UiTheme::from_colors(&colors, user_config.colors.appearance_mode);
        let shortcuts = ResolvedShortcuts::from_config(&user_config.conch.keyboard);

        let cfg = SharedConfig {
            user_config,
            persistent,
            colors,
            theme,
            theme_dirty: true,
            shortcuts,
            plugin_keybindings: Vec::new(),
            plugin_keybindings_version: 0,
            theme_version: 0,
        };

        assert!(cfg.theme_dirty);
        assert!(cfg.plugin_keybindings.is_empty());
        assert_eq!(cfg.theme_version, 0);
    }

    #[test]
    fn theme_version_tracking() {
        let mut ws = make_test_state();
        assert_eq!(ws.last_theme_version, 0, "new window starts at version 0");

        // Simulate the window "applying" theme version 1.
        ws.last_theme_version = 1;
        assert_eq!(ws.last_theme_version, 1);

        // A SharedConfig with version 2 would cause re-apply.
        let needs_apply = ws.last_theme_version != 2;
        assert!(needs_apply, "should need re-apply when versions differ");

        // After applying, versions match.
        ws.last_theme_version = 2;
        let needs_apply = ws.last_theme_version != 2;
        assert!(!needs_apply, "should not need re-apply when versions match");
    }

    #[test]
    fn theme_version_in_shared_config() {
        let user_config = config::UserConfig::default();
        let persistent = config::PersistentState::default();
        let scheme = conch_core::color_scheme::resolve_theme(&user_config.colors.theme);
        let colors = ResolvedColors::from_scheme(&scheme);
        let theme = UiTheme::from_colors(&colors, user_config.colors.appearance_mode);
        let shortcuts = ResolvedShortcuts::from_config(&user_config.conch.keyboard);

        let mut cfg = SharedConfig {
            user_config,
            persistent,
            colors,
            theme,
            theme_dirty: false,
            shortcuts,
            plugin_keybindings: Vec::new(),
            plugin_keybindings_version: 0,
            theme_version: 1,
        };

        assert_eq!(cfg.theme_version, 1);

        // Simulate a theme reload bumping the version.
        cfg.theme_version += 1;
        assert_eq!(cfg.theme_version, 2);

        // A window at version 1 would detect the mismatch.
        let ws = make_test_state();
        assert_ne!(ws.last_theme_version, cfg.theme_version);
    }
}
