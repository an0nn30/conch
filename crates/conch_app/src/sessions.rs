//! Session lifecycle: creation, removal, resizing, and terminal configuration.

use conch_core::config;
use conch_session::{LocalSession, SftpCmd};
use uuid::Uuid;

use crate::app::{ConchApp, DEFAULT_COLS, DEFAULT_ROWS};
use crate::extra_window::ExtraWindow;
use crate::state::{AppState, Session, SessionBackend};
use crate::ui::file_browser::FileListEntry;

impl ConchApp {
    /// Close a session and activate the previous tab.
    pub(crate) fn remove_session(&mut self, id: Uuid) {
        if let Some(session) = self.state.sessions.remove(&id) {
            session.backend.shutdown();
        }
        self.state.tab_order.retain(|&tab_id| tab_id != id);
        if self.state.active_tab == Some(id) {
            self.state.active_tab = self.state.tab_order.last().copied();
        }

        // Shut down SFTP worker if it belonged to this session.
        if self.sftp_session_id == Some(id) {
            if let Some(tx) = self.sftp_cmd_tx.take() {
                let _ = tx.send(SftpCmd::Shutdown);
            }
            self.sftp_result_rx = None;
            self.sftp_session_id = None;
            self.remote_home = None;
            self.state.file_browser.remote_path = None;
            self.state.file_browser.remote_entries.clear();
            self.transfers.clear();
        }

        // Clean up pending connection info if closing a connecting tab.
        self.pending_ssh_info.remove(&id);
    }

    /// Open a new local terminal tab.
    pub(crate) fn open_local_tab(&mut self) {
        let _ = open_local_terminal(
            &mut self.state,
            self.last_cols,
            self.last_rows,
            self.cell_width,
            self.cell_height,
        );
    }

    /// Build a `ViewportBuilder` for extra windows that matches the main window's
    /// decoration config.
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
            .and_then(|s| s.backend.child_pid())
            .and_then(conch_session::get_cwd_of_pid);
        let Some((_, session)) = create_local_session(&self.state.user_config, cwd) else {
            return;
        };
        let num = self.next_viewport_num;
        self.next_viewport_num += 1;
        let viewport_id = egui::ViewportId::from_hash_of(format!("conch_window_{num}"));
        let builder = self.build_extra_viewport();
        self.extra_windows.push(ExtraWindow::new(viewport_id, builder, session));
    }

    /// Resize all sessions if the computed grid dimensions changed.
    pub(crate) fn resize_sessions(&mut self, cols: u16, rows: u16) {
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

/// Build an `alacritty_terminal::term::Config` from the user's cursor settings.
pub(crate) fn build_term_config(cfg: &config::CursorConfig) -> alacritty_terminal::term::Config {
    use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};

    fn parse_style(s: &config::CursorStyleConfig) -> CursorStyle {
        let shape = match s.shape.to_lowercase().as_str() {
            "underline" => CursorShape::Underline,
            "beam" | "ibeam" => CursorShape::Beam,
            _ => CursorShape::Block,
        };
        CursorStyle { shape, blinking: s.blinking }
    }

    let mut tc = alacritty_terminal::term::Config::default();
    tc.default_cursor_style = parse_style(&cfg.style);
    tc.vi_mode_cursor_style = cfg.vi_mode_style.as_ref().map(parse_style);
    tc
}

/// Create a local terminal session without inserting it into any state.
pub(crate) fn create_local_session(
    user_config: &config::UserConfig,
    working_directory: Option<std::path::PathBuf>,
) -> Option<(Uuid, Session)> {
    let id = Uuid::new_v4();
    let shell_cfg = &user_config.terminal.shell;
    let shell = if shell_cfg.program.is_empty() {
        None
    } else {
        Some(alacritty_terminal::tty::Shell::new(
            shell_cfg.program.clone(),
            shell_cfg.args.clone(),
        ))
    };
    let term_config = build_term_config(&user_config.terminal.cursor);
    match LocalSession::new(
        DEFAULT_COLS, DEFAULT_ROWS, 8, 16,
        shell, &user_config.terminal.env, term_config,
        working_directory,
    ) {
        Ok(mut local) => {
            let event_rx = local.take_event_rx();
            let session = Session {
                id,
                title: "Local".into(),
                custom_title: None,
                backend: SessionBackend::Local(local),
                event_rx,
            };
            Some((id, session))
        }
        Err(e) => {
            log::error!("Failed to open local terminal: {e:#}");
            None
        }
    }
}

pub(crate) fn open_local_terminal(
    state: &mut AppState,
    last_cols: u16,
    last_rows: u16,
    cell_width: f32,
    cell_height: f32,
) -> Option<(Uuid, u32)> {
    let cwd = state
        .active_session()
        .and_then(|s| s.backend.child_pid())
        .and_then(conch_session::get_cwd_of_pid);
    let (id, session) = create_local_session(&state.user_config, cwd)?;
    let child_pid = match &session.backend {
        SessionBackend::Local(local) => local.child_pid(),
        _ => 0,
    };
    if last_cols > 0 && last_rows > 0 {
        session.backend.resize(last_cols, last_rows, cell_width as u16, cell_height as u16);
    }
    state.sessions.insert(id, session);
    state.tab_order.push(id);
    state.active_tab = Some(id);
    Some((id, child_pid))
}

/// Load and sort local directory entries for the file browser.
pub(crate) fn load_local_entries(path: &std::path::Path) -> Vec<FileListEntry> {
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
