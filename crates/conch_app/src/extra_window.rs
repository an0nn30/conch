//! Extra OS windows with independent terminal sessions.

use std::collections::HashMap;
use std::time::Instant;

use conch_core::config;
use egui::ViewportBuilder;
use uuid::Uuid;

use crate::input::ResolvedShortcuts;
use crate::mouse::Selection;
use crate::sessions::create_local_session;
use crate::state::Session;
use crate::terminal::color::ResolvedColors;
use crate::terminal::widget::TerminalFrameCache;

/// Actions that extra windows request from the main ConchApp.
pub enum ExtraWindowAction {
    SpawnNewWindow,
    QuitApp,
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
    pub last_cols: u16,
    pub last_rows: u16,
    pub selection: Selection,
    pub cursor_visible: bool,
    pub last_blink: Instant,
    pub frame_cache: TerminalFrameCache,
    pub should_close: bool,
    pub title: String,
    pub pending_actions: Vec<ExtraWindowAction>,
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
            last_cols: 0,
            last_rows: 0,
            selection: Selection::default(),
            cursor_visible: true,
            last_blink: Instant::now(),
            frame_cache: TerminalFrameCache::default(),
            should_close: false,
            title,
            pending_actions: Vec::new(),
        }
    }

    pub fn open_local_tab(&mut self, user_config: &config::UserConfig) {
        let cwd = self.active_tab
            .and_then(|id| self.sessions.get(&id))
            .map(|s| s.pty.child_pid())
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

    /// Render the extra window's UI. Returns whether the window should remain open.
    pub fn update(
        &mut self,
        _colors: &ResolvedColors,
        _shortcuts: &ResolvedShortcuts,
        _user_config: &config::UserConfig,
        _font_size: f32,
    ) {
        // Cursor blink.
        if self.last_blink.elapsed().as_millis() > 500 {
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

        if self.sessions.is_empty() {
            self.should_close = true;
        }

        // Update window title.
        if let Some(session) = self.active_tab.and_then(|id| self.sessions.get(&id)) {
            self.title = session.display_title().to_string();
        }
    }
}
