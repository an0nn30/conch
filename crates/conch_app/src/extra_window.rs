//! Extra OS windows with independent terminal sessions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use conch_core::config;
use conch_plugin::bus::PluginBus;
use conch_plugin_sdk::PanelLocation;
use egui::{ViewportBuilder, ViewportCommand};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::app::ConchApp;
use crate::host::bridge::PanelRegistry;
use crate::host::dialogs::DialogState;
use crate::icons::IconCache;
use crate::input::{self, ResolvedShortcuts};
use crate::mouse::Selection;
use crate::sessions::create_local_session;
use crate::state::Session;
use crate::tab_bar::{self, TabBarAction, TabBarState};
use crate::terminal::color::ResolvedColors;
use crate::terminal::widget::{self, TerminalFrameCache};
use crate::ui_theme::UiTheme;

/// Cursor blink interval in milliseconds.
const CURSOR_BLINK_MS: u128 = 500;

/// Actions that extra windows request from the main ConchApp.
pub enum ExtraWindowAction {
    SpawnNewWindow,
    QuitApp,
    PluginAction(crate::host::plugin_manager_ui::PluginManagerAction),
}

/// Read-only state borrowed from the main app for extra window rendering.
pub(crate) struct SharedState<'a> {
    pub user_config: &'a config::UserConfig,
    pub colors: &'a ResolvedColors,
    pub shortcuts: &'a ResolvedShortcuts,
    pub plugin_keybindings: &'a [crate::input::ResolvedPluginKeybind],
    pub theme: &'a UiTheme,
    pub effective_decorations: config::WindowDecorations,
    pub show_in_window_menu: bool,
    pub theme_dirty: bool,
    // Plugin panel state (shared from main app).
    pub panel_registry: &'a Arc<Mutex<PanelRegistry>>,
    pub plugin_bus: &'a Arc<PluginBus>,
    pub render_cache: &'a HashMap<String, String>,
    pub icon_cache: Option<&'a IconCache>,
    pub left_panel_width: f32,
    pub right_panel_width: f32,
    pub bottom_panel_height: f32,
}

/// An extra OS window with its own sessions and tabs.
pub struct ExtraWindow {
    pub viewport_id: egui::ViewportId,
    pub viewport_builder: ViewportBuilder,
    pub sessions: HashMap<Uuid, Session>,
    pub tab_order: Vec<Uuid>,
    pub active_tab: Option<Uuid>,
    pub cell_width: f32,
    pub cell_height: f32,
    pub cell_size_measured: bool,
    pub last_pixels_per_point: f32,
    pub last_cols: u16,
    pub last_rows: u16,
    pub selection: Selection,
    pub cursor_visible: bool,
    pub last_blink: Instant,
    pub frame_cache: TerminalFrameCache,
    pub should_close: bool,
    pub title: String,
    pub pending_actions: Vec<ExtraWindowAction>,
    pub tab_bar_state: TabBarState,
    pub style_applied: bool,
    pub show_plugin_manager: bool,
    /// Whether this window had OS focus during the last frame.
    pub has_focus: bool,
    /// Menu actions pushed by the main window for this extra window to handle.
    pub pending_menu_actions: Vec<crate::menu_bar::MenuAction>,
    /// Per-window panel visibility (independent from main window).
    pub left_panel_visible: bool,
    pub right_panel_visible: bool,
    pub bottom_panel_visible: bool,
}

impl ExtraWindow {
    pub fn new(viewport_id: egui::ViewportId, viewport_builder: ViewportBuilder, initial_session: Session) -> Self {
        let id = initial_session.id;
        let title = initial_session.display_title().to_string();
        let mut sessions = HashMap::new();
        sessions.insert(id, initial_session);

        Self {
            viewport_id,
            viewport_builder,
            sessions,
            tab_order: vec![id],
            active_tab: Some(id),
            cell_width: 0.0,
            cell_height: 0.0,
            cell_size_measured: false,
            last_pixels_per_point: 0.0,
            last_cols: 0,
            last_rows: 0,
            selection: Selection::default(),
            cursor_visible: true,
            last_blink: Instant::now(),
            frame_cache: TerminalFrameCache::default(),
            should_close: false,
            title,
            pending_actions: Vec::new(),
            tab_bar_state: TabBarState::default(),
            style_applied: false,
            show_plugin_manager: false,
            has_focus: false,
            pending_menu_actions: Vec::new(),
            left_panel_visible: true,
            right_panel_visible: true,
            bottom_panel_visible: true,
        }
    }

    pub fn open_local_tab(&mut self, user_config: &config::UserConfig) {
        let cwd = self.active_tab
            .and_then(|id| self.sessions.get(&id))
            .and_then(|s| s.child_pid())
            .and_then(conch_pty::get_cwd_of_pid);
        if let Some((id, session)) = create_local_session(user_config, cwd) {
            if self.last_cols > 0 && self.last_rows > 0 {
                session.resize(self.last_cols, self.last_rows, self.cell_width as u16, self.cell_height as u16);
            }
            self.sessions.insert(id, session);
            self.tab_order.push(id);
            self.active_tab = Some(id);
        }
    }

    /// Remove a session by ID, triggering the close animation.
    fn remove_session(&mut self, id: Uuid) {
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

    /// Get the active session, if any.
    fn active_session(&self) -> Option<&Session> {
        self.active_tab.and_then(|id| self.sessions.get(&id))
    }

    /// Render the extra window's UI inside a viewport closure.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        shared: &SharedState,
        plugin_manager: &mut crate::host::plugin_manager_ui::PluginManagerState,
        plugin_text_state: &mut HashMap<String, String>,
        active_panel_tab: &mut HashMap<PanelLocation, u64>,
        dialog_state: &mut DialogState,
    ) {
        // Clear pending actions from previous frame.
        self.pending_actions.clear();

        // Track OS-level focus for this window.
        self.has_focus = ctx.input(|i| i.focused);

        // Process any menu actions routed from the main window.
        for action in std::mem::take(&mut self.pending_menu_actions) {
            self.handle_menu_action(action, ctx, shared.user_config);
        }

        // Apply theme on first frame and when it changes.
        if !self.style_applied || shared.theme_dirty {
            shared.theme.apply(ctx);
            crate::apply_appearance_mode(ctx, shared.user_config.colors.appearance_mode);
            self.style_applied = true;
        }

        // Measure cell size (re-measure on DPI change).
        let ppp = ctx.pixels_per_point();
        if !self.cell_size_measured || (ppp - self.last_pixels_per_point).abs() > 0.001 {
            let font_size = shared.user_config.font.size;
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

        // Poll session events (title changes, exit).
        let mut exited_sessions = Vec::new();
        for (id, session) in &mut self.sessions {
            while let Ok(event) = session.event_rx.try_recv() {
                match event {
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
            self.sessions.remove(&id);
            self.tab_order.retain(|&tab_id| tab_id != id);
            if self.active_tab == Some(id) {
                self.active_tab = self.tab_order.last().copied();
            }
        }

        // Close window if no sessions remain.
        if self.sessions.is_empty() {
            self.should_close = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        // Handle window close request (shut down all sessions).
        if ctx.input(|i| i.viewport().close_requested()) {
            for (_, session) in &self.sessions {
                session.shutdown();
            }
            self.should_close = true;
            return;
        }

        // Copy/Paste event handling.
        let copy_requested = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
        });
        if copy_requested {
            if let Some((start, end)) = self.selection.normalized() {
                if let Some(session) = self.active_session() {
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
            if let Some(session) = self.active_session() {
                session.write(text.as_bytes());
            }
        }

        let bg_color = shared.theme.bg;

        // Buttonless drag region (matches main window).
        if shared.effective_decorations == config::WindowDecorations::Buttonless {
            let drag_h = self.cell_height.max(6.0);
            egui::TopBottomPanel::top("drag_region")
                .exact_height(drag_h)
                .frame(egui::Frame::NONE.fill(shared.theme.bg_with_alpha(180)))
                .show(ctx, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    let response = ui.interact(rect, ui.id().with("drag"), egui::Sense::drag());
                    if response.drag_started() {
                        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                    }
                });
        }

        // In-window menu bar (when not using native OS menu).
        // Use a viewport-unique ID to prevent menu state leaking between windows.
        if shared.show_in_window_menu {
            let menu_id = egui::Id::new("menu_bar").with(self.viewport_id);
            if let Some(action) = crate::menu_bar::egui_menu::show_with_id(ctx, menu_id) {
                self.handle_menu_action(action, ctx, shared.user_config);
            }
        }

        // Plugin manager window (floating, toggled via View menu).
        if self.show_plugin_manager {
            let pm_actions = crate::host::plugin_manager_ui::show_plugin_manager_window(
                ctx,
                &mut self.show_plugin_manager,
                plugin_manager,
                shared.theme,
            );
            for pm_action in pm_actions {
                self.pending_actions.push(ExtraWindowAction::PluginAction(pm_action));
            }
        }

        // Show plugin dialogs routed to this viewport.
        dialog_state.show(ctx, self.viewport_id);

        // Render plugin panels (same as main window, using shared state).
        crate::host::plugin_panels::render_plugin_panels_for_ctx(
            ctx,
            shared.panel_registry,
            shared.plugin_bus,
            shared.render_cache,
            plugin_text_state,
            active_panel_tab,
            self.left_panel_visible,
            self.right_panel_visible,
            self.bottom_panel_visible,
            shared.theme,
            shared.icon_cache,
            shared.left_panel_width,
            shared.right_panel_width,
            shared.bottom_panel_height,
            self.viewport_id,
        );
        // Extra windows don't persist panel sizes — only the main window does.

        // Tab bar.
        let tabs: Vec<(Uuid, String)> = self.tab_order.iter().map(|&id| {
            let title = self.sessions.get(&id)
                .map(|s| s.display_title().to_string())
                .unwrap_or_default();
            (id, title)
        }).collect();
        for action in tab_bar::show_for(ctx, &tabs, self.active_tab, shared.theme, &mut self.tab_bar_state) {
            match action {
                TabBarAction::SwitchTo(id) => {
                    self.active_tab = Some(id);
                }
                TabBarAction::Close(id) => {
                    self.remove_session(id);
                }
            }
        }

        // Central panel: terminal rendering + mouse handling.
        let mut pending_resize: Option<(u16, u16)> = None;
        let mut close_tab_requested = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg_color))
            .show(ctx, |ui| {
                if let Some(session) = self.active_tab.and_then(|id| self.sessions.get_mut(&id)) {
                    match session.status {
                        conch_plugin_sdk::SessionStatus::Connecting => {
                            let action = crate::app::show_connecting_screen(
                                ui,
                                &session.title,
                                session.status_detail.as_deref(),
                                session.connect_started,
                                session.prompt.as_mut(),
                                None, // icon_cache not available in extra windows on this branch
                            );
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
                            let sel = self.selection.normalized();
                            let term = session.term();
                            let (response, size_info) = widget::show_terminal(
                                ui,
                                term,
                                self.cell_width,
                                self.cell_height,
                                shared.colors,
                                shared.user_config.font.size,
                                self.cursor_visible,
                                sel,
                                &mut self.frame_cache,
                            );

                            pending_resize = Some((size_info.columns() as u16, size_info.rows() as u16));

                            // Mouse handling.
                            crate::mouse::handle_terminal_mouse(
                                ctx,
                                &response,
                                &size_info,
                                &mut self.selection,
                                term,
                                &|bytes| session.write(bytes),
                                self.cell_height,
                                shared.user_config.terminal.scroll_sensitivity,
                            );
                        }
                    }
                }
            });

        // Handle close-tab request from error screen.
        if close_tab_requested {
            if let Some(id) = self.active_tab {
                self.remove_session(id);
            }
        }

        // Resize sessions after releasing the panel borrow.
        if let Some((cols, rows)) = pending_resize {
            if cols != self.last_cols || rows != self.last_rows {
                self.last_cols = cols;
                self.last_rows = rows;
                for session in self.sessions.values() {
                    session.resize(cols, rows, self.cell_width as u16, self.cell_height as u16);
                }
            }
        }

        // Keyboard handling.
        self.handle_keyboard(ctx, shared);

        // Update window title.
        if let Some(session) = self.active_session() {
            let title = format!("{} — Conch", session.display_title());
            self.title = session.display_title().to_string();
            ctx.send_viewport_cmd(ViewportCommand::Title(title));
        }

        // Request repaint after 500ms for cursor blink.
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Handle a menu bar action locally within this extra window.
    fn handle_menu_action(&mut self, action: crate::menu_bar::MenuAction, ctx: &egui::Context, user_config: &config::UserConfig) {
        use crate::menu_bar::MenuAction;
        match action {
            MenuAction::NewTab => self.open_local_tab(user_config),
            MenuAction::NewWindow => self.pending_actions.push(ExtraWindowAction::SpawnNewWindow),
            MenuAction::CloseTab => {
                if let Some(id) = self.active_tab {
                    self.remove_session(id);
                }
            }
            MenuAction::Quit => self.pending_actions.push(ExtraWindowAction::QuitApp),
            MenuAction::Copy => {
                if let Some((start, end)) = self.selection.normalized() {
                    if let Some(session) = self.active_session() {
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
                self.show_plugin_manager = !self.show_plugin_manager;
            }
            // Actions not yet implemented.
            MenuAction::SelectAll | MenuAction::ZenMode | MenuAction::PluginAction { .. } => {}
        }
    }

    /// Handle keyboard input: app shortcuts and PTY forwarding.
    fn handle_keyboard(&mut self, ctx: &egui::Context, shared: &SharedState) {
        use alacritty_terminal::term::TermMode;

        let app_cursor = self.active_session().map_or(false, |s| {
            s.term()
                .try_lock_unfair()
                .map_or(false, |term| term.mode().contains(TermMode::APP_CURSOR))
        });

        // Collect key events to avoid borrow conflicts.
        let events: Vec<egui::Event> = ctx.input(|i| i.events.clone());

        for event in &events {
            match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // Command+number -> switch to tab N.
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
                            if let Some(&id) = self.tab_order.get(idx) {
                                self.active_tab = Some(id);
                                continue;
                            }
                        }
                    }

                    // App shortcuts.
                    if let Some(ref kb) = shared.shortcuts.new_window {
                        if kb.matches(key, modifiers) {
                            self.pending_actions.push(ExtraWindowAction::SpawnNewWindow);
                            continue;
                        }
                    }
                    if let Some(ref kb) = shared.shortcuts.new_tab {
                        if kb.matches(key, modifiers) {
                            self.open_local_tab(shared.user_config);
                            continue;
                        }
                    }
                    if let Some(ref kb) = shared.shortcuts.close_tab {
                        if kb.matches(key, modifiers) {
                            if let Some(id) = self.active_tab {
                                self.remove_session(id);
                            }
                            continue;
                        }
                    }
                    if let Some(ref kb) = shared.shortcuts.quit {
                        if kb.matches(key, modifiers) {
                            self.pending_actions.push(ExtraWindowAction::QuitApp);
                            continue;
                        }
                    }
                    if let Some(ref kb) = shared.shortcuts.toggle_left_panel {
                        if kb.matches(key, modifiers) {
                            self.left_panel_visible = !self.left_panel_visible;
                            continue;
                        }
                    }
                    if let Some(ref kb) = shared.shortcuts.toggle_right_panel {
                        if kb.matches(key, modifiers) {
                            self.right_panel_visible = !self.right_panel_visible;
                            continue;
                        }
                    }
                    if let Some(ref kb) = shared.shortcuts.toggle_bottom_panel {
                        if kb.matches(key, modifiers) {
                            self.bottom_panel_visible = !self.bottom_panel_visible;
                            continue;
                        }
                    }

                    // Plugin-registered global keybindings.
                    let mut plugin_handled = false;
                    for pkb in shared.plugin_keybindings {
                        if pkb.binding.matches(key, modifiers) {
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

                    // Forward to PTY.
                    if let Some(bytes) = input::key_to_bytes(key, modifiers, None, shared.shortcuts, app_cursor, shared.plugin_keybindings) {
                        if let Some(session) = self.active_session() {
                            if let Some(mut term) = session.term().try_lock_unfair() {
                                term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                            }
                            session.write(&bytes);
                        }
                    }
                }
                egui::Event::Text(text) => {
                    if let Some(session) = self.active_session() {
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
}

// ── Extra window orchestration on ConchApp ──

impl ConchApp {
    /// Render all extra windows and drain their pending actions.
    ///
    /// Returns the `effective_decorations` value so the caller can reuse it
    /// for the main window without recomputing.
    pub(crate) fn render_extra_windows(
        &mut self,
        ctx: &egui::Context,
    ) -> config::WindowDecorations {
        // Take windows out of self to avoid borrow conflict in the closure.
        let mut windows = std::mem::take(&mut self.extra_windows);
        let effective_decorations = self.platform.effective_decorations(
            self.state.user_config.window.decorations,
        );

        // Compute default panel sizes from persisted layout.
        let layout = &self.state.persistent.layout;
        let left_w = if layout.left_panel_width > 0.0 { layout.left_panel_width } else { 240.0 };
        let right_w = if layout.right_panel_width > 0.0 { layout.right_panel_width } else { 240.0 };
        let bottom_h = if layout.bottom_panel_height > 0.0 { layout.bottom_panel_height } else { 180.0 };

        let shared = SharedState {
            user_config: &self.state.user_config,
            colors: &self.state.colors,
            shortcuts: &self.shortcuts,
            plugin_keybindings: &self.plugin_keybindings,
            theme: &self.state.theme,
            effective_decorations,
            show_in_window_menu: self.menu_bar_state.is_in_window(),
            theme_dirty: self.state.theme_dirty,
            panel_registry: &self.panel_registry,
            plugin_bus: &self.plugin_bus,
            render_cache: &self.render_cache,
            icon_cache: self.icon_cache.as_ref(),
            left_panel_width: left_w,
            right_panel_width: right_w,
            bottom_panel_height: bottom_h,
        };

        for window in &mut windows {
            if window.should_close {
                continue;
            }
            let viewport_id = window.viewport_id;
            let builder = window.viewport_builder.clone().with_title(&window.title);
            ctx.show_viewport_immediate(
                viewport_id,
                builder,
                |vp_ctx, _class| {
                    window.update(
                        vp_ctx,
                        &shared,
                        &mut self.plugin_manager,
                        &mut self.plugin_text_state,
                        &mut self.active_panel_tab,
                        &mut self.dialog_state,
                    );
                },
            );
        }

        // Drain pending actions from extra windows.
        let mut spawn_new_window = false;
        for window in &mut windows {
            for action in window.pending_actions.drain(..) {
                match action {
                    ExtraWindowAction::SpawnNewWindow => spawn_new_window = true,
                    ExtraWindowAction::QuitApp => self.quit_requested = true,
                    ExtraWindowAction::PluginAction(pm_action) => {
                        self.handle_plugin_manager_action(pm_action);
                    }
                }
            }
        }

        // Remove closed windows and move back.
        windows.retain(|w| !w.should_close);
        self.extra_windows = windows;

        if spawn_new_window {
            self.spawn_extra_window();
        }

        effective_decorations
    }
}
