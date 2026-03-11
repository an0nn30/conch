//! Session lifecycle: creation, removal, resizing, and terminal configuration.

use conch_core::config;
use conch_pty::LocalSession;
use uuid::Uuid;

use crate::app::ConchApp;
use crate::state::{AppState, Session};

/// Default initial terminal dimensions (before font metrics are measured).
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

impl ConchApp {
    /// Close a session with tab close animation and activate the previous tab.
    pub(crate) fn remove_session(&mut self, id: Uuid) {
        let title = self.state.sessions.get(&id)
            .map(|s| s.display_title().to_string())
            .unwrap_or_default();
        let index = self.state.tab_order.iter().position(|&t| t == id).unwrap_or(0);
        self.tab_bar_state.begin_close(id, title, index);

        if let Some(session) = self.state.sessions.remove(&id) {
            session.pty.shutdown();
        }
        self.state.tab_order.retain(|&tab_id| tab_id != id);
        if self.state.active_tab == Some(id) {
            self.state.active_tab = self.state.tab_order.last().copied();
        }
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
            session.resize(cols, rows, cw, ch);
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
                pty: local,
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
) -> Option<Uuid> {
    let cwd = state
        .active_session()
        .map(|s| s.pty.child_pid())
        .and_then(conch_pty::get_cwd_of_pid);
    let (id, session) = create_local_session(&state.user_config, cwd)?;
    if last_cols > 0 && last_rows > 0 {
        session.resize(last_cols, last_rows, cell_width as u16, cell_height as u16);
    }
    state.sessions.insert(id, session);
    state.tab_order.push(id);
    state.active_tab = Some(id);
    Some(id)
}
