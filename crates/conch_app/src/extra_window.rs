//! Secondary window rendered via `show_viewport_immediate`.
//!
//! Each extra window has its own set of terminal sessions and tabs, but shares
//! the tokio runtime, user config, color scheme, shortcuts, and icon cache
//! with the main window.
//!
//! The expanded extra window supports full UI: left sidebar (files/plugins),
//! right sidebar (sessions), bottom panel, menu bar, and window decorations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use conch_core::config::{self, WindowDecorations};
use uuid::Uuid;

use crate::icons::{Icon, IconCache};
use crate::input::{self, ResolvedShortcuts};
use crate::mouse::{handle_terminal_mouse, Selection};
use crate::sessions::{create_local_session, load_local_entries};
use crate::state::Session;
use crate::terminal::widget::{get_selected_text, measure_cell_size, show_terminal};
use crate::ui::bottom_panel::{self, BottomPanelAction};
use crate::ui::file_browser::FileBrowserState;
use crate::ui::session_panel::{self, SessionPanelAction, SessionPanelState};
use crate::ui::sidebar::{self, SidebarAction, SidebarTab};

use crate::app::{CURSOR_BLINK_MS, DEFAULT_COLS, DEFAULT_ROWS};

// ---------------------------------------------------------------------------
// Shared read-only state borrowed from ConchApp
// ---------------------------------------------------------------------------

/// Bundles all read-only references from `ConchApp` that extra windows need
/// for rendering. Constructed each frame and passed into `ExtraWindow::update`.
pub(crate) struct SharedState<'a> {
    pub user_config: &'a config::UserConfig,
    pub colors: &'a crate::terminal::color::ResolvedColors,
    pub shortcuts: &'a ResolvedShortcuts,
    pub icon_cache: &'a Option<IconCache>,
    pub sessions_config: &'a config::SessionsConfig,
    pub ssh_config_hosts: &'a [conch_core::models::ServerEntry],
    pub plugin_display: &'a [sidebar::PluginDisplayInfo],
    pub plugin_output_lines: &'a [String],
    pub panel_widgets: &'a HashMap<usize, Vec<conch_plugin::PanelWidget>>,
    pub panel_names: &'a HashMap<usize, String>,
    pub plugin_icons: &'a HashMap<usize, egui::TextureHandle>,
    pub use_native_menu: bool,
    pub bottom_panel_tabs: &'a [usize],
    pub transfers: &'a [sidebar::TransferStatus],
}

// ---------------------------------------------------------------------------
// Actions that need ConchApp to process
// ---------------------------------------------------------------------------

/// Actions produced by the extra window that require `ConchApp` to handle
/// (e.g. opening dialogs, running plugins, SSH connections).
pub(crate) enum ExtraWindowAction {
    SpawnNewWindow,
    QuitApp,
    OpenNewConnection,
    OpenPreferences,
    OpenAbout,
    OpenTunnelDialog,
    OpenNotificationHistory,
    RunPlugin(usize),
    RefreshPlugins,
    ApplyPluginChanges(Vec<usize>),
    PanelButtonClick { plugin_idx: usize, button_id: String },
    DeactivatePanel(usize),
    SessionPanelAction(SessionPanelAction),
    BottomPanelAction(BottomPanelAction),
}

// ---------------------------------------------------------------------------
// Menu shortcut helper (duplicated from app.rs to avoid pub(crate) change)
// ---------------------------------------------------------------------------

fn cmd_shortcut(key: &str) -> egui::WidgetText {
    use egui::text::LayoutJob;
    let key_size = 12.0;
    let cmd_size = key_size * 0.62;
    let mut job = LayoutJob::default();
    job.append(
        "\u{2318}",
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(cmd_size),
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.append(
        key,
        1.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(key_size),
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.into()
}

// ---------------------------------------------------------------------------
// ExtraWindow
// ---------------------------------------------------------------------------

pub struct ExtraWindow {
    pub viewport_id: egui::ViewportId,
    /// The initial viewport builder (set once, used every frame).
    pub(crate) viewport_builder: egui::ViewportBuilder,
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

    // Sidebar state
    show_left_sidebar: bool,
    show_right_sidebar: bool,
    sidebar_tab: SidebarTab,
    file_browser: FileBrowserState,
    session_panel_state: SessionPanelState,

    // Plugin sidebar (per-window)
    selected_plugin: Option<usize>,
    plugin_search_query: String,
    plugin_search_focus: bool,
    pending_plugin_loads: Vec<bool>,

    // Bottom panel
    show_bottom_panel: bool,
    active_bottom_panel: Option<usize>,
    bottom_panel_height: f32,

    // Rename tab dialog
    rename_tab_id: Option<Uuid>,
    rename_tab_buf: String,
    rename_tab_focus: bool,

    // Window focus tracking
    #[allow(dead_code)]
    window_focused: bool,

    // Actions for ConchApp to process after update()
    pub(crate) pending_actions: Vec<ExtraWindowAction>,
}

impl ExtraWindow {
    pub fn new(viewport_id: egui::ViewportId, viewport_builder: egui::ViewportBuilder, session: Session) -> Self {
        let id = session.id;
        let mut sessions = HashMap::new();
        sessions.insert(id, session);
        Self {
            viewport_id,
            viewport_builder,
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

            // Sidebars default to hidden in new extra windows.
            show_left_sidebar: false,
            show_right_sidebar: false,
            sidebar_tab: SidebarTab::default(),
            file_browser: FileBrowserState::default(),
            session_panel_state: SessionPanelState::default(),

            selected_plugin: None,
            plugin_search_query: String::new(),
            plugin_search_focus: false,
            pending_plugin_loads: Vec::new(),

            show_bottom_panel: false,
            active_bottom_panel: None,
            bottom_panel_height: 150.0,

            rename_tab_id: None,
            rename_tab_buf: String::new(),
            rename_tab_focus: false,

            window_focused: true,
            pending_actions: Vec::new(),
        }
    }

    fn active_session(&self) -> Option<&Session> {
        self.active_tab.and_then(|id| self.sessions.get(&id))
    }

    pub fn open_local_tab(&mut self, user_config: &config::UserConfig) {
        if let Some((id, session)) = create_local_session(user_config, None) {
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

    // -------------------------------------------------------------------
    // Sidebar action handling (file browser navigation is local)
    // -------------------------------------------------------------------

    fn handle_sidebar_action(&mut self, action: SidebarAction) {
        match action {
            SidebarAction::NavigateLocal(path) => {
                let old = self.file_browser.local_path.clone();
                self.file_browser.local_back_stack.push(old);
                self.file_browser.local_forward_stack.clear();
                self.file_browser.local_entries = load_local_entries(&path);
                self.file_browser.local_path_edit = path.to_string_lossy().into_owned();
                self.file_browser.local_path = path;
                self.file_browser.local_selected = None;
            }
            SidebarAction::GoBackLocal => {
                if let Some(prev) = self.file_browser.local_back_stack.pop() {
                    let current = self.file_browser.local_path.clone();
                    self.file_browser.local_forward_stack.push(current);
                    self.file_browser.local_entries = load_local_entries(&prev);
                    self.file_browser.local_path_edit = prev.to_string_lossy().into_owned();
                    self.file_browser.local_path = prev;
                    self.file_browser.local_selected = None;
                }
            }
            SidebarAction::GoForwardLocal => {
                if let Some(next) = self.file_browser.local_forward_stack.pop() {
                    let current = self.file_browser.local_path.clone();
                    self.file_browser.local_back_stack.push(current);
                    self.file_browser.local_entries = load_local_entries(&next);
                    self.file_browser.local_path_edit = next.to_string_lossy().into_owned();
                    self.file_browser.local_path = next;
                    self.file_browser.local_selected = None;
                }
            }
            SidebarAction::RefreshLocal => {
                let path = self.file_browser.local_path.clone();
                self.file_browser.local_entries = load_local_entries(&path);
                self.file_browser.local_selected = None;
            }
            SidebarAction::GoHomeLocal => {
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
                let old = self.file_browser.local_path.clone();
                self.file_browser.local_back_stack.push(old);
                self.file_browser.local_forward_stack.clear();
                self.file_browser.local_entries = load_local_entries(&home);
                self.file_browser.local_path_edit = home.to_string_lossy().into_owned();
                self.file_browser.local_path = home;
                self.file_browser.local_selected = None;
            }
            SidebarAction::SelectFile(path) => {
                log::info!("File selected: {}", path.display());
            }
            // Remote actions are not supported in extra windows (no SFTP).
            SidebarAction::NavigateRemote(_)
            | SidebarAction::GoBackRemote
            | SidebarAction::GoForwardRemote
            | SidebarAction::RefreshRemote
            | SidebarAction::GoHomeRemote
            | SidebarAction::Upload { .. }
            | SidebarAction::Download { .. }
            | SidebarAction::CancelTransfer(_) => {}
            // Plugin actions are routed to ConchApp.
            SidebarAction::RunPlugin(idx) => {
                self.pending_actions.push(ExtraWindowAction::RunPlugin(idx));
            }
            SidebarAction::RefreshPlugins => {
                self.pending_actions.push(ExtraWindowAction::RefreshPlugins);
            }
            SidebarAction::ApplyPluginChanges(indices) => {
                self.pending_actions
                    .push(ExtraWindowAction::ApplyPluginChanges(indices));
            }
            SidebarAction::PanelButtonClick {
                plugin_idx,
                button_id,
            } => {
                self.pending_actions
                    .push(ExtraWindowAction::PanelButtonClick {
                        plugin_idx,
                        button_id,
                    });
            }
            SidebarAction::DeactivatePanel(idx) => {
                self.pending_actions
                    .push(ExtraWindowAction::DeactivatePanel(idx));
            }
            SidebarAction::None => {}
        }
    }

    // -------------------------------------------------------------------
    // Sidebar toggle helpers
    // -------------------------------------------------------------------

    pub(crate) fn toggle_left_sidebar(&mut self) {
        self.show_left_sidebar = !self.show_left_sidebar;
        if !self.show_left_sidebar {
            self.file_browser.focused = false;
        }
    }

    pub(crate) fn toggle_right_sidebar(&mut self) {
        self.show_right_sidebar = !self.show_right_sidebar;
    }

    pub(crate) fn toggle_bottom_panel(&mut self, bottom_panel_tabs: &[usize]) {
        if self.show_bottom_panel {
            self.show_bottom_panel = false;
        } else if !bottom_panel_tabs.is_empty() {
            self.show_bottom_panel = true;
        }
    }

    fn toggle_zen_mode(&mut self, bottom_panel_tabs: &[usize]) {
        if self.show_left_sidebar || self.show_right_sidebar || self.show_bottom_panel {
            self.show_left_sidebar = false;
            self.show_right_sidebar = false;
            self.show_bottom_panel = false;
            self.file_browser.focused = false;
        } else {
            self.show_left_sidebar = true;
            self.show_right_sidebar = true;
            self.show_bottom_panel = !bottom_panel_tabs.is_empty();
        }
    }

    // -------------------------------------------------------------------
    // Main update
    // -------------------------------------------------------------------

    /// Render this window's content. Called from `show_viewport_immediate`.
    pub fn update(&mut self, ctx: &egui::Context, shared: &SharedState) {
        // Clear pending actions from previous frame.
        self.pending_actions.clear();

        // 1. Track OS-level focus for this window.
        self.is_focused = ctx.input(|i| i.focused);

        // 2. Measure cell size, re-measure when pixels_per_point changes.
        let ppp = ctx.pixels_per_point();
        if !self.cell_size_measured || self.last_pixels_per_point != ppp {
            let (cw, ch) = measure_cell_size(ctx, shared.user_config.font.size);
            let offset = &shared.user_config.font.offset;
            if cw > 0.0 && ch > 0.0 {
                self.cell_width = (cw + offset.x).max(1.0);
                self.cell_height = (ch + offset.y).max(1.0);
                self.cell_size_measured = true;
                self.last_pixels_per_point = ppp;
                self.last_cols = 0;
                self.last_rows = 0;
            }
        }

        // 3. Cursor blink.
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

        // 4. Poll terminal events.
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

        // 5. Close window if no sessions remain.
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

        // 6. Tab handling: intercept Tab key before egui consumes it.
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

        // 7. Collect copy/paste events.
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

        // ---------------------------------------------------------------
        // 8. Drag region (for Buttonless decorations)
        // ---------------------------------------------------------------
        let decorations = shared.user_config.window.decorations;
        if decorations == WindowDecorations::Buttonless {
            let drag_h = self.cell_height.max(6.0);
            egui::TopBottomPanel::top("extra_drag_region")
                .exact_height(drag_h)
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    let response =
                        ui.interact(rect, ui.id().with("extra_drag"), egui::Sense::drag());
                    if response.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                });
        }

        // ---------------------------------------------------------------
        // 9. Titlebar spacer (native menu on macOS with fullsize_content_view)
        // ---------------------------------------------------------------
        if shared.use_native_menu {
            let needs_spacer = match decorations {
                WindowDecorations::None | WindowDecorations::Buttonless => false,
                _ => cfg!(target_os = "macos"),
            };
            if needs_spacer {
                egui::TopBottomPanel::top("extra_titlebar_spacer")
                    .exact_height(28.0)
                    .frame(egui::Frame::NONE)
                    .show(ctx, |_ui| {});
            }
        }

        // ---------------------------------------------------------------
        // 10. In-window menu bar
        // ---------------------------------------------------------------
        if !shared.use_native_menu {
            let in_titlebar = cfg!(target_os = "macos");
            let top_pad: i8 = if in_titlebar { 7 } else { 4 };
            let bottom_pad: i8 = if in_titlebar { 6 } else { 4 };
            let left_pad: i8 = if in_titlebar { 72 } else { 8 };

            self.render_menu_bar(ctx, shared, in_titlebar, top_pad, bottom_pad, left_pad);
        }

        // ---------------------------------------------------------------
        // 11. Left sidebar
        // ---------------------------------------------------------------
        let sidebar_action = if self.show_left_sidebar {
            let panel_tabs: Vec<(usize, String)> = shared
                .panel_names
                .iter()
                .map(|(idx, name)| (*idx, name.clone()))
                .collect();
            sidebar::show_tab_strip(
                ctx,
                &mut self.sidebar_tab,
                shared.icon_cache.as_ref(),
                &panel_tabs,
                shared.plugin_icons,
            );
            sidebar::show_sidebar_content(
                ctx,
                &self.sidebar_tab,
                &mut self.file_browser,
                shared.icon_cache.as_ref(),
                shared.plugin_display,
                shared.plugin_output_lines,
                &mut self.selected_plugin,
                shared.transfers,
                &mut self.plugin_search_query,
                &mut self.plugin_search_focus,
                shared.panel_widgets,
                shared.panel_names,
                &mut self.pending_plugin_loads,
            )
        } else {
            SidebarAction::None
        };
        self.handle_sidebar_action(sidebar_action);

        // ---------------------------------------------------------------
        // 12. Right sidebar (session panel)
        // ---------------------------------------------------------------
        let mut panel_action = SessionPanelAction::None;
        if self.show_right_sidebar {
            let icons = shared.icon_cache.as_ref();
            egui::SidePanel::right("extra_right_sidebar")
                .resizable(true)
                .default_width(220.0)
                .width_range(100.0..=400.0)
                .show(ctx, |ui| {
                    panel_action = session_panel::show_session_panel(
                        ui,
                        &shared.sessions_config.folders,
                        shared.ssh_config_hosts,
                        icons,
                        &mut self.session_panel_state,
                    );
                });
        }
        // Escape in quick connect search bar.
        if self.session_panel_state.dismissed {
            self.session_panel_state.dismissed = false;
        }
        if !matches!(panel_action, SessionPanelAction::None) {
            self.pending_actions
                .push(ExtraWindowAction::SessionPanelAction(panel_action));
        }

        // ---------------------------------------------------------------
        // 13. Rename tab dialog
        // ---------------------------------------------------------------
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
                    if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        save = true;
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if crate::ui::widgets::dialog_button(ui, "Save").clicked() {
                                    save = true;
                                }
                                if crate::ui::widgets::dialog_button(ui, "Cancel").clicked() {
                                    cancel = true;
                                }
                            },
                        );
                    });
                });
            if save {
                let name = self.rename_tab_buf.trim().to_string();
                if let Some(session) = self.sessions.get_mut(&tab_id) {
                    session.custom_title = if name.is_empty() { None } else { Some(name) };
                }
                self.rename_tab_id = None;
                self.rename_tab_buf.clear();
            } else if cancel {
                self.rename_tab_id = None;
                self.rename_tab_buf.clear();
            }
        }

        // ---------------------------------------------------------------
        // 14. Tab bar (only when multiple tabs)
        // ---------------------------------------------------------------
        if self.tab_order.len() > 1 {
            self.render_tab_bar(ctx, shared.user_config, shared.icon_cache);
        }

        // ---------------------------------------------------------------
        // 15. Bottom panel
        // ---------------------------------------------------------------
        if self.show_bottom_panel && !shared.bottom_panel_tabs.is_empty() {
            let bp_action = bottom_panel::show_bottom_panel(
                ctx,
                shared.bottom_panel_tabs,
                &mut self.active_bottom_panel,
                shared.panel_widgets,
                shared.panel_names,
                &mut self.bottom_panel_height,
                &mut self.show_bottom_panel,
            );
            if !matches!(bp_action, BottomPanelAction::None) {
                self.pending_actions
                    .push(ExtraWindowAction::BottomPanelAction(bp_action));
            }
        }

        // ---------------------------------------------------------------
        // 16. Central panel (terminal)
        // ---------------------------------------------------------------
        let font_size = shared.user_config.font.size;
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
                            shared.colors,
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
                                shared.user_config.terminal.scroll_sensitivity,
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

        // ---------------------------------------------------------------
        // 17. Undo Tab focus cycling & forward to PTY
        // ---------------------------------------------------------------
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
                            term.mode()
                                .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
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

        // Drag-and-drop files -> paste paths into the terminal.
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
                                term.mode()
                                    .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
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

        // ---------------------------------------------------------------
        // 18. Keyboard input
        // ---------------------------------------------------------------
        self.handle_keyboard(ctx, forward_to_pty, shared);

        // ---------------------------------------------------------------
        // 19. Window title
        // ---------------------------------------------------------------
        let window_title = self
            .active_session()
            .map(|s| {
                let name = s.custom_title.as_ref().unwrap_or(&s.title);
                format!("{name} \u{2014} Conch")
            })
            .unwrap_or_else(|| "Conch".into());
        self.title = window_title.clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(window_title));
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    // -------------------------------------------------------------------
    // Menu bar rendering
    // -------------------------------------------------------------------

    fn render_menu_bar(
        &mut self,
        ctx: &egui::Context,
        shared: &SharedState,
        in_titlebar: bool,
        top_pad: i8,
        bottom_pad: i8,
        left_pad: i8,
    ) {
        egui::TopBottomPanel::top("extra_menu_bar")
            .frame(
                egui::Frame::side_top_panel(ctx.style().as_ref()).inner_margin(egui::Margin {
                    top: top_pad,
                    bottom: bottom_pad,
                    left: left_pad,
                    right: 8,
                }),
            )
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    if in_titlebar {
                        ui.label(
                            egui::RichText::new("Conch").color(ui.visuals().weak_text_color()),
                        );
                        ui.add_space(4.0);
                    }

                    // Menu buttons right-aligned (rendered right-to-left).
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            // Help
                            ui.menu_button("Help", |ui| {
                                if ui.button("About Conch").clicked() {
                                    self.pending_actions.push(ExtraWindowAction::OpenAbout);
                                    ui.close_menu();
                                }
                            });

                            // View
                            ui.menu_button("View", |ui| {
                                let left_check = if self.show_left_sidebar {
                                    "\u{2713} "
                                } else {
                                    "   "
                                };
                                if ui
                                    .add(
                                        egui::Button::new(format!("{left_check}Left Toolbar"))
                                            .shortcut_text(cmd_shortcut("1")),
                                    )
                                    .clicked()
                                {
                                    self.toggle_left_sidebar();
                                    ui.close_menu();
                                }
                                let right_check = if self.show_right_sidebar {
                                    "\u{2713} "
                                } else {
                                    "   "
                                };
                                if ui
                                    .add(
                                        egui::Button::new(format!("{right_check}Right Toolbar"))
                                            .shortcut_text(cmd_shortcut("2")),
                                    )
                                    .clicked()
                                {
                                    self.toggle_right_sidebar();
                                    ui.close_menu();
                                }
                                let bottom_check = if self.show_bottom_panel {
                                    "\u{2713} "
                                } else {
                                    "   "
                                };
                                if ui
                                    .add(
                                        egui::Button::new(format!("{bottom_check}Bottom Panel"))
                                            .shortcut_text(cmd_shortcut("J")),
                                    )
                                    .clicked()
                                {
                                    self.toggle_bottom_panel(shared.bottom_panel_tabs);
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("Notification History...").clicked() {
                                    self.pending_actions
                                        .push(ExtraWindowAction::OpenNotificationHistory);
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("Preferences...").clicked() {
                                    self.pending_actions
                                        .push(ExtraWindowAction::OpenPreferences);
                                    ui.close_menu();
                                }
                            });

                            // Tools
                            ui.menu_button("Tools", |ui| {
                                if ui.button("SSH Tunnels...").clicked() {
                                    self.pending_actions
                                        .push(ExtraWindowAction::OpenTunnelDialog);
                                    ui.close_menu();
                                }
                                // Show loaded action (non-panel) plugins.
                                let loaded_actions: Vec<(usize, String)> = shared
                                    .plugin_display
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, p)| !p.is_panel && !p.is_bottom_panel && p.is_loaded)
                                    .map(|(i, p)| (i, p.name.clone()))
                                    .collect();
                                if !loaded_actions.is_empty() {
                                    ui.separator();
                                    for (i, name) in &loaded_actions {
                                        if ui.button(name).clicked() {
                                            self.pending_actions
                                                .push(ExtraWindowAction::RunPlugin(*i));
                                            ui.close_menu();
                                        }
                                    }
                                }
                            });

                            // Sessions
                            ui.menu_button("Sessions", |ui| {
                                if ui
                                    .add(
                                        egui::Button::new("New Local Terminal")
                                            .shortcut_text(cmd_shortcut("T")),
                                    )
                                    .clicked()
                                {
                                    self.open_local_tab(shared.user_config);
                                    ui.close_menu();
                                }
                                if ui
                                    .add(
                                        egui::Button::new("New SSH Session...")
                                            .shortcut_text(cmd_shortcut("N")),
                                    )
                                    .clicked()
                                {
                                    self.pending_actions
                                        .push(ExtraWindowAction::OpenNewConnection);
                                    ui.close_menu();
                                }
                            });

                            // File
                            ui.menu_button("File", |ui| {
                                if ui
                                    .add(
                                        egui::Button::new("New Connection...")
                                            .shortcut_text(cmd_shortcut("N")),
                                    )
                                    .clicked()
                                {
                                    self.pending_actions
                                        .push(ExtraWindowAction::OpenNewConnection);
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui
                                    .add(
                                        egui::Button::new("Quit Conch")
                                            .shortcut_text(cmd_shortcut("Q")),
                                    )
                                    .clicked()
                                {
                                    self.pending_actions.push(ExtraWindowAction::QuitApp);
                                    ui.close_menu();
                                }
                            });
                        },
                    );
                });
            });
    }

    // -------------------------------------------------------------------
    // Tab bar rendering
    // -------------------------------------------------------------------

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
                let mut rename_id = None;
                let mut x = panel_rect.min.x;
                let tab_count = self.tab_order.len();
                let tab_w =
                    TAB_MAX_W.min(panel_rect.width() / (tab_count as f32 + 1.0));

                let hint_font = egui::FontId::new(9.0, egui::FontFamily::Proportional);

                for (tab_idx, &id) in self.tab_order.iter().enumerate() {
                    if let Some(session) = self.sessions.get(&id) {
                        let title = session
                            .custom_title
                            .as_deref()
                            .unwrap_or(&session.title);
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

                        // Separator between tabs.
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

                        // Close button area.
                        let close_size = 14.0;
                        let close_pad = 4.0;
                        let close_x = tab_rect.max.x - close_size - close_pad;
                        let close_y = tab_rect.center().y - close_size / 2.0;
                        let close_rect = egui::Rect::from_min_size(
                            egui::Pos2::new(close_x - 2.0, close_y - 2.0),
                            egui::Vec2::new(close_size + 4.0, close_size + 4.0),
                        );
                        if let Some(tex_id) = icon_cache
                            .as_ref()
                            .and_then(|ic| ic.texture_id(Icon::TabClose))
                        {
                            painter.image(
                                tex_id,
                                egui::Rect::from_min_size(
                                    egui::Pos2::new(close_x, close_y),
                                    egui::Vec2::new(close_size, close_size),
                                ),
                                egui::Rect::from_min_max(
                                    egui::Pos2::ZERO,
                                    egui::Pos2::new(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        }

                        // Tab number hint.
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

                        // Tab label with pixel-based truncation.
                        let label_color = if selected { text_color } else { dim_text };
                        let left_pad = 6.0;
                        let max_text_w = hint_x - tab_rect.min.x - left_pad - 2.0;

                        let full_galley = painter.layout_no_wrap(
                            title.to_string(),
                            font_id.clone(),
                            label_color,
                        );

                        let galley =
                            if full_galley.size().x > max_text_w && max_text_w > 0.0 {
                                let ellipsis_w = painter
                                    .layout_no_wrap(
                                        "\u{2026}".to_string(),
                                        font_id.clone(),
                                        label_color,
                                    )
                                    .size()
                                    .x;
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
                                    end =
                                        truncated.floor_char_boundary(end.saturating_sub(1));
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
                    if let Some(session) = self.sessions.get(&id) {
                        self.rename_tab_buf = session
                            .custom_title
                            .clone()
                            .unwrap_or_else(|| session.title.clone());
                        self.rename_tab_id = Some(id);
                        self.rename_tab_focus = true;
                    }
                }

                // "+" button.
                let plus_rect = egui::Rect::from_min_size(
                    egui::Pos2::new(x + 4.0, panel_rect.min.y),
                    egui::Vec2::new(24.0, tab_h),
                );
                let plus_galley =
                    painter.layout_no_wrap("+".to_string(), font_id, dim_text);
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
                    ui.id().with("extra_tab_plus"),
                    egui::Sense::click(),
                );
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

    // -------------------------------------------------------------------
    // Keyboard handling
    // -------------------------------------------------------------------

    fn handle_keyboard(
        &mut self,
        ctx: &egui::Context,
        forward_to_pty: bool,
        shared: &SharedState,
    ) {
        use alacritty_terminal::term::TermMode;

        let app_cursor = forward_to_pty
            && self.active_session().map_or(false, |s| {
                s.backend
                    .term()
                    .try_lock_unfair()
                    .map_or(false, |term| term.mode().contains(TermMode::APP_CURSOR))
            });

        let shortcuts = shared.shortcuts;
        let user_config = shared.user_config;
        let bottom_panel_tabs = shared.bottom_panel_tabs;

        // Collect keyboard actions into a vec to avoid borrow issues.
        // We process simple state changes inline via closures, but sidebar
        // toggles / action pushes happen after the input closure.
        enum KbAction {
            SwitchTab(usize),
            NewWindow,
            NewTab,
            CloseTab,
            ToggleLeftSidebar,
            ToggleRightSidebar,
            NewConnection,
            FocusQuickConnect,
            FocusPluginSearch,
            FocusFiles,
            ZenMode,
            SshTunnels,
            NotificationHistory,
            ToggleBottomPanel,
            Quit,
            WriteBytes(Vec<u8>),
            WriteText(String),
        }

        let mut actions: Vec<KbAction> = Vec::new();

        ctx.input(|input_state| {
            for event in &input_state.events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        // Cmd+number -> switch tab.
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
                                actions.push(KbAction::SwitchTab(idx));
                                return;
                            }
                        }

                        // App-level configurable shortcuts.
                        if let Some(ref kb) = shortcuts.new_window {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::NewWindow);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.new_tab {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::NewTab);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.close_tab {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::CloseTab);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.toggle_left_sidebar {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::ToggleLeftSidebar);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.toggle_right_sidebar {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::ToggleRightSidebar);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.new_connection {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::NewConnection);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.focus_quick_connect {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::FocusQuickConnect);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.focus_plugin_search {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::FocusPluginSearch);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.focus_files {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::FocusFiles);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.zen_mode {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::ZenMode);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.ssh_tunnels {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::SshTunnels);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.notification_history {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::NotificationHistory);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.toggle_bottom_panel {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::ToggleBottomPanel);
                                return;
                            }
                        }
                        if let Some(ref kb) = shortcuts.quit {
                            if kb.matches(key, modifiers) {
                                actions.push(KbAction::Quit);
                                return;
                            }
                        }

                        // On Linux/Windows, Ctrl+Shift+C copies terminal selection.
                        #[cfg(not(target_os = "macos"))]
                        if forward_to_pty
                            && modifiers.ctrl
                            && modifiers.shift
                            && *key == egui::Key::C
                        {
                            // Copy handled below via action.
                            return;
                        }

                        // Forward to PTY.
                        if forward_to_pty {
                            if let Some(bytes) = input::key_to_bytes(
                                key, modifiers, None, shortcuts, app_cursor,
                            ) {
                                actions.push(KbAction::WriteBytes(bytes));
                            }
                        }
                    }
                    egui::Event::Text(text) => {
                        if forward_to_pty {
                            actions.push(KbAction::WriteText(text.clone()));
                        }
                    }
                    _ => {}
                }
            }
        });

        // Process collected actions.
        for action in actions {
            match action {
                KbAction::SwitchTab(idx) => {
                    if let Some(&id) = self.tab_order.get(idx) {
                        self.active_tab = Some(id);
                    }
                }
                KbAction::NewWindow => {
                    self.pending_actions.push(ExtraWindowAction::SpawnNewWindow);
                }
                KbAction::NewTab => {
                    self.open_local_tab(user_config);
                }
                KbAction::CloseTab => {
                    if let Some(id) = self.active_tab {
                        self.remove_session(id);
                        if self.sessions.is_empty() {
                            self.open_local_tab(user_config);
                        }
                    }
                }
                KbAction::ToggleLeftSidebar => {
                    self.toggle_left_sidebar();
                }
                KbAction::ToggleRightSidebar => {
                    self.toggle_right_sidebar();
                }
                KbAction::NewConnection => {
                    self.pending_actions
                        .push(ExtraWindowAction::OpenNewConnection);
                }
                KbAction::FocusQuickConnect => {
                    if !self.show_right_sidebar {
                        self.show_right_sidebar = true;
                    }
                    self.session_panel_state.quick_connect_focus = true;
                }
                KbAction::FocusPluginSearch => {
                    self.show_left_sidebar = true;
                    self.sidebar_tab = SidebarTab::Plugins;
                    self.plugin_search_focus = true;
                }
                KbAction::FocusFiles => {
                    if self.file_browser.focused {
                        self.file_browser.focused = false;
                    } else {
                        if !self.show_left_sidebar {
                            self.show_left_sidebar = true;
                        }
                        self.sidebar_tab = SidebarTab::Files;
                        self.file_browser.focused = true;
                        if self.file_browser.local_selected.is_none()
                            && !self.file_browser.local_entries.is_empty()
                        {
                            self.file_browser.local_selected = Some(0);
                        }
                    }
                }
                KbAction::ZenMode => {
                    self.toggle_zen_mode(bottom_panel_tabs);
                }
                KbAction::SshTunnels => {
                    self.pending_actions
                        .push(ExtraWindowAction::OpenTunnelDialog);
                }
                KbAction::NotificationHistory => {
                    self.pending_actions
                        .push(ExtraWindowAction::OpenNotificationHistory);
                }
                KbAction::ToggleBottomPanel => {
                    self.toggle_bottom_panel(bottom_panel_tabs);
                }
                KbAction::Quit => {
                    self.should_close = true;
                }
                KbAction::WriteBytes(bytes) => {
                    if let Some(session) = self.active_session() {
                        session.backend.write(&bytes);
                    }
                }
                KbAction::WriteText(text) => {
                    if let Some(session) = self.active_session() {
                        session.backend.write(text.as_bytes());
                    }
                }
            }
        }
    }
}
