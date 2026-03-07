//! Application state: sessions, tabs, and UI state.

use std::collections::HashMap;

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use conch_core::color_scheme;
use conch_core::config::{PersistentState, SessionsConfig, UserConfig};
use conch_core::models::ServerEntry;
use conch_session::{EventProxy, LocalSession, SshSession};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::terminal::color::ResolvedColors;
use crate::ui::dialogs::new_connection::NewConnectionForm;
use crate::ui::file_browser::FileBrowserState;
use crate::ui::sidebar::SidebarTab;

/// Backend type for a session -- either local PTY or SSH.
pub enum SessionBackend {
    Local(LocalSession),
    Ssh(SshSession),
}

impl SessionBackend {
    pub fn write(&self, data: &[u8]) {
        match self {
            Self::Local(s) => s.write(data),
            Self::Ssh(s) => s.write(data),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16, cell_width: u16, cell_height: u16) {
        match self {
            Self::Local(s) => s.resize(cols, rows, cell_width, cell_height),
            Self::Ssh(s) => s.resize(cols, rows, cell_width, cell_height),
        }
    }

    pub fn shutdown(&self) {
        match self {
            Self::Local(s) => s.shutdown(),
            Self::Ssh(s) => s.shutdown(),
        }
    }

    pub fn term(&self) -> &Arc<FairMutex<Term<EventProxy>>> {
        match self {
            Self::Local(s) => &s.term,
            Self::Ssh(s) => &s.term,
        }
    }

    /// Get the child process PID (only meaningful for local sessions).
    pub fn child_pid(&self) -> Option<u32> {
        match self {
            Self::Local(s) => Some(s.child_pid()),
            Self::Ssh(_) => None,
        }
    }

}

/// A single terminal session (local or SSH).
pub struct Session {
    pub id: Uuid,
    pub title: String,
    /// User-set custom title (overrides `title` for display when `Some`).
    pub custom_title: Option<String>,
    pub backend: SessionBackend,
    pub event_rx: mpsc::UnboundedReceiver<TermEvent>,
}

/// The full application state.
pub struct AppState {
    pub user_config: UserConfig,
    pub persistent: PersistentState,
    pub sessions_config: SessionsConfig,
    pub colors: ResolvedColors,
    pub sessions: HashMap<Uuid, Session>,
    pub active_tab: Option<Uuid>,
    pub tab_order: Vec<Uuid>,
    pub ssh_config_hosts: Vec<ServerEntry>,
    pub sidebar_tab: SidebarTab,
    pub new_connection_form: Option<NewConnectionForm>,
    /// When editing an existing server, stores its address for in-place update.
    pub editing_server_addr: Option<crate::ui::session_panel::ServerAddress>,
    pub file_browser: FileBrowserState,
    pub show_left_sidebar: bool,
    pub show_right_sidebar: bool,
}

impl AppState {
    pub fn new(user_config: UserConfig, persistent: PersistentState, sessions_config: SessionsConfig) -> Self {
        let show_left_sidebar = !persistent.layout.left_panel_collapsed;
        let show_right_sidebar = !persistent.layout.right_panel_collapsed;

        let scheme = color_scheme::resolve_theme(&user_config.colors.theme);
        let colors = ResolvedColors::from_scheme(&scheme);

        Self {
            user_config,
            persistent,
            sessions_config,
            colors,
            sessions: HashMap::new(),
            active_tab: None,
            tab_order: Vec::new(),
            ssh_config_hosts: Vec::new(),
            sidebar_tab: SidebarTab::default(),
            new_connection_form: None,
            editing_server_addr: None,
            file_browser: FileBrowserState::default(),
            show_left_sidebar,
            show_right_sidebar,
        }
    }

    /// Get the currently active session, if any.
    pub fn active_session(&self) -> Option<&Session> {
        self.active_tab.and_then(|id| self.sessions.get(&id))
    }
}
