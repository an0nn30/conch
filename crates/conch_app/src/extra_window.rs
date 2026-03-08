//! Secondary window rendered via `show_viewport_immediate`.
//!
//! Each extra window has its own set of terminal sessions and tabs, but shares
//! the tokio runtime, user config, color scheme, shortcuts, and icon cache
//! with the main window.

use std::collections::HashMap;
use std::time::Instant;

use conch_core::config;
use uuid::Uuid;

use crate::icons::IconCache;
use crate::input::{self, ResolvedShortcuts};
use crate::mouse::{handle_terminal_mouse, Selection};
use crate::state::Session;
use crate::terminal::widget::{get_selected_text, measure_cell_size, show_terminal};
use crate::ui::sidebar;

use crate::app::{CURSOR_BLINK_MS, DEFAULT_COLS, DEFAULT_ROWS};
use crate::sessions::create_local_session;

pub struct ExtraWindow {
    pub viewport_id: egui::ViewportId,
    pub sessions: HashMap<Uuid, Session>,
    pub tab_order: Vec<Uuid>,
    pub active_tab: Option<Uuid>,
    pub cell_width: f32,
    pub cell_height: f32,
    cell_size_measured: bool,
    last_pixels_per_point: f32,
    cursor_visible: bool,
    last_blink: Instant,
    last_cols: u16,
    last_rows: u16,
    selection: Selection,
    pub should_close: bool,
    /// Whether this window currently has OS focus.
    pub is_focused: bool,
    /// User-visible window title.
    pub title: String,
}

impl ExtraWindow {
    pub fn new(viewport_id: egui::ViewportId, session: Session) -> Self {
        let id = session.id;
        let mut sessions = HashMap::new();
        sessions.insert(id, session);
        Self {
            viewport_id,
            sessions,
            tab_order: vec![id],
            active_tab: Some(id),
            cell_width: 8.0,
            cell_height: 16.0,
            cell_size_measured: false,
            last_pixels_per_point: 0.0,
            cursor_visible: true,
            last_blink: Instant::now(),
            last_cols: DEFAULT_COLS,
            last_rows: DEFAULT_ROWS,
            selection: Selection::default(),
            should_close: false,
            is_focused: false,
            title: "Conch".into(),
        }
    }

    fn active_session(&self) -> Option<&Session> {
        self.active_tab.and_then(|id| self.sessions.get(&id))
    }

    pub fn open_local_tab(&mut self, user_config: &config::UserConfig) {
        if let Some((id, session)) = create_local_session(user_config, None) {
            // Resize the new session to match the current window dimensions
            // so it doesn't start at the default size.
            if self.last_cols > 0 && self.last_rows > 0 {
                session.backend.resize(
                    self.last_cols,
                    self.last_rows,
                    self.cell_width as u16,
                    self.cell_height as u16,
                );
            }
            self.sessions.insert(id, session);
            self.tab_order.push(id);
            self.active_tab = Some(id);
        }
    }

    fn remove_session(&mut self, id: Uuid) {
        if let Some(session) = self.sessions.remove(&id) {
            session.backend.shutdown();
        }
        self.tab_order.retain(|&tab_id| tab_id != id);
        if self.active_tab == Some(id) {
            self.active_tab = self.tab_order.last().copied();
        }
    }

    fn resize_sessions(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 || (cols == self.last_cols && rows == self.last_rows) {
            return;
        }
        self.last_cols = cols;
        self.last_rows = rows;
        let cw = self.cell_width as u16;
        let ch = self.cell_height as u16;
        for session in self.sessions.values() {
            session.backend.resize(cols, rows, cw, ch);
        }
    }

    /// Render this window's content. Called from `show_viewport_immediate`.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        user_config: &config::UserConfig,
        colors: &crate::terminal::color::ResolvedColors,
        shortcuts: &ResolvedShortcuts,
        icon_cache: &Option<IconCache>,
    ) {
        // Track OS-level focus for this window.
        self.is_focused = ctx.input(|i| i.focused);

        // Measure cell size, and re-measure when pixels_per_point changes.
        let ppp = ctx.pixels_per_point();
        if !self.cell_size_measured || self.last_pixels_per_point != ppp {
            let (cw, ch) = measure_cell_size(ctx, user_config.font.size);
            let offset = &user_config.font.offset;
            if cw > 0.0 && ch > 0.0 {
                self.cell_width = (cw + offset.x).max(1.0);
                self.cell_height = (ch + offset.y).max(1.0);
                self.cell_size_measured = true;
                self.last_pixels_per_point = ppp;
                self.last_cols = 0;
                self.last_rows = 0;
            }
        }

        // Cursor blink.
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_blink).as_millis();
        if elapsed >= CURSOR_BLINK_MS {
            self.cursor_visible = !self.cursor_visible;
            self.last_blink = now;
            ctx.request_repaint_after(std::time::Duration::from_millis(CURSOR_BLINK_MS as u64));
        } else {
            let remaining = CURSOR_BLINK_MS - elapsed;
            ctx.request_repaint_after(std::time::Duration::from_millis(remaining as u64));
        }

        // Poll terminal events.
        let mut exited = Vec::new();
        for session in self.sessions.values_mut() {
            while let Ok(event) = session.event_rx.try_recv() {
                match event {
                    alacritty_terminal::event::Event::Wakeup => ctx.request_repaint(),
                    alacritty_terminal::event::Event::Title(title) => session.title = title,
                    alacritty_terminal::event::Event::Exit => exited.push(session.id),
                    _ => {}
                }
            }
        }
        for id in exited {
            self.remove_session(id);
        }

        // Close window if no sessions remain.
        if self.sessions.is_empty() {
            self.should_close = true;
        }

        // Handle close request from window chrome.
        if ctx.input(|i| i.viewport().close_requested()) {
            self.should_close = true;
            for session in self.sessions.values() {
                session.backend.shutdown();
            }
        }

        if self.should_close {
            return;
        }

        // Tab handling: intercept Tab key before egui consumes it.
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
                            false
                        }
                        _ => true,
                    });
                });
                consumed_tab_for_pty = tab_bytes.is_some();
                if let Some(bytes) = tab_bytes {
                    if let Some(session) = self.active_session() {
                        session.backend.write(&bytes);
                    }
                }
            } else {
                consumed_tab_for_pty = false;
            }
        }

        // Collect copy/paste events.
        let mut copy_requested = false;
        let mut paste_text: Option<String> = None;
        let mut ctrl_c_for_pty = false;
        let mut ctrl_x_for_pty = false;
        ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Copy | egui::Event::Cut => {
                        if cfg!(target_os = "macos") {
                            copy_requested = true;
                        } else {
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

        // Tab bar (only when multiple tabs).
        if self.tab_order.len() > 1 {
            self.render_tab_bar(ctx, user_config, icon_cache);
        }

        // Central panel (terminal).
        let font_size = user_config.font.size;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                if let Some(id) = self.active_tab {
                    if let Some(session) = self.sessions.get(&id) {
                        let term = session.backend.term().clone();
                        let (response, size_info) = show_terminal(
                            ui,
                            &term,
                            self.cell_width,
                            self.cell_height,
                            colors,
                            font_size,
                            self.cursor_visible,
                            self.selection.normalized(),
                        );

                        // Mouse selection/forwarding.
                        {
                            let cell_height = self.cell_height;
                            let sessions = &self.sessions;
                            let active_tab = self.active_tab;
                            let write_fn = |data: &[u8]| {
                                if let Some(s) = active_tab.and_then(|id| sessions.get(&id)) {
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
                    }
                }
            });

        // Undo Tab focus cycling.
        if consumed_tab_for_pty {
            if let Some(id) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
        }
        let forward_to_pty = !ctx.memory(|m| m.focused().is_some());

        // Paste (with bracketed paste support).
        if let Some(text) = paste_text {
            if forward_to_pty {
                if let Some(session) = self.active_session() {
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
                if let Some(session) = self.active_session() {
                    let paths: Vec<String> = dropped
                        .iter()
                        .filter_map(|f| f.path.as_ref())
                        .map(|p| {
                            let s = p.to_string_lossy().into_owned();
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

        // On Linux/Windows, forward Ctrl+C/X to the PTY as control characters.
        if forward_to_pty {
            if ctrl_c_for_pty {
                if let Some(session) = self.active_session() {
                    session.backend.write(&[0x03]);
                }
            }
            if ctrl_x_for_pty {
                if let Some(session) = self.active_session() {
                    session.backend.write(&[0x18]);
                }
            }
        }

        // Keyboard input.
        self.handle_keyboard(ctx, forward_to_pty, user_config, shortcuts);

        // Window title.
        let window_title = self.active_session()
            .map(|s| {
                let name = s.custom_title.as_ref().unwrap_or(&s.title);
                format!("{name} — Conch")
            })
            .unwrap_or_else(|| "Conch".into());
        self.title = window_title.clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(window_title));
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn render_tab_bar(
        &mut self,
        ctx: &egui::Context,
        user_config: &config::UserConfig,
        icon_cache: &Option<IconCache>,
    ) {
        egui::TopBottomPanel::top("extra_tab_bar")
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
                painter.rect_filled(panel_rect, 0.0, darker_bg);

                let mut switch_to = None;
                let mut close_id = None;
                let mut x = panel_rect.min.x;
                let tab_count = self.tab_order.len();
                let tab_w = TAB_MAX_W.min(panel_rect.width() / (tab_count as f32 + 1.0));

                for &id in &self.tab_order {
                    if let Some(session) = self.sessions.get(&id) {
                        let title = session.custom_title.as_deref().unwrap_or(&session.title);
                        let selected = self.active_tab == Some(id);
                        let tab_rect = egui::Rect::from_min_size(
                            egui::Pos2::new(x, panel_rect.min.y),
                            egui::Vec2::new(tab_w, tab_h),
                        );
                        if selected {
                            painter.rect_filled(tab_rect, 0.0, base_bg);
                            let accent_rect = egui::Rect::from_min_size(
                                egui::Pos2::new(tab_rect.min.x, tab_rect.max.y - 3.0),
                                egui::Vec2::new(tab_w, 3.0),
                            );
                            painter.rect_filled(accent_rect, 0.0, accent_color);
                        }

                        // Close button area.
                        let close_size = 14.0;
                        let close_pad = 4.0;
                        let close_x = tab_rect.max.x - close_size - close_pad;
                        let close_y = tab_rect.center().y - close_size / 2.0;
                        let close_rect = egui::Rect::from_min_size(
                            egui::Pos2::new(close_x - 2.0, close_y - 2.0),
                            egui::Vec2::new(close_size + 4.0, close_size + 4.0),
                        );
                        if let Some(tex_id) = icon_cache.as_ref().and_then(|ic| ic.texture_id(crate::icons::Icon::TabClose)) {
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

                        // Tab label.
                        let label_color = if selected { text_color } else { dim_text };
                        let galley = painter.layout_no_wrap(
                            title.to_string(),
                            font_id.clone(),
                            label_color,
                        );
                        let text_pos = egui::Pos2::new(
                            tab_rect.min.x + 6.0,
                            tab_rect.center().y - galley.size().y / 2.0,
                        );
                        painter.galley(text_pos, galley, label_color);

                        let tab_resp = ui.interact(
                            tab_rect,
                            ui.id().with(("extra_tab", id)),
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
                        x += tab_w;
                    }
                }

                // "+" button.
                let plus_rect = egui::Rect::from_min_size(
                    egui::Pos2::new(x + 4.0, panel_rect.min.y),
                    egui::Vec2::new(24.0, tab_h),
                );
                let plus_galley = painter.layout_no_wrap("+".to_string(), font_id, dim_text);
                painter.galley(
                    egui::Pos2::new(
                        plus_rect.center().x - plus_galley.size().x / 2.0,
                        plus_rect.center().y - plus_galley.size().y / 2.0,
                    ),
                    plus_galley,
                    dim_text,
                );
                let plus_resp = ui.interact(plus_rect, ui.id().with("extra_tab_plus"), egui::Sense::click());
                if plus_resp.clicked() {
                    self.open_local_tab(user_config);
                }

                if let Some(id) = switch_to {
                    self.active_tab = Some(id);
                }
                if let Some(id) = close_id {
                    self.remove_session(id);
                    if self.sessions.is_empty() {
                        self.open_local_tab(user_config);
                    }
                }
            });
    }

    fn handle_keyboard(
        &mut self,
        ctx: &egui::Context,
        forward_to_pty: bool,
        user_config: &config::UserConfig,
        shortcuts: &ResolvedShortcuts,
    ) {
        use alacritty_terminal::term::TermMode;

        let app_cursor = forward_to_pty
            && self.active_session().map_or(false, |s| {
                s.backend
                    .term()
                    .try_lock_unfair()
                    .map_or(false, |term| term.mode().contains(TermMode::APP_CURSOR))
            });

        ctx.input(|input_state| {
            for event in &input_state.events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        // Cmd+number → switch tab.
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
                                    return;
                                }
                            }
                        }

                        // New tab.
                        if let Some(ref kb) = shortcuts.new_tab {
                            if kb.matches(key, modifiers) {
                                self.open_local_tab(user_config);
                                return;
                            }
                        }
                        // Close tab.
                        if let Some(ref kb) = shortcuts.close_tab {
                            if kb.matches(key, modifiers) {
                                if let Some(id) = self.active_tab {
                                    self.remove_session(id);
                                    if self.sessions.is_empty() {
                                        self.open_local_tab(user_config);
                                    }
                                }
                                return;
                            }
                        }
                        // Quit shortcut closes just this window.
                        if let Some(ref kb) = shortcuts.quit {
                            if kb.matches(key, modifiers) {
                                self.should_close = true;
                                return;
                            }
                        }

                        // On Linux/Windows, Ctrl+Shift+C copies terminal selection.
                        #[cfg(not(target_os = "macos"))]
                        if forward_to_pty && modifiers.ctrl && modifiers.shift && *key == egui::Key::C {
                            if let Some((start, end)) = self.selection.normalized() {
                                if let Some(session) = self.active_session() {
                                    let text = get_selected_text(session.backend.term(), start, end);
                                    if !text.is_empty() {
                                        ctx.copy_text(text);
                                    }
                                }
                            }
                            return;
                        }

                        // Forward to PTY.
                        if forward_to_pty {
                            if let Some(bytes) = input::key_to_bytes(key, modifiers, None, shortcuts, app_cursor) {
                                if let Some(session) = self.active_session() {
                                    session.backend.write(&bytes);
                                }
                            }
                        }
                    }
                    egui::Event::Text(text) => {
                        if forward_to_pty {
                            if let Some(session) = self.active_session() {
                                session.backend.write(text.as_bytes());
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    }
}
