//! Extra OS windows with independent terminal sessions.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use conch_core::config;
use egui::{ViewportBuilder, ViewportCommand};
use uuid::Uuid;

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
    pub theme: &'a UiTheme,
    pub effective_decorations: config::WindowDecorations,
    pub show_in_window_menu: bool,
    pub theme_dirty: bool,
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
    ) {
        // Clear pending actions from previous frame.
        self.pending_actions.clear();

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
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg_color))
            .show(ctx, |ui| {
                if let Some(session) = self.active_tab.and_then(|id| self.sessions.get(&id)) {
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
            });

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
            MenuAction::SelectAll | MenuAction::ZenMode => {}
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

                    // Forward to PTY.
                    if let Some(bytes) = input::key_to_bytes(key, modifiers, None, shared.shortcuts, app_cursor) {
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
