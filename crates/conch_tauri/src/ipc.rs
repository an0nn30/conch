//! IPC socket for receiving commands from external processes.
//!
//! Listens on a Unix domain socket. `conch msg new-window` / `conch msg new-tab`
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
}

/// Determine the IPC socket path (same logic as conch_app).
pub fn ipc_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir).join("conch.sock");
    }

    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/conch-{uid}.sock"))
    }

    #[cfg(not(unix))]
    {
        PathBuf::from("/tmp/conch.sock")
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

    listener
        .set_nonblocking(true)
        .expect("Failed to set non-blocking on IPC socket");

    let path_clone = socket_path.clone();
    std::thread::Builder::new()
        .name("ipc-listener".into())
        .spawn(move || {
            ipc_listen_loop(listener, app_handle);
        })
        .expect("Failed to spawn IPC listener thread");

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
                            if let Err(e) = super::create_new_window(&app) {
                                log::error!("IPC create_window failed: {e}");
                            }
                        }
                        Ok(IpcMessage::CreateTab { .. }) => {
                            super::emit_menu_action_to_focused_window(
                                &app,
                                super::MENU_ACTION_NEW_TAB,
                            );
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
