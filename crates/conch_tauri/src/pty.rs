//! PTY spawning, I/O, and lifecycle management.
//!
//! Contains the Tauri commands for creating, writing to, resizing, and closing
//! local PTY sessions, plus the reader loop that forwards output to the frontend.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::Emitter;
use ts_rs::TS;

use crate::TauriState;
use crate::pty_backend::PtyBackend;

// ---------------------------------------------------------------------------
// Event payloads
// ---------------------------------------------------------------------------

#[derive(Clone, serde::Serialize, TS)]
#[ts(export)]
pub(crate) struct PtyOutputEvent {
    pub window_label: String,
    pub pane_id: u32,
    pub data: String,
}

#[derive(Clone, serde::Serialize, TS)]
#[ts(export)]
pub(crate) struct PtyExitEvent {
    pub window_label: String,
    pub pane_id: u32,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn session_key(window_label: &str, pane_id: u32) -> String {
    format!("{window_label}:{pane_id}")
}

fn resolved_shell(shell: &conch_core::config::TerminalShell) -> (Option<&str>, &[String]) {
    let program = shell.program.trim();
    if program.is_empty() {
        (None, &[])
    } else {
        (Some(program), shell.args.as_slice())
    }
}

fn spawn_shell_for_pane(
    window_label: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, TauriState>,
    pane_id: u32,
    cols: u16,
    rows: u16,
    shell: Option<String>,
    shell_args: Vec<String>,
    clear_tmux_env: bool,
) -> Result<(), String> {
    let key = session_key(&window_label, pane_id);
    let cfg = state.config.read();
    let backend = PtyBackend::new(
        cols,
        rows,
        shell.as_deref(),
        &shell_args,
        &cfg.terminal.env,
        clear_tmux_env,
    )
    .map_err(|e| format!("Failed to spawn PTY: {e}"))?;
    drop(cfg);

    let reader = backend
        .try_clone_reader()
        .ok_or("Failed to clone PTY reader")?;

    {
        let mut ptys = state.ptys.lock();
        if ptys.contains_key(&key) {
            return Err(format!(
                "Pane {pane_id} already exists on window {window_label}"
            ));
        }
        ptys.insert(key.clone(), backend);
    }

    let ptys = Arc::clone(&state.ptys);
    std::thread::Builder::new()
        .name(format!("pty-reader-{window_label}-{pane_id}"))
        .spawn(move || {
            pty_reader_loop(&app, &ptys, key, window_label, pane_id, reader);
        })
        .map_err(|e| format!("Failed to spawn PTY reader thread: {e}"))?;

    Ok(())
}

fn debug_bytes_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn spawn_shell(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, TauriState>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let window_label = window.label().to_string();
    let cfg = state.config.read();
    let (shell, shell_args) = resolved_shell(&cfg.terminal.shell);
    let shell_owned = shell.map(|value| value.to_string());
    let shell_args_owned = shell_args.to_vec();
    drop(cfg);
    spawn_shell_for_pane(
        window_label,
        app,
        state,
        pane_id,
        cols,
        rows,
        shell_owned,
        shell_args_owned,
        false,
    )
}

#[tauri::command]
pub(crate) fn spawn_default_shell(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, TauriState>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let window_label = window.label().to_string();
    spawn_shell_for_pane(
        window_label,
        app,
        state,
        pane_id,
        cols,
        rows,
        None,
        Vec::new(),
        true,
    )
}

#[tauri::command]
pub(crate) fn write_to_pty(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, TauriState>,
    pane_id: u32,
    data: String,
) -> Result<(), String> {
    let key = session_key(window.label(), pane_id);
    if data.as_bytes().contains(&0x1b) {
        log::debug!(
            "[conch-keydbg] write_to_pty pane={} len={} hex={}",
            pane_id,
            data.len(),
            debug_bytes_hex(data.as_bytes())
        );
    }
    let guard = state.ptys.lock();
    let pty = guard.get(&key).ok_or("PTY not spawned")?;
    pty.write(data.as_bytes()).map_err(|e| format!("{e}"))
}

#[tauri::command]
pub(crate) fn resize_pty(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, TauriState>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let key = session_key(window.label(), pane_id);
    let guard = state.ptys.lock();
    let pty = guard.get(&key).ok_or("PTY not spawned")?;
    pty.resize(cols, rows).map_err(|e| format!("{e}"))
}

#[tauri::command]
pub(crate) fn close_pty(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, TauriState>,
    pane_id: u32,
) {
    let key = session_key(window.label(), pane_id);
    state.ptys.lock().remove(&key);
}

#[tauri::command]
pub(crate) fn get_local_pane_cwd(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, TauriState>,
    pane_id: u32,
) -> Option<String> {
    let key = session_key(window.label(), pane_id);
    let guard = state.ptys.lock();
    guard.get(&key).and_then(|pty| pty.current_dir())
}

// ---------------------------------------------------------------------------
// PTY reader loop
// ---------------------------------------------------------------------------

/// Continuously reads PTY output and emits "pty-output" events to the frontend.
fn pty_reader_loop(
    handle: &tauri::AppHandle,
    pty_state: &Arc<Mutex<HashMap<String, PtyBackend>>>,
    pty_key: String,
    window_label: String,
    pane_id: u32,
    mut reader: Box<dyn std::io::Read + Send>,
) {
    let mut buf = [0u8; 8192];
    let mut utf8 = crate::utf8_stream::Utf8Accumulator::new();

    loop {
        use std::io::Read;
        match reader.read(&mut buf) {
            Ok(0) => {
                // EOF — shell exited.
                pty_state.lock().remove(&pty_key);
                let _ = handle.emit_to(
                    &window_label,
                    "pty-exit",
                    PtyExitEvent {
                        window_label: window_label.clone(),
                        pane_id,
                    },
                );
                break;
            }
            Ok(n) => {
                let text = utf8.push(&buf[..n]);
                if text.is_empty() {
                    continue;
                }
                let _ = handle.emit_to(
                    &window_label,
                    "pty-output",
                    PtyOutputEvent {
                        window_label: window_label.clone(),
                        pane_id,
                        data: text,
                    },
                );
            }
            Err(e) => {
                log::error!("PTY read error on pane {pane_id}: {e}");
                pty_state.lock().remove(&pty_key);
                let _ = handle.emit_to(
                    &window_label,
                    "pty-exit",
                    PtyExitEvent {
                        window_label: window_label.clone(),
                        pane_id,
                    },
                );
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_uses_pane_id() {
        assert_eq!(session_key("main", 42), "main:42");
    }

    #[test]
    fn resolved_shell_empty_program_uses_default_shell() {
        let shell = conch_core::config::TerminalShell::default();
        let (program, args) = resolved_shell(&shell);
        assert!(program.is_none());
        assert!(args.is_empty());
    }

    #[test]
    fn resolved_shell_uses_configured_program_and_args() {
        let shell = conch_core::config::TerminalShell {
            program: "/bin/zsh".into(),
            args: vec!["-l".into(), "-c".into(), "echo ok".into()],
        };
        let (program, args) = resolved_shell(&shell);
        assert_eq!(program, Some("/bin/zsh"));
        assert_eq!(args, shell.args.as_slice());
    }
}
