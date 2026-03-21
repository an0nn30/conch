//! Tauri commands for SSH session management.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::mpsc;

use conch_remote::callbacks::RemoteCallbacks;
use conch_remote::config::ServerEntry;
use conch_remote::ssh::ChannelInput;

use crate::callbacks::MobileRemoteCallbacks;
use crate::state::{MobileState, SshSession};

// ---------------------------------------------------------------------------
// Event types emitted to the frontend
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
struct PtyOutputEvent {
    session_id: String,
    data: String,
}

#[derive(Clone, Serialize)]
struct PtyExitEvent {
    session_id: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct ActiveSessionInfo {
    session_id: String,
    host: String,
    user: String,
    port: u16,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Quick-connect by parsing a connection string.
#[tauri::command]
pub async fn ssh_quick_connect(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<MobileState>>>,
    spec: String,
    cols: u16,
    rows: u16,
    password: Option<String>,
) -> Result<String, String> {
    let (user, host, port) = parse_quick_connect(&spec);

    let auth_method = if password.is_some() {
        "password".to_string()
    } else {
        "key".to_string()
    };

    let entry = ServerEntry {
        id: uuid::Uuid::new_v4().to_string(),
        label: format!("{user}@{host}:{port}"),
        host,
        port,
        user,
        auth_method,
        key_path: None,
        proxy_command: None,
        proxy_jump: None,
    };

    let (session_id, pending_prompts, paths) = {
        let mut s = state.lock();
        let sid = s.alloc_session_id();
        (sid, Arc::clone(&s.pending_prompts), s.paths.clone())
    };

    let callbacks: Arc<dyn RemoteCallbacks> = Arc::new(MobileRemoteCallbacks {
        app: app.clone(),
        pending_prompts: Arc::clone(&pending_prompts),
    });

    let (ssh_handle, channel) =
        conch_remote::ssh::connect_and_open_shell(&entry, password, callbacks, &paths).await?;

    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Request initial resize.
    let _ = input_tx.send(ChannelInput::Resize { cols, rows });

    // Store the session.
    {
        let mut s = state.lock();
        s.sessions.insert(
            session_id.clone(),
            SshSession {
                input_tx,
                ssh_handle: Arc::new(ssh_handle),
                host: entry.host.clone(),
                user: entry.user.clone(),
                port: entry.port,
            },
        );
    }

    // Spawn channel loop.
    let state_for_loop = Arc::clone(&*state);
    let sid_for_loop = session_id.clone();
    let app_for_loop = app.clone();
    tokio::spawn(async move {
        let exited_naturally =
            conch_remote::ssh::channel_loop(channel, input_rx, output_tx).await;

        state_for_loop.lock().sessions.remove(&sid_for_loop);

        if exited_naturally {
            let _ = app_for_loop.emit("pty-exit", PtyExitEvent {
                session_id: sid_for_loop,
            });
        }
    });

    // Spawn output forwarder.
    spawn_output_forwarder(&app, &session_id, output_rx);

    Ok(session_id)
}

/// Write data to an SSH session.
#[tauri::command]
pub async fn ssh_write(
    state: tauri::State<'_, Arc<Mutex<MobileState>>>,
    session_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let s = state.lock();
    let session = s.sessions.get(&session_id)
        .ok_or_else(|| format!("Session '{session_id}' not found"))?;
    session.input_tx.send(ChannelInput::Write(data))
        .map_err(|_| "Session channel closed".to_string())
}

/// Resize an SSH session's PTY.
#[tauri::command]
pub async fn ssh_resize(
    state: tauri::State<'_, Arc<Mutex<MobileState>>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let s = state.lock();
    let session = s.sessions.get(&session_id)
        .ok_or_else(|| format!("Session '{session_id}' not found"))?;
    session.input_tx.send(ChannelInput::Resize { cols, rows })
        .map_err(|_| "Session channel closed".to_string())
}

/// Disconnect an SSH session.
#[tauri::command]
pub async fn ssh_disconnect(
    state: tauri::State<'_, Arc<Mutex<MobileState>>>,
    session_id: String,
) -> Result<(), String> {
    let mut s = state.lock();
    if let Some(session) = s.sessions.remove(&session_id) {
        let _ = session.input_tx.send(ChannelInput::Shutdown);
    }
    Ok(())
}

/// Respond to a host key verification prompt.
#[tauri::command]
pub async fn auth_respond_host_key(
    state: tauri::State<'_, Arc<Mutex<MobileState>>>,
    prompt_id: String,
    accepted: bool,
) -> Result<(), String> {
    let s = state.lock();
    if let Some(tx) = s.pending_prompts.lock().host_key.remove(&prompt_id) {
        let _ = tx.send(accepted);
    }
    Ok(())
}

/// Respond to a password prompt.
#[tauri::command]
pub async fn auth_respond_password(
    state: tauri::State<'_, Arc<Mutex<MobileState>>>,
    prompt_id: String,
    password: Option<String>,
) -> Result<(), String> {
    let s = state.lock();
    if let Some(tx) = s.pending_prompts.lock().password.remove(&prompt_id) {
        let _ = tx.send(password);
    }
    Ok(())
}

/// Get list of active sessions.
#[tauri::command]
pub async fn get_sessions(
    state: tauri::State<'_, Arc<Mutex<MobileState>>>,
) -> Result<Vec<ActiveSessionInfo>, String> {
    let s = state.lock();
    Ok(s.sessions.iter().map(|(id, session)| ActiveSessionInfo {
        session_id: id.clone(),
        host: session.host.clone(),
        user: session.user.clone(),
        port: session.port,
    }).collect())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn a task that drains output_rx and emits pty-output events.
/// Handles partial UTF-8 sequences that may be split across packets.
fn spawn_output_forwarder(
    app: &tauri::AppHandle,
    session_id: &str,
    mut output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let app = app.clone();
    let sid = session_id.to_owned();
    tokio::spawn(async move {
        let mut pending = Vec::new();
        while let Some(data) = output_rx.recv().await {
            pending.extend_from_slice(&data);

            // Find the longest valid UTF-8 prefix, keep the rest for next time.
            let valid_len = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) => e.valid_up_to(),
            };

            if valid_len == 0 {
                continue;
            }

            let text = String::from_utf8_lossy(&pending[..valid_len]).to_string();
            pending.drain(..valid_len);

            let _ = app.emit("pty-output", PtyOutputEvent {
                session_id: sid.clone(),
                data: text,
            });
        }
    });
}

/// Parse a quick connect string into (user, host, port).
/// Supports: `host`, `user@host`, `user@host:port`, `host:port`
fn parse_quick_connect(spec: &str) -> (String, String, u16) {
    let (user, rest) = if let Some((u, r)) = spec.split_once('@') {
        (u.to_string(), r)
    } else {
        ("root".to_string(), spec)
    };

    let (host, port) = if let Some((h, p)) = rest.rsplit_once(':') {
        (h.to_string(), p.parse().unwrap_or(22))
    } else {
        (rest.to_string(), 22)
    };

    (user, host, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_quick_connect_host_only() {
        let (user, host, port) = parse_quick_connect("example.com");
        assert_eq!(user, "root");
        assert_eq!(host, "example.com");
        assert_eq!(port, 22);
    }

    #[test]
    fn parse_quick_connect_user_at_host() {
        let (user, host, port) = parse_quick_connect("deploy@10.0.0.1");
        assert_eq!(user, "deploy");
        assert_eq!(host, "10.0.0.1");
        assert_eq!(port, 22);
    }

    #[test]
    fn parse_quick_connect_user_at_host_port() {
        let (user, host, port) = parse_quick_connect("admin@server.io:2222");
        assert_eq!(user, "admin");
        assert_eq!(host, "server.io");
        assert_eq!(port, 2222);
    }

    #[test]
    fn parse_quick_connect_host_port() {
        let (user, host, port) = parse_quick_connect("myhost:8022");
        assert_eq!(user, "root");
        assert_eq!(host, "myhost");
        assert_eq!(port, 8022);
    }
}
