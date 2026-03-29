//! Remote module — unified SSH connections, SFTP, and file operations.
//!
//! Exposes Tauri commands for SSH session lifecycle. The frontend sees
//! the same `pty-output` / `pty-exit` events as local PTY tabs — xterm.js
//! doesn't care whether bytes come from a local shell or an SSH channel.
//!
//! All SSH/SFTP/transfer/tunnel logic is delegated to `conch_remote`.
//! This module provides the Tauri command wrappers and the
//! `TauriRemoteCallbacks` implementation that bridges `RemoteCallbacks`
//! to Tauri events + oneshot prompt channels.

pub(crate) mod auth;
pub(crate) mod local_fs;
pub(crate) mod server_commands;
pub(crate) mod sftp_commands;
pub(crate) mod ssh_commands;
pub(crate) mod transfer_commands;
pub(crate) mod tunnel_commands;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::mpsc;

use conch_remote::callbacks::{RemoteCallbacks, RemotePaths};
use conch_remote::config::{ServerEntry, SshConfig};
use conch_remote::handler::ConchSshHandler;
use conch_remote::ssh::{ChannelInput, SshCredentials};
use conch_remote::transfer::{TransferProgress, TransferRegistry};
use conch_remote::tunnel::TunnelManager;

use crate::pty::{PtyExitEvent, PtyOutputEvent};

// ---------------------------------------------------------------------------
// TauriRemoteCallbacks — bridges RemoteCallbacks to Tauri events
// ---------------------------------------------------------------------------

/// Bridges `conch_remote::callbacks::RemoteCallbacks` to Tauri events and
/// oneshot prompt channels. When the SSH handler needs user interaction
/// (host key confirmation, password entry), this implementation emits a
/// Tauri event and waits on a oneshot channel that the frontend will
/// resolve via `auth_respond_host_key` / `auth_respond_password`.
pub(crate) struct TauriRemoteCallbacks {
    pub app: tauri::AppHandle,
    pub pending_prompts: Arc<Mutex<PendingPrompts>>,
}

#[async_trait::async_trait]
impl RemoteCallbacks for TauriRemoteCallbacks {
    async fn verify_host_key(&self, message: &str, fingerprint: &str) -> bool {
        let prompt_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_prompts
            .lock()
            .host_key
            .insert(prompt_id.clone(), tx);
        let _ = self.app.emit(
            "ssh-host-key-prompt",
            HostKeyPromptEvent {
                prompt_id,
                message: message.to_string(),
                detail: fingerprint.to_string(),
            },
        );
        rx.await.unwrap_or(false)
    }

    async fn prompt_password(&self, message: &str) -> Option<String> {
        let prompt_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_prompts
            .lock()
            .password
            .insert(prompt_id.clone(), tx);
        let _ = self.app.emit(
            "ssh-password-prompt",
            PasswordPromptEvent {
                prompt_id,
                message: message.to_string(),
            },
        );
        rx.await.unwrap_or(None)
    }

    fn on_transfer_progress(&self, _transfer_id: &str, _bytes: u64, _total: Option<u64>) {
        // Transfer progress is handled via the existing mpsc channel pattern
        // in the transfer module, not through callbacks.
    }
}

// ---------------------------------------------------------------------------
// Desktop paths
// ---------------------------------------------------------------------------

/// Build the `RemotePaths` for a desktop environment.
fn desktop_remote_paths() -> RemotePaths {
    let home = dirs::home_dir().unwrap_or_default();
    let ssh_dir = home.join(".ssh");
    RemotePaths {
        known_hosts_file: ssh_dir.join("known_hosts"),
        config_dir: conch_core::config::config_dir().join("remote"),
        default_key_paths: vec![
            ssh_dir.join("id_ed25519"),
            ssh_dir.join("id_rsa"),
            ssh_dir.join("id_ecdsa"),
        ],
    }
}

/// Spawn an async task that drains `output_rx` and emits `pty-output` events,
/// buffering partial UTF-8 sequences between channel messages.
fn spawn_output_forwarder(
    app: &tauri::AppHandle,
    window_label: &str,
    pane_id: u32,
    mut output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let app = app.clone();
    let wl = window_label.to_owned();
    tokio::spawn(async move {
        let mut utf8 = crate::utf8_stream::Utf8Accumulator::new();
        while let Some(data) = output_rx.recv().await {
            let text = utf8.push(&data);
            if text.is_empty() {
                continue;
            }
            let _ = app.emit_to(
                &wl,
                "pty-output",
                PtyOutputEvent {
                    window_label: wl.clone(),
                    pane_id,
                    data: text,
                },
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Auth prompt events — frontend receives these, responds via commands
// ---------------------------------------------------------------------------

/// Emitted to the frontend when the SSH handler needs host key confirmation.
#[derive(Clone, Serialize)]
struct HostKeyPromptEvent {
    prompt_id: String,
    message: String,
    detail: String,
}

/// Emitted to the frontend when the SSH handler needs a password.
#[derive(Clone, Serialize)]
struct PasswordPromptEvent {
    prompt_id: String,
    message: String,
}

/// Pending auth prompts waiting for frontend responses.
pub(crate) struct PendingPrompts {
    pub(crate) host_key: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    pub(crate) password: HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>,
}

impl PendingPrompts {
    pub(crate) fn new() -> Self {
        Self {
            host_key: HashMap::new(),
            password: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// A shared SSH connection that may serve multiple pane channels.
pub(crate) struct SshConnection {
    pub ssh_handle: Arc<conch_remote::russh::client::Handle<ConchSshHandler>>,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub ref_count: u32,
}

/// A live SSH session tracked by the backend.
pub(crate) struct SshSession {
    pub input_tx: mpsc::UnboundedSender<ChannelInput>,
    pub connection_id: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    /// Handle to abort the channel loop task on cleanup.
    pub abort_handle: Option<tokio::task::AbortHandle>,
}

/// Shared state for all remote operations.
pub(crate) struct RemoteState {
    /// SSH sessions keyed by `"{window_label}:{pane_id}"` (same as local PTY keys).
    pub sessions: HashMap<String, SshSession>,
    /// Shared SSH connections keyed by `"conn:{window_label}:{pane_id}"`.
    /// Multiple sessions may reference the same connection via `connection_id`.
    pub connections: HashMap<String, SshConnection>,
    /// Server configuration.
    pub config: SshConfig,
    /// Hosts imported from `~/.ssh/config`.
    pub ssh_config_entries: Vec<ServerEntry>,
    /// Pending auth prompts waiting for frontend responses.
    pub pending_prompts: Arc<Mutex<PendingPrompts>>,
    /// Active tunnel manager.
    pub tunnel_manager: TunnelManager,
    /// Active file transfers.
    pub transfers: Arc<Mutex<TransferRegistry>>,
    /// Channel for transfer progress events (forwarded to Tauri events).
    pub transfer_progress_tx: mpsc::UnboundedSender<TransferProgress>,
    /// Platform-specific paths for SSH operations.
    pub paths: RemotePaths,
}

impl RemoteState {
    pub fn new(transfer_progress_tx: mpsc::UnboundedSender<TransferProgress>) -> Self {
        let paths = desktop_remote_paths();
        let config = conch_remote::config::load_config(&paths.config_dir);
        let ssh_config_entries = conch_remote::config::parse_ssh_config();
        Self {
            sessions: HashMap::new(),
            connections: HashMap::new(),
            config,
            ssh_config_entries,
            pending_prompts: Arc::new(Mutex::new(PendingPrompts::new())),
            tunnel_manager: TunnelManager::new(),
            transfers: Arc::new(Mutex::new(TransferRegistry::new())),
            transfer_progress_tx,
            paths,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn session_key(window_label: &str, pane_id: u32) -> String {
    format!("{window_label}:{pane_id}")
}

fn connection_key(window_label: &str, pane_id: u32) -> String {
    format!("conn:{window_label}:{pane_id}")
}

/// Shared logic for establishing an SSH session: duplicate check, SSH
/// connection, channel I/O loop, output forwarder, and cleanup task.
///
/// Both `ssh_connect` and `ssh_quick_connect` delegate to this after
/// resolving their respective server entry and credentials.
async fn establish_ssh_session(
    window_label: &str,
    app: &tauri::AppHandle,
    remote: &Arc<Mutex<RemoteState>>,
    pane_id: u32,
    server: &ServerEntry,
    credentials: &SshCredentials,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let key = session_key(window_label, pane_id);

    // Check for duplicate session.
    let (pending_prompts, paths) = {
        let state = remote.lock();
        if state.sessions.contains_key(&key) {
            return Err(format!(
                "Pane {pane_id} already has an SSH session on window {window_label}"
            ));
        }
        (Arc::clone(&state.pending_prompts), state.paths.clone())
    };

    // Build callbacks and connect via conch_remote.
    let callbacks: Arc<dyn RemoteCallbacks> = Arc::new(TauriRemoteCallbacks {
        app: app.clone(),
        pending_prompts: Arc::clone(&pending_prompts),
    });

    let (ssh_handle, channel) =
        conch_remote::ssh::connect_and_open_shell(server, credentials, callbacks, &paths)
            .await
            .map_err(|e| e.to_string())?;

    // Set up the channel I/O loop.
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Request initial resize.
    let _ = input_tx.send(ChannelInput::Resize { cols, rows });

    // Store the connection and session.
    let conn_key = connection_key(window_label, pane_id);
    let remote_clone = Arc::clone(remote);
    {
        let mut state = remote_clone.lock();
        state.connections.insert(
            conn_key.clone(),
            SshConnection {
                ssh_handle: Arc::new(ssh_handle),
                host: server.host.clone(),
                user: credentials.username.clone(),
                port: server.port,
                ref_count: 1,
            },
        );
        state.sessions.insert(
            key.clone(),
            SshSession {
                input_tx,
                connection_id: conn_key.clone(),
                host: server.host.clone(),
                user: credentials.username.clone(),
                port: server.port,
                abort_handle: None,
            },
        );
    }

    // Spawn channel loop.
    let remote_for_loop = Arc::clone(&remote_clone);
    let key_for_loop = key.clone();
    let wl = window_label.to_owned();
    let app_handle = app.clone();
    let task = tokio::spawn(async move {
        let exited_naturally = conch_remote::ssh::channel_loop(channel, input_rx, output_tx).await;

        // Clean up session and decrement connection ref count.
        let mut state = remote_for_loop.lock();
        if let Some(session) = state.sessions.remove(&key_for_loop) {
            if let Some(conn) = state.connections.get_mut(&session.connection_id) {
                conn.ref_count = conn.ref_count.saturating_sub(1);
                if conn.ref_count == 0 {
                    state.connections.remove(&session.connection_id);
                }
            }
        }
        drop(state);

        if exited_naturally {
            let _ = app_handle.emit_to(
                &wl,
                "pty-exit",
                PtyExitEvent {
                    window_label: wl.clone(),
                    pane_id,
                },
            );
        }
    });

    // Store the abort handle so the channel loop can be cancelled on window close.
    {
        let mut state = remote_clone.lock();
        if let Some(session) = state.sessions.get_mut(&key) {
            session.abort_handle = Some(task.abort_handle());
        }
    }

    spawn_output_forwarder(app, window_label, pane_id, output_rx);

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_format() {
        assert_eq!(session_key("main", 3), "main:3");
    }

    #[test]
    fn remote_state_new_has_no_sessions() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = RemoteState::new(tx);
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn connection_key_format() {
        let key = connection_key("main", 1);
        assert_eq!(key, "conn:main:1");
    }

    #[test]
    fn connection_key_differs_from_session_key() {
        let ck = connection_key("main", 1);
        let sk = session_key("main", 1);
        assert_ne!(ck, sk);
        assert!(ck.starts_with("conn:"));
    }

    #[test]
    fn remote_state_new_has_no_connections() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = RemoteState::new(tx);
        assert!(state.connections.is_empty());
    }

    #[test]
    fn pending_prompts_new_is_empty() {
        let prompts = PendingPrompts::new();
        assert!(prompts.host_key.is_empty());
        assert!(prompts.password.is_empty());
    }

    #[test]
    fn desktop_remote_paths_populated() {
        let paths = desktop_remote_paths();
        // Should have 3 default key paths.
        assert_eq!(paths.default_key_paths.len(), 3);
        assert!(
            paths
                .known_hosts_file
                .to_str()
                .unwrap()
                .contains("known_hosts")
        );
        assert!(paths.config_dir.to_str().unwrap().contains("remote"));
    }
}
