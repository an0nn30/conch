//! Tmux backend integration for Tauri.

pub(crate) mod bridge;
pub(crate) mod events;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use conch_tmux::{CommandBuilder, ConnectionHandle, ConnectionWriter, SessionList};
use tauri::{AppHandle, Emitter, WebviewWindow};

use events::{TmuxConnectedEvent, TmuxSessionInfo};

/// Per-window tmux connection state.
pub(crate) struct TmuxWindowConnection {
    pub writer: ConnectionWriter,
    pub _handle: ConnectionHandle,
    pub _reader_join: Option<JoinHandle<()>>,
    pub attached_session: Option<String>,
}

/// App-level tmux state.
pub(crate) struct TmuxState {
    pub connections: Mutex<HashMap<String, TmuxWindowConnection>>,
    pub sessions: Arc<RwLock<SessionList>>,
    pub binary: String,
}

impl TmuxState {
    pub(crate) fn new(binary: String) -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
            sessions: Arc::new(RwLock::new(SessionList::new())),
            binary,
        }
    }
}

/// Check that tmux is installed and >= 1.8 (control mode support).
pub(crate) fn validate_tmux_binary(binary: &str) -> Result<String, String> {
    let output = std::process::Command::new(binary)
        .arg("-V")
        .output()
        .map_err(|e| format!("tmux not found at '{}': {}", binary, e))?;
    let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let version_part = version_str.strip_prefix("tmux ").unwrap_or(&version_str);
    let major_minor: f64 = version_part
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .parse()
        .unwrap_or(0.0);
    if major_minor < 1.8 {
        return Err(format!(
            "tmux {} is too old — control mode requires tmux >= 1.8",
            version_str
        ));
    }
    Ok(version_str)
}

// --- Tauri commands ---

#[tauri::command]
pub(crate) fn tmux_connect(
    window: WebviewWindow,
    app: AppHandle,
    state: tauri::State<'_, TmuxState>,
    session_name: String,
) -> Result<(), String> {
    let window_label = window.label().to_string();
    let binary = state.binary.clone();

    let (reader, writer, handle) =
        conch_tmux::spawn(&binary, &["-CC", "new-session", "-A", "-s", &session_name])
            .map_err(|e| format!("Failed to start tmux: {e}"))?;

    let sessions = Arc::clone(&state.sessions);
    let reader_join =
        bridge::spawn_reader_thread(app.clone(), window_label.clone(), reader, sessions);

    let conn = TmuxWindowConnection {
        writer,
        _handle: handle,
        _reader_join: Some(reader_join),
        attached_session: Some(session_name.clone()),
    };

    state
        .connections
        .lock()
        .map_err(|e| e.to_string())?
        .insert(window_label.clone(), conn);

    // Persist last session for attach_last_session startup behavior.
    if let Ok(mut ps) = conch_core::config::load_persistent_state() {
        ps.last_tmux_session = Some(session_name.clone());
        let _ = conch_core::config::save_persistent_state(&ps);
    }

    let _ = app.emit_to(
        &window_label,
        "tmux-connected",
        TmuxConnectedEvent {
            session: session_name,
        },
    );

    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_disconnect(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    if let Some(conn) = conns.remove(&label) {
        drop(conn);
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_list_sessions(
    state: tauri::State<'_, TmuxState>,
) -> Result<Vec<TmuxSessionInfo>, String> {
    let list = state.sessions.read().map_err(|e| e.to_string())?;
    Ok(list.sessions().iter().map(TmuxSessionInfo::from).collect())
}

#[tauri::command]
pub(crate) fn tmux_create_session(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    name: Option<String>,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    let cmd = CommandBuilder::new_session(name.as_deref());
    conn.writer.send_command(&cmd).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_kill_session(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    name: String,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    conn.writer
        .send_command(&CommandBuilder::kill_session(&name))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_rename_session(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    conn.writer
        .send_command(&CommandBuilder::rename_session(&old_name, &new_name))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_new_window(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    let session = conn
        .attached_session
        .as_deref()
        .ok_or("Not attached to a session")?;
    conn.writer
        .send_command(&CommandBuilder::new_window(session))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_close_window(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    window_id: u64,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    let target = format!("@{window_id}");
    conn.writer
        .send_command(&CommandBuilder::kill_window(&target))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_rename_window(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    window_id: u64,
    name: String,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    let target = format!("@{window_id}");
    conn.writer
        .send_command(&CommandBuilder::rename_window(&target, &name))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_split_pane(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
    horizontal: bool,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    let target = format!("%{pane_id}");
    conn.writer
        .send_command(&CommandBuilder::split_window(&target, horizontal))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_close_pane(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    let target = format!("%{pane_id}");
    conn.writer
        .send_command(&CommandBuilder::kill_pane(&target))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_select_pane(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    let target = format!("%{pane_id}");
    conn.writer
        .send_command(&CommandBuilder::select_pane(&target))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_write_to_pane(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
    data: String,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    let target = format!("%{pane_id}");
    conn.writer
        .send_command(&CommandBuilder::send_keys(&target, &data))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_resize_pane(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(&label)
        .ok_or("No tmux connection for this window")?;
    let target = format!("%{pane_id}");
    conn.writer
        .send_command(&CommandBuilder::resize_pane(&target, cols, rows))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tmux_get_backend() -> String {
    let config = conch_core::config::load_user_config().unwrap_or_default();
    match config.terminal.backend {
        conch_core::config::TerminalBackend::Local => "local".into(),
        conch_core::config::TerminalBackend::Tmux => "tmux".into(),
    }
}

#[tauri::command]
pub(crate) fn tmux_get_last_session() -> Option<String> {
    conch_core::config::load_persistent_state()
        .ok()
        .and_then(|s| s.last_tmux_session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_state_new_has_empty_connections() {
        let state = TmuxState::new("tmux".into());
        assert!(state.connections.lock().unwrap().is_empty());
    }

    #[test]
    fn tmux_state_new_has_empty_sessions() {
        let state = TmuxState::new("tmux".into());
        assert!(state.sessions.read().unwrap().sessions().is_empty());
    }

    #[test]
    fn tmux_state_stores_binary() {
        let state = TmuxState::new("/opt/homebrew/bin/tmux".into());
        assert_eq!(state.binary, "/opt/homebrew/bin/tmux");
    }
}
