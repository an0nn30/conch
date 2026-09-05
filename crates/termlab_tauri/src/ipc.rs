//! IPC socket for receiving commands from external processes.
//!
//! Listens on a Unix domain socket. `termlab msg new-window` / `termlab msg new-tab`
//! connect, send a JSON message, and disconnect. Messages are dispatched directly
//! to the Tauri app via the AppHandle.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum IpcMessage {
    CreateWindow {
        #[serde(default)]
        working_directory: Option<String>,
    },
    CreateTab {
        #[serde(default)]
        working_directory: Option<String>,
    },
    OpenPath {
        path: String,
    },
}

/// Determine the IPC socket path (same logic as termlab_app).
pub fn ipc_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir).join("termlab.sock");
    }

    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/termlab-{uid}.sock"))
    }

    #[cfg(not(unix))]
    {
        PathBuf::from("/tmp/termlab.sock")
    }
}

/// Start the IPC listener. Returns a guard that removes the socket on drop.
#[cfg(unix)]
pub fn start(app_handle: tauri::AppHandle) -> Option<IpcGuard> {
    use std::os::unix::net::UnixListener;

    let socket_path = ipc_socket_path();
    let _ = std::fs::remove_file(&socket_path);
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            log::error!(
                "Failed to bind IPC socket at {}: {e}",
                socket_path.display()
            );
            return None;
        }
    };

    // Owner-only. The socket accepts `open_path`, which makes any peer that
    // can connect able to display any file this uid can read in a window on
    // this user's screen — so the socket must not be reachable by other
    // local users. `bind` honours the process umask, which is not something
    // to rely on for this, so the mode is set explicitly right after.
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        {
            log::warn!(
                "Failed to restrict IPC socket permissions at {}: {e}",
                socket_path.display()
            );
        }
    }

    if let Err(e) = listener.set_nonblocking(true) {
        log::error!("Failed to set non-blocking on IPC socket: {e}");
        return None;
    }

    let path_clone = socket_path.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("ipc-listener".into())
        .spawn(move || {
            ipc_listen_loop(listener, app_handle);
        })
    {
        log::error!("Failed to spawn IPC listener thread: {e}");
        return None;
    }

    log::info!("IPC socket listening at {}", socket_path.display());
    Some(IpcGuard {
        socket_path: path_clone,
    })
}

#[cfg(not(unix))]
pub fn start(_app_handle: tauri::AppHandle) -> Option<IpcGuard> {
    None
}

pub struct IpcGuard {
    socket_path: PathBuf,
}

impl Drop for IpcGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(unix)]
fn ipc_listen_loop(listener: std::os::unix::net::UnixListener, app: tauri::AppHandle) {
    use std::io::{BufRead, BufReader};
    use tauri::Emitter;

    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let reader = BufReader::new(&stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<IpcMessage>(line) {
                        Ok(IpcMessage::CreateWindow { .. }) => {
                            if let Err(e) = crate::windows::create_new_window(&app) {
                                log::error!("IPC create_window failed: {e}");
                            }
                        }
                        Ok(IpcMessage::CreateTab { .. }) => {
                            crate::menu::emit_menu_action_to_focused_window(
                                &app,
                                crate::menu::MENU_ACTION_NEW_TAB,
                            );
                        }
                        Ok(IpcMessage::OpenPath { path }) => {
                            crate::open_path::open_in_running_app(&app, &path);
                        }
                        Err(e) => {
                            log::warn!("Invalid IPC message: {e}");
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                log::error!("IPC accept error: {e}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_path_message_parses() {
        let msg: IpcMessage =
            serde_json::from_str(r#"{"type":"open_path","path":"/tmp/a.txt"}"#).unwrap();
        assert!(matches!(msg, IpcMessage::OpenPath { path } if path == "/tmp/a.txt"));
    }

    #[test]
    fn legacy_messages_still_parse() {
        assert!(serde_json::from_str::<IpcMessage>(r#"{"type":"create_window"}"#).is_ok());
        assert!(serde_json::from_str::<IpcMessage>(r#"{"type":"create_tab"}"#).is_ok());
    }
}
