//! Tauri-based UI for Conch (experimental).
//!
//! Uses xterm.js in a webview for terminal rendering, with a raw PTY backend
//! via `portable-pty`. This bypasses alacritty_terminal entirely — xterm.js
//! handles all terminal emulation.

mod pty_backend;
pub(crate) mod remote;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use conch_core::config::UserConfig;
use parking_lot::Mutex;
use pty_backend::PtyBackend;
use remote::RemoteState;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const MENU_NEW_TAB_ID: &str = "file.new_tab";
const MENU_CLOSE_TAB_ID: &str = "file.close_tab";
const MENU_NEW_WINDOW_ID: &str = "file.new_window";
const MENU_ACTION_EVENT: &str = "menu-action";
const MENU_ACTION_NEW_TAB: &str = "new-tab";
const MENU_ACTION_CLOSE_TAB: &str = "close-tab";

static NEXT_WINDOW_ID: AtomicU32 = AtomicU32::new(1);

struct TauriState {
    ptys: Arc<Mutex<HashMap<String, PtyBackend>>>,
    config: UserConfig,
}

#[derive(Clone, serde::Serialize)]
struct PtyOutputEvent {
    window_label: String,
    tab_id: u32,
    data: String,
}

#[derive(Clone, serde::Serialize)]
struct PtyExitEvent {
    window_label: String,
    tab_id: u32,
}

#[derive(Clone, serde::Serialize)]
struct MenuActionEvent {
    window_label: String,
    action: String,
}

fn resolved_shell(shell: &conch_core::config::TerminalShell) -> (Option<&str>, &[String]) {
    let program = shell.program.trim();
    if program.is_empty() {
        (None, &[])
    } else {
        (Some(program), shell.args.as_slice())
    }
}

fn session_key(window_label: &str, tab_id: u32) -> String {
    format!("{window_label}:{tab_id}")
}

#[tauri::command]
fn spawn_shell(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, TauriState>,
    tab_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let window_label = window.label().to_string();
    let key = session_key(&window_label, tab_id);
    let (shell, shell_args) = resolved_shell(&state.config.terminal.shell);

    let backend = PtyBackend::new(cols, rows, shell, shell_args, &state.config.terminal.env)
        .map_err(|e| format!("Failed to spawn PTY: {e}"))?;

    let reader = backend
        .try_clone_reader()
        .ok_or("Failed to clone PTY reader")?;

    {
        let mut ptys = state.ptys.lock();
        if ptys.contains_key(&key) {
            return Err(format!(
                "Tab {tab_id} already exists on window {window_label}"
            ));
        }
        ptys.insert(key.clone(), backend);
    }

    let ptys = Arc::clone(&state.ptys);
    std::thread::Builder::new()
        .name(format!("pty-reader-{window_label}-{tab_id}"))
        .spawn(move || {
            pty_reader_loop(&app, &ptys, key, window_label, tab_id, reader);
        })
        .map_err(|e| format!("Failed to spawn PTY reader thread: {e}"))?;

    Ok(())
}

#[tauri::command]
fn write_to_pty(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, TauriState>,
    tab_id: u32,
    data: String,
) -> Result<(), String> {
    let key = session_key(window.label(), tab_id);
    let guard = state.ptys.lock();
    let pty = guard.get(&key).ok_or("PTY not spawned")?;
    pty.write(data.as_bytes()).map_err(|e| format!("{e}"))
}

#[tauri::command]
fn resize_pty(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, TauriState>,
    tab_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let key = session_key(window.label(), tab_id);
    let guard = state.ptys.lock();
    let pty = guard.get(&key).ok_or("PTY not spawned")?;
    pty.resize(cols, rows).map_err(|e| format!("{e}"))
}

#[tauri::command]
fn close_pty(window: tauri::WebviewWindow, state: tauri::State<'_, TauriState>, tab_id: u32) {
    let key = session_key(window.label(), tab_id);
    state.ptys.lock().remove(&key);
}

#[tauri::command]
fn current_window_label(window: tauri::WebviewWindow) -> String {
    window.label().to_string()
}

fn build_app_menu<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<Menu<R>> {
    let new_tab = MenuItem::with_id(app, MENU_NEW_TAB_ID, "New Tab", true, Some("CmdOrCtrl+T"))?;
    let close_tab = MenuItem::with_id(
        app,
        MENU_CLOSE_TAB_ID,
        "Close Tab",
        true,
        Some("CmdOrCtrl+W"),
    )?;
    let new_window = MenuItem::with_id(
        app,
        MENU_NEW_WINDOW_ID,
        "New Window",
        true,
        Some("CmdOrCtrl+Shift+N"),
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let close_window = PredefinedMenuItem::close_window(app, None)?;

    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[&new_tab, &new_window, &separator, &close_tab, &close_window],
    )?;
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    {
        let app_name = app.package_info().name.clone();
        let app_menu = Submenu::with_items(
            app,
            app_name,
            true,
            &[
                &PredefinedMenuItem::about(app, None, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?;
        return Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu, &window_menu]);
    }

    #[cfg(not(target_os = "macos"))]
    {
        Menu::with_items(app, &[&file_menu, &edit_menu, &window_menu])
    }
}

fn focused_webview_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Option<tauri::WebviewWindow<R>> {
    let windows = app.webview_windows();
    for window in windows.values() {
        if window.is_focused().unwrap_or(false) {
            return Some(window.clone());
        }
    }
    windows.into_values().next()
}

fn emit_menu_action_to_focused_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>, action: &str) {
    if let Some(window) = focused_webview_window(app) {
        let _ = window.emit(
            MENU_ACTION_EVENT,
            MenuActionEvent {
                window_label: window.label().to_string(),
                action: action.to_string(),
            },
        );
    }
}

fn create_new_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    let label = loop {
        let id = NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = format!("window-{id}");
        if app.get_webview_window(&candidate).is_none() {
            break candidate;
        }
    };

    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title("Conch")
        .inner_size(1200.0, 800.0)
        .resizable(true)
        .decorations(true)
        .build()?;
    Ok(())
}

/// Launch the Tauri-based UI.
pub fn run(config: UserConfig) -> anyhow::Result<()> {
    let remote_state = Arc::new(Mutex::new(RemoteState::new()));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(TauriState {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            config,
        })
        .manage(Arc::clone(&remote_state))
        .setup(move |app| {
            let menu = build_app_menu(&app.handle())
                .map_err(|e| anyhow::anyhow!("Failed to build app menu: {e}"))?;
            app.handle()
                .set_menu(menu)
                .map_err(|e| anyhow::anyhow!("Failed to set app menu: {e}"))?;
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_NEW_TAB_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_NEW_TAB),
            MENU_CLOSE_TAB_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_CLOSE_TAB),
            MENU_NEW_WINDOW_ID => {
                if let Err(e) = create_new_window(app) {
                    log::error!("Failed to create window from menu: {e}");
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            spawn_shell,
            write_to_pty,
            resize_pty,
            close_pty,
            current_window_label,
            remote::ssh_connect,
            remote::ssh_quick_connect,
            remote::ssh_write,
            remote::ssh_resize,
            remote::ssh_disconnect,
            remote::remote_get_servers,
            remote::remote_save_server,
            remote::remote_delete_server,
            remote::remote_add_folder,
            remote::remote_delete_folder,
            remote::remote_import_ssh_config,
        ])
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("Tauri error: {e}"))?;

    Ok(())
}

/// Continuously reads PTY output and emits "pty-output" events to the frontend.
fn pty_reader_loop(
    handle: &tauri::AppHandle,
    pty_state: &Arc<Mutex<HashMap<String, PtyBackend>>>,
    pty_key: String,
    window_label: String,
    tab_id: u32,
    mut reader: Box<dyn std::io::Read + Send>,
) {
    let mut buf = [0u8; 8192];

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
                        tab_id,
                    },
                );
                break;
            }
            Ok(n) => {
                // Send raw bytes as a string (xterm.js expects UTF-8 or latin1).
                // Use lossy conversion for binary data.
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = handle.emit_to(
                    &window_label,
                    "pty-output",
                    PtyOutputEvent {
                        window_label: window_label.clone(),
                        tab_id,
                        data: text,
                    },
                );
            }
            Err(e) => {
                log::error!("PTY read error on tab {tab_id}: {e}");
                pty_state.lock().remove(&pty_key);
                let _ = handle.emit_to(
                    &window_label,
                    "pty-exit",
                    PtyExitEvent {
                        window_label: window_label.clone(),
                        tab_id,
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
    fn tauri_state_default_has_no_pty() {
        let state = TauriState {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            config: UserConfig::default(),
        };
        assert!(state.ptys.lock().is_empty());
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
