//! Remote module — unified SSH connections, SFTP, and file operations.
//!
//! Exposes Tauri commands for SSH session lifecycle. The frontend sees
//! the same `pty-output` / `pty-exit` events as local PTY tabs — xterm.js
//! doesn't care whether bytes come from a local shell or an SSH channel.

pub(crate) mod config;
mod known_hosts;
pub(crate) mod ssh;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::mpsc;

use config::{ServerEntry, SshConfig};
use ssh::{AuthPrompt, ChannelInput, SshHandler};

use crate::{PtyExitEvent, PtyOutputEvent};

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// A live SSH session tracked by the backend.
pub(crate) struct SshSession {
    pub input_tx: mpsc::UnboundedSender<ChannelInput>,
    pub ssh_handle: russh::client::Handle<SshHandler>,
    pub host: String,
    pub user: String,
    pub port: u16,
}

/// Shared state for all remote operations.
pub(crate) struct RemoteState {
    /// SSH sessions keyed by `"{window_label}:{tab_id}"` (same as local PTY keys).
    pub sessions: HashMap<String, SshSession>,
    /// Server configuration.
    pub config: SshConfig,
    /// Hosts imported from `~/.ssh/config`.
    pub ssh_config_entries: Vec<ServerEntry>,
}

impl RemoteState {
    pub fn new() -> Self {
        let config = config::load_config();
        let ssh_config_entries = config::parse_ssh_config();
        Self {
            sessions: HashMap::new(),
            config,
            ssh_config_entries,
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

fn session_key(window_label: &str, tab_id: u32) -> String {
    format!("{window_label}:{tab_id}")
}

/// Connect to an SSH server and open a shell channel in a tab.
#[tauri::command]
pub(crate) async fn ssh_connect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    server_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let window_label = window.label().to_string();
    let key = session_key(&window_label, tab_id);

    // Find the server entry.
    let server = {
        let state = remote.lock();
        state
            .config
            .find_server(&server_id)
            .or_else(|| state.ssh_config_entries.iter().find(|s| s.id == server_id))
            .cloned()
            .ok_or_else(|| format!("Server '{server_id}' not found"))?
    };

    // Check for duplicate.
    {
        let state = remote.lock();
        if state.sessions.contains_key(&key) {
            return Err(format!("Tab {tab_id} already has an SSH session on window {window_label}"));
        }
    }

    // Set up the auth prompt bridge.
    let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel::<AuthPrompt>();

    // Spawn the connection on a background task.
    let app_handle = app.clone();
    let server_clone = server.clone();
    let key_clone = key.clone();
    let remote_clone = Arc::clone(&*remote);

    // Bridge auth prompts to the frontend via Tauri events + commands.
    // For now, use a simple approach: spawn the connection, service prompts inline.
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let result =
            ssh::connect_and_open_shell(&server_clone, None, prompt_tx).await;
        let _ = result_tx.send(result);
    });

    // Service auth prompts while waiting for the connection result.
    // In the future this will emit Tauri events and await responses,
    // but for key-based auth with known hosts this loop completes immediately.
    let connection_result: Result<_, String> = tokio::spawn(async move {
        let mut result_rx = result_rx;
        loop {
            tokio::select! {
                result = &mut result_rx => {
                    return result.map_err(|_| "Connection task dropped".to_string())?;
                }
                prompt = prompt_rx.recv() => {
                    match prompt {
                        Some(AuthPrompt::HostKeyConfirm { reply, message, detail }) => {
                            // TODO: proper frontend dialog
                            log::info!("Host key prompt: {message} — {detail}");
                            let _ = reply.send(true);
                        }
                        Some(AuthPrompt::PasswordPrompt { reply, message }) => {
                            log::info!("Password prompt: {message}");
                            let _ = reply.send(None);
                        }
                        None => {
                            continue;
                        }
                    }
                }
            }
        }
    })
    .await
    .map_err(|e| format!("Connection task panicked: {e}"))?;

    let (ssh_handle, channel) = connection_result?;

    // Set up the channel I/O loop.
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, mut output_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Request initial resize.
    let _ = input_tx.send(ChannelInput::Resize { cols, rows });

    // Store the session.
    {
        let mut state = remote_clone.lock();
        state.sessions.insert(
            key_clone.clone(),
            SshSession {
                input_tx,
                ssh_handle,
                host: server.host.clone(),
                user: server.user.clone(),
                port: server.port,
            },
        );
    }

    // Spawn channel loop.
    let remote_for_loop = Arc::clone(&remote_clone);
    let key_for_loop = key_clone.clone();
    let wl = window_label.clone();
    tokio::spawn(async move {
        let exited_naturally = ssh::channel_loop(channel, input_rx, output_tx).await;

        // Clean up session.
        remote_for_loop.lock().sessions.remove(&key_for_loop);

        if exited_naturally {
            let _ = app_handle.emit_to(
                &wl,
                "pty-exit",
                PtyExitEvent {
                    window_label: wl.clone(),
                    tab_id,
                },
            );
        }
    });

    // Spawn output forwarder (channel output → Tauri events).
    let wl2 = window_label.clone();
    let app2 = app.clone();
    tokio::spawn(async move {
        while let Some(data) = output_rx.recv().await {
            let text = String::from_utf8_lossy(&data).into_owned();
            let _ = app2.emit_to(
                &wl2,
                "pty-output",
                PtyOutputEvent {
                    window_label: wl2.clone(),
                    tab_id,
                    data: text,
                },
            );
        }
    });

    Ok(())
}

/// Quick-connect by parsing a `user@host:port` string.
#[tauri::command]
pub(crate) async fn ssh_quick_connect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    spec: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let (user, host, port) = parse_quick_connect(&spec);

    let entry = ServerEntry {
        id: uuid::Uuid::new_v4().to_string(),
        label: format!("{user}@{host}:{port}"),
        host,
        port,
        user,
        auth_method: "key".to_string(),
        key_path: None,
        proxy_command: None,
        proxy_jump: None,
    };

    // Add to config temporarily.
    let server_id = entry.id.clone();
    {
        let mut state = remote.lock();
        state.config.add_server(entry);
        config::save_config(&state.config);
    }

    ssh_connect(window, app, remote, tab_id, server_id, cols, rows).await
}

/// Write data to an SSH session.
#[tauri::command]
pub(crate) fn ssh_write(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    data: String,
) -> Result<(), String> {
    let key = session_key(window.label(), tab_id);
    let state = remote.lock();
    let session = state.sessions.get(&key).ok_or("SSH session not found")?;
    session
        .input_tx
        .send(ChannelInput::Write(data.into_bytes()))
        .map_err(|_| "SSH channel closed".to_string())
}

/// Resize an SSH session's terminal.
#[tauri::command]
pub(crate) fn ssh_resize(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let key = session_key(window.label(), tab_id);
    let state = remote.lock();
    let session = state.sessions.get(&key).ok_or("SSH session not found")?;
    session
        .input_tx
        .send(ChannelInput::Resize { cols, rows })
        .map_err(|_| "SSH channel closed".to_string())
}

/// Disconnect an SSH session.
#[tauri::command]
pub(crate) fn ssh_disconnect(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
) {
    let key = session_key(window.label(), tab_id);
    let mut state = remote.lock();
    if let Some(session) = state.sessions.remove(&key) {
        let _ = session.input_tx.send(ChannelInput::Shutdown);
    }
}

// ---------------------------------------------------------------------------
// Server config commands
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct ServerListResponse {
    folders: Vec<config::ServerFolder>,
    ungrouped: Vec<config::ServerEntry>,
    ssh_config: Vec<config::ServerEntry>,
}

#[tauri::command]
pub(crate) fn remote_get_servers(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
) -> ServerListResponse {
    let state = remote.lock();
    ServerListResponse {
        folders: state.config.folders.clone(),
        ungrouped: state.config.ungrouped.clone(),
        ssh_config: state.ssh_config_entries.clone(),
    }
}

#[tauri::command]
pub(crate) fn remote_save_server(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    entry: ServerEntry,
    folder_id: Option<String>,
) {
    let mut state = remote.lock();
    // Remove existing if updating.
    state.config.remove_server(&entry.id);
    if let Some(fid) = folder_id {
        state.config.add_server_to_folder(entry, &fid);
    } else {
        state.config.add_server(entry);
    }
    config::save_config(&state.config);
}

#[tauri::command]
pub(crate) fn remote_delete_server(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    server_id: String,
) {
    let mut state = remote.lock();
    state.config.remove_server(&server_id);
    config::save_config(&state.config);
}

#[tauri::command]
pub(crate) fn remote_add_folder(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    name: String,
) {
    let mut state = remote.lock();
    state.config.add_folder(&name);
    config::save_config(&state.config);
}

#[tauri::command]
pub(crate) fn remote_delete_folder(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    folder_id: String,
) {
    let mut state = remote.lock();
    state.config.remove_folder(&folder_id);
    config::save_config(&state.config);
}

#[tauri::command]
pub(crate) fn remote_import_ssh_config(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
) -> Vec<ServerEntry> {
    let mut state = remote.lock();
    state.ssh_config_entries = config::parse_ssh_config();
    state.ssh_config_entries.clone()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_quick_connect(input: &str) -> (String, String, u16) {
    let parts: Vec<&str> = input.splitn(2, '@').collect();
    let (user, host_port) = if parts.len() == 2 {
        (parts[0].to_string(), parts[1])
    } else {
        (
            std::env::var("USER").unwrap_or_else(|_| "root".to_string()),
            parts[0],
        )
    };

    let parts: Vec<&str> = host_port.rsplitn(2, ':').collect();
    let (host, port) = if parts.len() == 2 {
        (parts[1].to_string(), parts[0].parse().unwrap_or(22))
    } else {
        (parts[0].to_string(), 22u16)
    };

    (user, host, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_quick_connect_full() {
        let (user, host, port) = parse_quick_connect("deploy@10.0.0.1:2222");
        assert_eq!(user, "deploy");
        assert_eq!(host, "10.0.0.1");
        assert_eq!(port, 2222);
    }

    #[test]
    fn parse_quick_connect_no_port() {
        let (user, host, port) = parse_quick_connect("root@example.com");
        assert_eq!(user, "root");
        assert_eq!(host, "example.com");
        assert_eq!(port, 22);
    }

    #[test]
    fn parse_quick_connect_host_only() {
        let (user, host, port) = parse_quick_connect("example.com");
        assert!(!user.is_empty()); // uses $USER or "root"
        assert_eq!(host, "example.com");
        assert_eq!(port, 22);
    }

    #[test]
    fn session_key_format() {
        assert_eq!(session_key("main", 3), "main:3");
    }

    #[test]
    fn remote_state_new_has_no_sessions() {
        let state = RemoteState::new();
        assert!(state.sessions.is_empty());
    }
}
