//! Window and app cleanup handlers.
//!
//! When a window is destroyed, all PTY sessions and SSH sessions belonging to
//! that window must be cleaned up. This module provides the logic for
//! identifying and removing sessions keyed by window label.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use conch_remote::ssh::ChannelInput;

use crate::pty_backend::PtyBackend;
use crate::remote::RemoteState;

/// Collect all keys from a HashMap that belong to the given window label.
///
/// Session keys use the format `"{window_label}:{pane_id}"`, so any key that
/// starts with `"{label}:"` belongs to that window.
pub(crate) fn keys_for_window(keys: &[String], label: &str) -> Vec<String> {
    let prefix = format!("{label}:");
    keys.iter()
        .filter(|k| k.starts_with(&prefix))
        .cloned()
        .collect()
}

/// Remove all PTY sessions belonging to the given window label.
///
/// Dropping a `PtyBackend` closes the underlying PTY file descriptors, which
/// causes the shell process to receive SIGHUP and exit.
///
/// Returns the number of PTY sessions removed.
pub(crate) fn cleanup_ptys(
    ptys: &Arc<Mutex<HashMap<String, PtyBackend>>>,
    window_label: &str,
) -> usize {
    let mut guard = ptys.lock();
    let all_keys: Vec<String> = guard.keys().cloned().collect();
    let matching = keys_for_window(&all_keys, window_label);
    let count = matching.len();
    for key in &matching {
        guard.remove(key);
        log::info!("Cleaned up PTY session: {key}");
    }
    count
}

/// Remove all SSH sessions belonging to the given window label.
///
/// For each session:
/// 1. Send `ChannelInput::Shutdown` to signal the channel loop to exit
/// 2. Abort the channel loop task via its `AbortHandle`
/// 3. Decrement the connection's `ref_count` and remove it if it reaches 0
///
/// Returns the number of SSH sessions removed.
pub(crate) fn cleanup_ssh_sessions(remote: &Arc<Mutex<RemoteState>>, window_label: &str) -> usize {
    let mut state = remote.lock();
    let all_keys: Vec<String> = state.sessions.keys().cloned().collect();
    let matching = keys_for_window(&all_keys, window_label);
    let count = matching.len();

    // Collect sessions to clean up, then process them.
    let sessions: Vec<(String, crate::remote::SshSession)> = matching
        .into_iter()
        .filter_map(|k| state.sessions.remove(&k).map(|s| (k, s)))
        .collect();

    for (key, session) in sessions {
        // Signal the channel loop to shut down.
        let _ = session.input_tx.send(ChannelInput::Shutdown);

        // Abort the spawned channel loop task.
        if let Some(handle) = &session.abort_handle {
            handle.abort();
        }

        // Decrement connection ref count.
        if let Some(conn) = state.connections.get_mut(&session.connection_id) {
            conn.ref_count = conn.ref_count.saturating_sub(1);
            if conn.ref_count == 0 {
                state.connections.remove(&session.connection_id);
                log::info!(
                    "Removed SSH connection {} (no remaining sessions)",
                    session.connection_id
                );
            }
        }

        log::info!(
            "Cleaned up SSH session: {key} ({}@{}:{})",
            session.user,
            session.host,
            session.port,
        );
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_for_window_filters_by_label() {
        let keys = vec![
            "main:0".to_string(),
            "main:1".to_string(),
            "window-2:0".to_string(),
            "window-2:1".to_string(),
            "main:5".to_string(),
        ];
        let result = keys_for_window(&keys, "main");
        assert_eq!(result.len(), 3);
        assert!(result.contains(&"main:0".to_string()));
        assert!(result.contains(&"main:1".to_string()));
        assert!(result.contains(&"main:5".to_string()));
    }

    #[test]
    fn keys_for_window_returns_empty_when_no_match() {
        let keys = vec!["main:0".to_string(), "main:1".to_string()];
        let result = keys_for_window(&keys, "window-3");
        assert!(result.is_empty());
    }

    #[test]
    fn keys_for_window_empty_input() {
        let keys: Vec<String> = vec![];
        let result = keys_for_window(&keys, "main");
        assert!(result.is_empty());
    }

    #[test]
    fn keys_for_window_does_not_match_partial_label() {
        // "main" should NOT match "main-alt:0"
        let keys = vec!["main:0".to_string(), "main-alt:0".to_string()];
        let result = keys_for_window(&keys, "main");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "main:0");
    }

    #[test]
    fn keys_for_window_numeric_window_ids() {
        let keys = vec![
            "window-1:0".to_string(),
            "window-1:1".to_string(),
            "window-10:0".to_string(),
            "window-12:0".to_string(),
        ];
        // "window-1" should only match "window-1:*", not "window-10:*"
        let result = keys_for_window(&keys, "window-1");
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"window-1:0".to_string()));
        assert!(result.contains(&"window-1:1".to_string()));
    }

    #[test]
    fn cleanup_ptys_on_empty_map() {
        let ptys: Arc<Mutex<HashMap<String, PtyBackend>>> = Arc::new(Mutex::new(HashMap::new()));
        let count = cleanup_ptys(&ptys, "main");
        assert_eq!(count, 0);
    }

    /// Test that SSH session cleanup removes only the target window's sessions
    /// and sends shutdown signals. We skip connections because constructing a
    /// real `russh::client::Handle` requires an active SSH connection.
    #[test]
    fn cleanup_ssh_sessions_removes_matching_sessions() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let remote = Arc::new(Mutex::new(RemoteState {
            sessions: HashMap::new(),
            connections: HashMap::new(),
            config: conch_remote::config::SshConfig::default(),
            ssh_config_entries: vec![],
            pending_prompts: Arc::new(Mutex::new(crate::remote::PendingPrompts::new())),
            tunnel_manager: conch_remote::tunnel::TunnelManager::new(),
            transfers: Arc::new(Mutex::new(conch_remote::transfer::TransferRegistry::new())),
            transfer_progress_tx: tx,
            paths: conch_remote::callbacks::RemotePaths {
                known_hosts_file: std::path::PathBuf::from("/tmp/test"),
                config_dir: std::path::PathBuf::from("/tmp/test"),
                default_key_paths: vec![],
            },
        }));

        // Insert test sessions (no connections — they require a real SSH handle).
        let (input_tx1, mut input_rx1) = tokio::sync::mpsc::unbounded_channel();
        let (input_tx2, mut input_rx2) = tokio::sync::mpsc::unbounded_channel();
        let (input_tx3, _input_rx3) = tokio::sync::mpsc::unbounded_channel();

        {
            let mut state = remote.lock();
            state.sessions.insert(
                "win1:0".to_string(),
                crate::remote::SshSession {
                    input_tx: input_tx1,
                    connection_id: "conn:win1:0".to_string(),
                    host: "host1.example.com".to_string(),
                    user: "alice".to_string(),
                    port: 22,
                    abort_handle: None,
                },
            );
            state.sessions.insert(
                "win1:1".to_string(),
                crate::remote::SshSession {
                    input_tx: input_tx2,
                    connection_id: "conn:win1:0".to_string(),
                    host: "host1.example.com".to_string(),
                    user: "alice".to_string(),
                    port: 22,
                    abort_handle: None,
                },
            );
            state.sessions.insert(
                "win2:0".to_string(),
                crate::remote::SshSession {
                    input_tx: input_tx3,
                    connection_id: "conn:win2:0".to_string(),
                    host: "host2.example.com".to_string(),
                    user: "bob".to_string(),
                    port: 22,
                    abort_handle: None,
                },
            );
        }

        // Clean up win1 sessions.
        let count = cleanup_ssh_sessions(&remote, "win1");
        assert_eq!(count, 2, "should remove 2 sessions for win1");

        let state = remote.lock();
        assert_eq!(state.sessions.len(), 1, "only win2 session should remain");
        assert!(state.sessions.contains_key("win2:0"));
        drop(state);

        // Verify shutdown signals were sent.
        let msg1 = input_rx1.try_recv();
        assert!(
            matches!(msg1, Ok(ChannelInput::Shutdown)),
            "shutdown should be sent to win1:0"
        );
        let msg2 = input_rx2.try_recv();
        assert!(
            matches!(msg2, Ok(ChannelInput::Shutdown)),
            "shutdown should be sent to win1:1"
        );
    }

    #[test]
    fn cleanup_ssh_sessions_noop_when_no_matching_sessions() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let remote = Arc::new(Mutex::new(RemoteState {
            sessions: HashMap::new(),
            connections: HashMap::new(),
            config: conch_remote::config::SshConfig::default(),
            ssh_config_entries: vec![],
            pending_prompts: Arc::new(Mutex::new(crate::remote::PendingPrompts::new())),
            tunnel_manager: conch_remote::tunnel::TunnelManager::new(),
            transfers: Arc::new(Mutex::new(conch_remote::transfer::TransferRegistry::new())),
            transfer_progress_tx: tx,
            paths: conch_remote::callbacks::RemotePaths {
                known_hosts_file: std::path::PathBuf::from("/tmp/test"),
                config_dir: std::path::PathBuf::from("/tmp/test"),
                default_key_paths: vec![],
            },
        }));

        let count = cleanup_ssh_sessions(&remote, "nonexistent");
        assert_eq!(count, 0);
    }
}
