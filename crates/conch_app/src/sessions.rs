//! Session lifecycle: creation, removal, resizing, and terminal configuration.

use conch_core::config;
use uuid::Uuid;

use crate::state::Session;

/// Default initial terminal dimensions (before font metrics are measured).
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

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
    create_session_inner(user_config, working_directory, false)
}

/// Create a local terminal session with a plain default shell,
/// ignoring the `terminal.shell` config. Used by plugins that need
/// a clean shell (e.g., to attach to tmux without nesting).
pub(crate) fn create_plain_session(
    user_config: &config::UserConfig,
    working_directory: Option<std::path::PathBuf>,
) -> Option<(Uuid, Session)> {
    create_session_inner(user_config, working_directory, true)
}

fn create_session_inner(
    user_config: &config::UserConfig,
    working_directory: Option<std::path::PathBuf>,
    plain_shell: bool,
) -> Option<(Uuid, Session)> {
    let id = Uuid::new_v4();
    let shell = if plain_shell {
        None // Use OS default shell
    } else {
        let shell_cfg = &user_config.terminal.shell;
        if shell_cfg.program.is_empty() {
            None
        } else {
            Some(alacritty_terminal::tty::Shell::new(
                shell_cfg.program.clone(),
                shell_cfg.args.clone(),
            ))
        }
    };
    let term_config = build_term_config(&user_config.terminal.cursor);
    match conch_pty::LocalSession::new(
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
                backend: crate::state::SessionBackend::Local(local),
                event_rx,
                status: conch_plugin_sdk::SessionStatus::Connected,
                status_detail: None,
                connect_started: None,
                prompt: None,
            };
            Some((id, session))
        }
        Err(e) => {
            log::error!("Failed to open local terminal: {e:#}");
            None
        }
    }
}
