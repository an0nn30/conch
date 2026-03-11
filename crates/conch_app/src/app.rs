//! Main application struct and egui update loop.

use std::sync::Arc;
use std::time::Instant;

use conch_core::config;
use egui::{Color32, ViewportCommand};

use crate::extra_window::ExtraWindow;
use crate::input::ResolvedShortcuts;
use crate::ipc::{IpcListener, IpcMessage};
use crate::mouse::Selection;
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

    // Tab bar.
    pub(crate) tab_bar_state: crate::tab_bar::TabBarState,

    // Multi-window.
    pub(crate) extra_windows: Vec<ExtraWindow>,
    pub(crate) next_viewport_num: u32,

    // System.
    pub(crate) ipc_listener: Option<IpcListener>,
    pub(crate) file_watcher: Option<FileWatcher>,
    pub(crate) has_ever_had_session: bool,
    pub(crate) quit_requested: bool,
    pub(crate) use_native_menu: bool,
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
        let use_native_menu = cfg!(target_os = "macos") && user_config.conch.ui.native_menu_bar;
        let state = AppState::new(user_config, persistent);

        let ipc_listener = IpcListener::start();
        let file_watcher = FileWatcher::start();

        Self {
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
            extra_windows: Vec::new(),
            next_viewport_num: 1,
            ipc_listener,
            file_watcher,
            has_ever_had_session: false,
            quit_requested: false,
            use_native_menu,
            rt,
        }
    }

    /// Build a `ViewportBuilder` for extra windows matching main window decorations.
    pub(crate) fn build_extra_viewport(&self) -> egui::ViewportBuilder {
        use config::WindowDecorations;
        let native_menu = self.use_native_menu;
        let mut builder = egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0]);
        match self.state.user_config.window.decorations {
            WindowDecorations::Full => {
                if cfg!(target_os = "macos") && !native_menu {
                    builder = builder
                        .with_fullsize_content_view(true)
                        .with_titlebar_shown(true)
                        .with_title_shown(false);
                } else {
                    builder = builder
                        .with_title_shown(true)
                        .with_titlebar_shown(true);
                }
            }
            WindowDecorations::Transparent => {
                builder = builder
                    .with_fullsize_content_view(true)
                    .with_titlebar_shown(true)
                    .with_title_shown(false)
                    .with_transparent(true);
            }
            WindowDecorations::Buttonless => {
                builder = builder
                    .with_decorations(false)
                    .with_transparent(true);
            }
            WindowDecorations::None => {
                builder = builder.with_decorations(false);
            }
        }
        builder
    }

    /// Open a new OS window with a fresh local terminal tab.
    pub(crate) fn spawn_extra_window(&mut self) {
        let cwd = self.state
            .active_session()
            .map(|s| s.pty.child_pid())
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
                        crate::apply_appearance_mode(ctx, new_config.colors.appearance_mode);
                        self.state.user_config = new_config;
                    }
                }
                FileChangeKind::Themes => {
                    log::info!("Themes changed, reloading...");
                    let scheme = conch_core::color_scheme::resolve_theme(&self.state.user_config.colors.theme);
                    self.state.colors = ResolvedColors::from_scheme(&scheme);
                }
            }
        }
    }

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

        // Open initial tab on first frame, close app when all sessions have exited.
        if self.state.sessions.is_empty() {
            if self.has_ever_had_session {
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }
            self.open_local_tab();
            self.has_ever_had_session = true;
        }

        // Handle copy from selection (Cmd+C on macOS).
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

        // Handle paste (Cmd+V on macOS).
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

        // ── Render extra windows ──
        let mut windows_to_close: Vec<usize> = Vec::new();

        for (i, window) in self.extra_windows.iter_mut().enumerate() {
            window.update(&self.state.colors, &self.shortcuts, &self.state.user_config, self.state.user_config.font.size);

            if window.should_close {
                windows_to_close.push(i);
                continue;
            }

            let viewport_id = window.viewport_id;
            let builder = window.viewport_builder.clone();
            let title = window.title.clone();

            // Render the extra window viewport.
            ctx.show_viewport_deferred(
                viewport_id,
                builder.with_title(&title),
                |_ctx, _class| {
                    // Extra window rendering is handled by its own update() call.
                    // For now, we just keep the viewport alive.
                },
            );
        }

        // Remove closed windows (in reverse to preserve indices).
        for i in windows_to_close.into_iter().rev() {
            self.extra_windows.remove(i);
        }

        // ── Main window UI ──
        let bg_color = Color32::from_rgba_unmultiplied(
            (self.state.colors.background[0] * 255.0) as u8,
            (self.state.colors.background[1] * 255.0) as u8,
            (self.state.colors.background[2] * 255.0) as u8,
            255,
        );

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

        // Central panel: terminal.
        let mut pending_resize: Option<(u16, u16)> = None;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg_color))
            .show(ctx, |ui| {
                if let Some(session) = self.state.active_tab.and_then(|id| self.state.sessions.get(&id)) {
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
                }
            });

        // Resize sessions after releasing the panel borrow.
        if let Some((cols, rows)) = pending_resize {
            self.resize_sessions(cols, rows);
        }

        // Keyboard handling — always forward to PTY in Phase 1.
        self.handle_keyboard(ctx, true);

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
        let _ = config::save_persistent_state(&self.state.persistent);
    }
}
