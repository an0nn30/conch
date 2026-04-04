//! Tmux backend integration for Tauri.

pub(crate) mod bridge;
pub(crate) mod events;

use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use conch_tmux::{CommandBuilder, ConnectionHandle, ConnectionWriter, SessionList};
use tauri::{AppHandle, Emitter, WebviewWindow};

use events::{TmuxConnectedEvent, TmuxPaneInfo, TmuxSessionInfo, TmuxWindowInfo};

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
    pub input_line_buffers: Mutex<HashMap<(String, u64), String>>,
    pub binary: String,
}

impl TmuxState {
    pub(crate) fn new(binary: String) -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
            sessions: Arc::new(RwLock::new(SessionList::new())),
            input_line_buffers: Mutex::new(HashMap::new()),
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

fn run_tmux(binary: &str, args: &[&str]) -> Result<String, String> {
    log::info!("[tmux] run_tmux binary={} args={:?}", binary, args);
    let output = Command::new(binary)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run tmux: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if message.is_empty() {
            format!("tmux exited with {}", output.status)
        } else {
            message
        });
    }
    let stdout = String::from_utf8(output.stdout).map_err(|e| format!("invalid tmux output: {e}"))?;
    log::info!("[tmux] run_tmux ok args={:?} stdout={:?}", args, stdout);
    Ok(stdout)
}

fn is_no_tmux_server_error(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    normalized.contains("no server running on")
        || normalized.contains("failed to connect to server")
        || normalized.contains("no sessions")
}

fn is_missing_window_error(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    normalized.contains("can't find window") || normalized.contains("no such window")
}

fn is_missing_session_error(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    normalized.contains("can't find session") || normalized.contains("no such session")
}

fn is_missing_pane_error(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    normalized.contains("can't find pane") || normalized.contains("no such pane")
}

fn apply_session_runtime_options(binary: &str, session_name: &str) {
    if let Err(error) = run_tmux(
        binary,
        &["set-option", "-t", session_name, "remain-on-exit", "off"],
    ) {
        log::warn!(
            "[tmux] failed to apply remain-on-exit=off for session {}: {}",
            session_name,
            error
        );
    }
}

fn should_force_kill_session_on_input(data: &str) -> bool {
    let has_ctrl_d = data.chars().any(|ch| ch == '\u{4}');
    if has_ctrl_d {
        return true;
    }
    let has_enter = data.chars().any(|ch| ch == '\r' || ch == '\n');
    if !has_enter {
        return false;
    }
    let normalized = data
        .trim_matches(|ch: char| ch == '\r' || ch == '\n' || ch.is_whitespace())
        .to_ascii_lowercase();
    normalized == "exit"
}

fn update_exit_input_line(buffer: &mut String, data: &str) -> bool {
    let mut should_force_kill = false;
    for ch in data.chars() {
        match ch {
            '\u{4}' => {
                should_force_kill = true;
                buffer.clear();
            }
            '\r' | '\n' => {
                if buffer.trim().eq_ignore_ascii_case("exit") {
                    should_force_kill = true;
                }
                buffer.clear();
            }
            '\u{7f}' => {
                let _ = buffer.pop();
            }
            '\u{3}' | '\u{1a}' => {
                buffer.clear();
            }
            '\t' => {}
            _ if ch.is_control() => {}
            _ => buffer.push(ch),
        }
    }
    if buffer.len() > 1024 {
        buffer.clear();
    }
    should_force_kill
}

fn should_force_kill_from_tracked_input(
    state: &TmuxState,
    window_label: &str,
    pane_id: u64,
    data: &str,
) -> bool {
    let key = (window_label.to_string(), pane_id);
    match state.input_line_buffers.lock() {
        Ok(mut buffers) => {
            let entry = buffers.entry(key).or_default();
            update_exit_input_line(entry, data)
        }
        Err(error) => {
            log::warn!(
                "[tmux] failed to lock input line buffers for exit detection: {}",
                error
            );
            false
        }
    }
}

fn clear_input_buffers_for_window(state: &TmuxState, window_label: &str) {
    if let Ok(mut buffers) = state.input_line_buffers.lock() {
        buffers.retain(|(label, _), _| label != window_label);
    }
}

fn maybe_kill_single_window_session_for_exit(
    state: &TmuxState,
    window_label: &str,
    pane_id: u64,
    should_force_kill: bool,
) {
    if !should_force_kill {
        return;
    }
    let session_name = match attached_session_for_window(state, window_label) {
        Ok(name) => name,
        Err(_) => return,
    };
    let windows = match list_windows_for_session(&state.binary, &session_name) {
        Ok(windows) => windows,
        Err(_) => return,
    };
    if windows.len() != 1 {
        return;
    }
    let only_window = &windows[0];
    if only_window.panes.len() != 1 {
        return;
    }
    let only_pane = &only_window.panes[0];
    if only_pane.id != pane_id {
        return;
    }
    if let Err(error) = run_tmux(&state.binary, &["kill-session", "-t", &session_name]) {
        if !is_missing_session_error(&error) {
            log::warn!(
                "[tmux] failed to force-kill single-window session {} after exit input: {}",
                session_name,
                error
            );
        }
    } else {
        let _ = refresh_sessions(state);
    }
}

fn refresh_sessions(state: &TmuxState) -> Result<Vec<TmuxSessionInfo>, String> {
    let raw = match run_tmux(
        &state.binary,
        &[
            "list-sessions",
            "-F",
            "#{session_id} #{session_name} #{session_windows} #{?session_attached,1,0} #{session_created}",
        ],
    ) {
        Ok(raw) => raw,
        Err(error) if is_no_tmux_server_error(&error) => {
            let mut list = state.sessions.write().map_err(|e| e.to_string())?;
            list.update_from_list_output("");
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    let mut list = state.sessions.write().map_err(|e| e.to_string())?;
    list.update_from_list_output(&raw);
    Ok(list.sessions().iter().map(TmuxSessionInfo::from).collect())
}

fn parse_window_line(line: &str) -> Option<(u64, String, bool)> {
    let mut parts = line.splitn(3, '\t');
    let id = parts.next()?.strip_prefix('@')?.parse().ok()?;
    let name = parts.next()?.to_string();
    let active = parts.next()? == "1";
    Some((id, name, active))
}

fn parse_pane_line(line: &str) -> Option<TmuxPaneInfo> {
    let mut parts = line.splitn(6, '\t');
    let id = parts.next()?.strip_prefix('%')?.parse().ok()?;
    let active = parts.next()? == "1";
    let width = parts.next()?.parse().ok()?;
    let height = parts.next()?.parse().ok()?;
    let left = parts.next()?.parse().ok()?;
    let top = parts.next()?.parse().ok()?;
    Some(TmuxPaneInfo {
        id,
        active,
        width,
        height,
        left,
        top,
        content: String::new(),
    })
}

fn capture_pane_content(binary: &str, pane_id: u64) -> String {
    let target = format!("%{pane_id}");
    run_tmux(
        binary,
        &["capture-pane", "-p", "-e", "-S", "-200", "-t", &target],
    )
    .unwrap_or_default()
}

fn list_panes_for_window(binary: &str, window_id: u64) -> Result<Vec<TmuxPaneInfo>, String> {
    let target = format!("@{window_id}");
    let raw = run_tmux(
        binary,
        &[
            "list-panes",
            "-t",
            &target,
            "-F",
            "#{pane_id}\t#{pane_active}\t#{pane_width}\t#{pane_height}\t#{pane_left}\t#{pane_top}",
        ],
    )?;
    let mut panes = Vec::new();
    for line in raw.lines() {
        if let Some(mut pane) = parse_pane_line(line) {
            pane.content = capture_pane_content(binary, pane.id);
            panes.push(pane);
        }
    }
    Ok(panes)
}

fn list_windows_for_session(binary: &str, session_name: &str) -> Result<Vec<TmuxWindowInfo>, String> {
    let raw = match run_tmux(
        binary,
        &[
            "list-windows",
            "-t",
            session_name,
            "-F",
            "#{window_id}\t#{window_name}\t#{window_active}",
        ],
    ) {
        Ok(raw) => raw,
        Err(error) if is_no_tmux_server_error(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut windows = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((id, name, active)) = parse_window_line(line) {
            windows.push(TmuxWindowInfo {
                id,
                name,
                active,
                panes: list_panes_for_window(binary, id)?,
            });
        }
    }
    Ok(windows)
}

fn attached_session_for_window(state: &TmuxState, window_label: &str) -> Result<String, String> {
    let conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get(window_label)
        .ok_or("No tmux connection for this window")?;
    conn.attached_session
        .clone()
        .ok_or("Not attached to a session".to_string())
}

fn with_window_connection_mut<T, F>(
    state: &TmuxState,
    window_label: &str,
    mut f: F,
) -> Result<T, String>
where
    F: FnMut(&mut TmuxWindowConnection) -> Result<T, String>,
{
    let mut conns = state.connections.lock().map_err(|e| e.to_string())?;
    let conn = conns
        .get_mut(window_label)
        .ok_or("No tmux connection for this window")?;
    f(conn)
}

fn send_control_commands(state: &TmuxState, window_label: &str, commands: &[String]) -> Result<(), String> {
    with_window_connection_mut(state, window_label, |conn| {
        for command in commands {
            conn.writer
                .send_command(command)
                .map_err(|e| format!("failed to send tmux command: {e}"))?;
        }
        Ok(())
    })
}

fn send_control_command(state: &TmuxState, window_label: &str, command: String) -> Result<(), String> {
    send_control_commands(state, window_label, &[command])
}

fn send_tmux_input(state: &TmuxState, window_label: &str, pane_id: u64, data: &str) -> Result<(), String> {
    let target = format!("%{pane_id}");
    let chars: Vec<char> = data.chars().collect();
    let mut index = 0usize;
    let mut literal = String::new();
    let mut commands = Vec::new();

    let flush_literal = |literal: &mut String, commands: &mut Vec<String>| -> Result<(), String> {
        if literal.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::take(literal);
        commands.push(CommandBuilder::send_keys(&target, &chunk));
        Ok(())
    };

    let send_key = |commands: &mut Vec<String>, token: &str| -> Result<(), String> {
        commands.push(format!("send-keys -t {} {}\n", target, token));
        Ok(())
    };

    while index < chars.len() {
        let ch = chars[index];
        match ch {
            ' ' => {
                flush_literal(&mut literal, &mut commands)?;
                send_key(&mut commands, "Space")?;
            }
            '\r' | '\n' => {
                flush_literal(&mut literal, &mut commands)?;
                send_key(&mut commands, "Enter")?;
            }
            '\t' => {
                flush_literal(&mut literal, &mut commands)?;
                send_key(&mut commands, "Tab")?;
            }
            '\u{7f}' => {
                flush_literal(&mut literal, &mut commands)?;
                send_key(&mut commands, "BSpace")?;
            }
            '\u{3}' => {
                flush_literal(&mut literal, &mut commands)?;
                send_key(&mut commands, "C-c")?;
            }
            '\u{4}' => {
                flush_literal(&mut literal, &mut commands)?;
                send_key(&mut commands, "C-d")?;
            }
            '\u{1a}' => {
                flush_literal(&mut literal, &mut commands)?;
                send_key(&mut commands, "C-z")?;
            }
            '\u{1b}' => {
                flush_literal(&mut literal, &mut commands)?;
                if index + 2 < chars.len() && chars[index + 1] == '[' {
                    let token = match chars[index + 2] {
                        'A' => Some("Up"),
                        'B' => Some("Down"),
                        'C' => Some("Right"),
                        'D' => Some("Left"),
                        'H' => Some("Home"),
                        'F' => Some("End"),
                        _ => None,
                    };
                    if let Some(token) = token {
                        send_key(&mut commands, token)?;
                        index += 2;
                    }
                }
            }
            _ => literal.push(ch),
        }
        index += 1;
    }

    flush_literal(&mut literal, &mut commands)?;
    send_control_commands(state, window_label, &commands)
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
    log::info!(
        "[tmux] tmux_connect start window_label={} session_name={}",
        window_label,
        session_name
    );

    if let Ok(mut conns) = state.connections.lock() {
        log::info!(
            "[tmux] tmux_connect dropping existing connection? {}",
            conns.contains_key(&window_label)
        );
        conns.remove(&window_label);
    }
    clear_input_buffers_for_window(&state, &window_label);

    let sessions = refresh_sessions(&state)?;
    let session_exists = sessions.iter().any(|session| session.name == session_name);
    log::info!(
        "[tmux] tmux_connect session_exists={} session_name={}",
        session_exists,
        session_name
    );

    let spawn_args: Vec<&str> = if session_exists {
        vec!["-u", "-CC", "attach-session", "-t", session_name.as_str()]
    } else {
        vec!["-u", "-CC", "new-session", "-s", session_name.as_str()]
    };
    let (reader, writer, handle) =
        conch_tmux::spawn(&binary, &spawn_args).map_err(|e| format!("Failed to start tmux: {e}"))?;

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

    apply_session_runtime_options(&state.binary, &session_name);

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
    log::info!("[tmux] tmux_connect emitted tmux-connected window_label={}", window_label);

    let _ = refresh_sessions(&state).and_then(|sessions| {
        app.emit_to(
            &window_label,
            "tmux-sessions-changed",
            events::TmuxSessionsChangedEvent { sessions },
        )
        .map_err(|e| e.to_string())
    });

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
    drop(conns);
    clear_input_buffers_for_window(&state, &label);
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_list_sessions(
    state: tauri::State<'_, TmuxState>,
) -> Result<Vec<TmuxSessionInfo>, String> {
    refresh_sessions(&state)
}

#[tauri::command]
pub(crate) fn tmux_create_session(
    state: tauri::State<'_, TmuxState>,
    name: Option<String>,
) -> Result<(), String> {
    log::info!("[tmux] tmux_create_session start name={:?}", name);
    let args = match name.as_deref() {
        Some(session_name) => vec!["new-session", "-d", "-s", session_name],
        None => vec!["new-session", "-d"],
    };
    let _ = run_tmux(&state.binary, &args)?;
    let sessions = refresh_sessions(&state)?;
    log::info!(
        "[tmux] tmux_create_session complete name={:?} sessions_after={}",
        name,
        sessions.len()
    );
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_kill_session(
    state: tauri::State<'_, TmuxState>,
    name: String,
) -> Result<(), String> {
    let kill_with_target = |target: &str| run_tmux(&state.binary, &["kill-session", "-t", target]);

    match kill_with_target(&name) {
        Ok(_) => {}
        Err(error) if is_missing_session_error(&error) && name.chars().all(|c| c.is_ascii_digit()) => {
            let id_target = format!("${name}");
            match kill_with_target(&id_target) {
                Ok(_) => {}
                Err(fallback_error) if is_missing_session_error(&fallback_error) => {
                    // Already gone; make kill idempotent.
                }
                Err(fallback_error) => return Err(fallback_error),
            }
        }
        Err(error) if is_missing_session_error(&error) => {
            // Already gone; make kill idempotent.
        }
        Err(error) => return Err(error),
    }

    let _ = refresh_sessions(&state)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_rename_session(
    state: tauri::State<'_, TmuxState>,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    let _ = run_tmux(&state.binary, &["rename-session", "-t", &old_name, &new_name])?;
    let _ = refresh_sessions(&state)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_list_windows(
    state: tauri::State<'_, TmuxState>,
    session_name: String,
) -> Result<Vec<TmuxWindowInfo>, String> {
    list_windows_for_session(&state.binary, &session_name)
}

#[tauri::command]
pub(crate) fn tmux_new_window(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
) -> Result<(), String> {
    let label = window.label().to_string();
    let session = attached_session_for_window(&state, &label)?;
    send_control_command(&state, &label, CommandBuilder::new_window(&session))
}

#[tauri::command]
pub(crate) fn tmux_close_window(
    state: tauri::State<'_, TmuxState>,
    window_id: u64,
) -> Result<(), String> {
    let target = format!("@{window_id}");
    if let Err(error) = run_tmux(&state.binary, &["kill-window", "-t", &target]) {
        if !is_missing_window_error(&error) {
            return Err(error);
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_rename_window(
    state: tauri::State<'_, TmuxState>,
    window_id: u64,
    name: String,
) -> Result<(), String> {
    let target = format!("@{window_id}");
    let _ = run_tmux(&state.binary, &["rename-window", "-t", &target, &name])?;
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_split_pane(
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
    horizontal: bool,
) -> Result<(), String> {
    let target = format!("%{pane_id}");
    let flag = if horizontal { "-h" } else { "-v" };
    let _ = run_tmux(&state.binary, &["split-window", flag, "-t", &target])?;
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_close_pane(
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
) -> Result<(), String> {
    let target = format!("%{pane_id}");
    let _ = run_tmux(&state.binary, &["kill-pane", "-t", &target])?;
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_select_pane(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
) -> Result<(), String> {
    let target = format!("%{pane_id}");
    let label = window.label().to_string();
    send_control_command(&state, &label, CommandBuilder::select_pane(&target))
}

#[tauri::command]
pub(crate) fn tmux_write_to_pane(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
    data: String,
) -> Result<(), String> {
    let label = window.label().to_string();
    send_tmux_input(&state, &label, pane_id, &data)?;
    let should_force_kill = should_force_kill_session_on_input(&data)
        || should_force_kill_from_tracked_input(&state, &label, pane_id, &data);
    maybe_kill_single_window_session_for_exit(&state, &label, pane_id, should_force_kill);
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_resize_pane(
    state: tauri::State<'_, TmuxState>,
    pane_id: u64,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let target = format!("%{pane_id}");
    let cols_s = cols.to_string();
    let rows_s = rows.to_string();
    if let Err(error) = run_tmux(
        &state.binary,
        &["resize-pane", "-t", &target, "-x", &cols_s, "-y", &rows_s],
    ) {
        if !is_missing_pane_error(&error) {
            return Err(error);
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn tmux_resize_client(
    window: WebviewWindow,
    state: tauri::State<'_, TmuxState>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let label = window.label().to_string();
    let command = format!("refresh-client -C {},{}\n", cols, rows);
    send_control_command(&state, &label, command)
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
    let last_session = conch_core::config::load_persistent_state()
        .ok()
        .and_then(|s| s.last_tmux_session);
    log::info!("[tmux] tmux_get_last_session -> {:?}", last_session);
    last_session
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

    #[test]
    fn parse_window_line_parses_tab_separated_output() {
        let parsed = parse_window_line("@7\teditor\t1").unwrap();
        assert_eq!(parsed.0, 7);
        assert_eq!(parsed.1, "editor");
        assert!(parsed.2);
    }

    #[test]
    fn parse_pane_line_parses_tab_separated_output() {
        let parsed = parse_pane_line("%11\t0\t120\t42").unwrap();
        assert_eq!(parsed.id, 11);
        assert!(!parsed.active);
        assert_eq!(parsed.width, 120);
        assert_eq!(parsed.height, 42);
    }

    #[test]
    fn update_exit_input_line_detects_exit_across_keystrokes() {
        let mut buffer = String::new();
        assert!(!update_exit_input_line(&mut buffer, "e"));
        assert!(!update_exit_input_line(&mut buffer, "x"));
        assert!(!update_exit_input_line(&mut buffer, "i"));
        assert!(!update_exit_input_line(&mut buffer, "t"));
        assert!(update_exit_input_line(&mut buffer, "\r"));
        assert!(buffer.is_empty());
    }

    #[test]
    fn update_exit_input_line_handles_backspace() {
        let mut buffer = String::new();
        assert!(!update_exit_input_line(&mut buffer, "exix"));
        assert!(!update_exit_input_line(&mut buffer, "\u{7f}"));
        assert!(!update_exit_input_line(&mut buffer, "t"));
        assert!(update_exit_input_line(&mut buffer, "\n"));
        assert!(buffer.is_empty());
    }
}
