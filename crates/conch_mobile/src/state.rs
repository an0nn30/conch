//! App state for Conch Mobile — SSH sessions, config, auth prompts.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;

use conch_remote::callbacks::RemotePaths;
use conch_remote::config::SshConfig;
use conch_remote::handler::ConchSshHandler;
use conch_remote::ssh::ChannelInput;

/// A live SSH session.
pub struct SshSession {
    pub input_tx: mpsc::UnboundedSender<ChannelInput>,
    pub ssh_handle: Arc<conch_remote::russh::client::Handle<ConchSshHandler>>,
    pub host: String,
    pub user: String,
    pub port: u16,
}

/// Pending auth prompts waiting for frontend responses.
pub struct PendingPrompts {
    pub host_key: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    pub password: HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>,
}

impl PendingPrompts {
    pub fn new() -> Self {
        Self {
            host_key: HashMap::new(),
            password: HashMap::new(),
        }
    }
}

/// Shared state for all remote operations.
pub struct MobileState {
    /// SSH sessions keyed by session ID (e.g., "session-0", "session-1").
    pub sessions: HashMap<String, SshSession>,
    /// Next session ID counter.
    pub next_session_id: u32,
    /// Server configuration.
    pub config: SshConfig,
    /// Pending auth prompts.
    pub pending_prompts: Arc<Mutex<PendingPrompts>>,
    /// Platform-specific paths.
    pub paths: RemotePaths,
}

impl MobileState {
    pub fn new() -> Self {
        let paths = mobile_remote_paths();
        let config = conch_remote::config::load_config(&paths.config_dir);
        Self {
            sessions: HashMap::new(),
            next_session_id: 0,
            config,
            pending_prompts: Arc::new(Mutex::new(PendingPrompts::new())),
            paths,
        }
    }

    /// Allocate a new session ID.
    pub fn alloc_session_id(&mut self) -> String {
        let id = format!("session-{}", self.next_session_id);
        self.next_session_id += 1;
        id
    }
}

/// Build `RemotePaths` for iOS.
///
/// On iOS, there is no `~/.ssh/` directory. Keys are imported via
/// the iOS Files app and stored in the app's documents directory.
/// Known hosts are stored in the app's config directory.
fn mobile_remote_paths() -> RemotePaths {
    // On iOS, use the app's documents/config directory.
    // For now, use dirs::config_dir() which maps to the app sandbox.
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("conch")
        .join("remote");
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
        .join("conch");

    RemotePaths {
        known_hosts_file: data_dir.join("known_hosts"),
        config_dir,
        // No default key paths on iOS — keys come from explicit import.
        default_key_paths: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_state_new_has_no_sessions() {
        let state = MobileState::new();
        assert!(state.sessions.is_empty());
        assert_eq!(state.next_session_id, 0);
    }

    #[test]
    fn alloc_session_id_increments() {
        let mut state = MobileState::new();
        assert_eq!(state.alloc_session_id(), "session-0");
        assert_eq!(state.alloc_session_id(), "session-1");
        assert_eq!(state.alloc_session_id(), "session-2");
    }

    #[test]
    fn pending_prompts_new_is_empty() {
        let p = PendingPrompts::new();
        assert!(p.host_key.is_empty());
        assert!(p.password.is_empty());
    }

    #[test]
    fn mobile_remote_paths_has_empty_key_paths() {
        let paths = mobile_remote_paths();
        assert!(paths.default_key_paths.is_empty());
    }
}
