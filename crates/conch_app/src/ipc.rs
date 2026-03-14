//! IPC socket for receiving commands from external processes.
//!
//! Listens on a Unix domain socket. External tools (e.g. `conch msg new-window`)
//! connect, send a JSON message, and disconnect. The app polls for messages
//! each frame via `drain_ipc_messages()`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

/// Commands that can be sent over the IPC socket.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcMessage {
    /// Create a new OS window (optionally in a given directory).
    CreateWindow {
        #[serde(default)]
        working_directory: Option<String>,
    },
    /// Create a new tab in the focused window.
    CreateTab {
        #[serde(default)]
        working_directory: Option<String>,
    },
}

/// Shared message queue between the listener thread and the main app.
type MessageQueue = Arc<Mutex<Vec<IpcMessage>>>;

/// Handle to the IPC listener. Removes the socket file on drop.
pub struct IpcListener {
    socket_path: PathBuf,
    messages: MessageQueue,
    _thread: std::thread::JoinHandle<()>,
}

impl IpcListener {
    /// Start listening on a Unix domain socket.
    ///
    /// Returns `None` if the socket cannot be created (e.g. on Windows).
    #[cfg(unix)]
    pub fn start() -> Option<Self> {
        use std::os::unix::net::UnixListener;

        let socket_path = ipc_socket_path();

        // Remove stale socket if it exists.
        let _ = std::fs::remove_file(&socket_path);

        // Ensure parent directory exists.
        if let Some(parent) = socket_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let listener = match UnixListener::bind(&socket_path) {
            Ok(l) => l,
            Err(e) => {
                log::error!("Failed to bind IPC socket at {}: {e}", socket_path.display());
                return None;
            }
        };

        // Non-blocking so the thread can be joined on drop (via a short poll loop).
        listener
            .set_nonblocking(true)
            .expect("Failed to set non-blocking on IPC socket");

        let messages: MessageQueue = Arc::new(Mutex::new(Vec::new()));
        let msgs = Arc::clone(&messages);

        let thread = std::thread::Builder::new()
            .name("ipc-listener".into())
            .spawn(move || {
                ipc_listen_loop(listener, msgs);
            })
            .expect("Failed to spawn IPC listener thread");

        log::info!("IPC socket listening at {}", socket_path.display());
        Some(Self {
            socket_path,
            messages,
            _thread: thread,
        })
    }

    #[cfg(not(unix))]
    pub fn start() -> Option<Self> {
        None
    }

    /// Drain all pending IPC messages.
    pub fn drain(&self) -> Vec<IpcMessage> {
        self.messages
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }
}

impl Drop for IpcListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Determine the IPC socket path.
///
/// Prefers `$XDG_RUNTIME_DIR/conch.sock`, falls back to `/tmp/conch-{uid}.sock`.
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

/// Background loop: accept connections and parse JSON messages.
#[cfg(unix)]
fn ipc_listen_loop(listener: std::os::unix::net::UnixListener, messages: MessageQueue) {
    use std::io::{BufRead, BufReader};
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Read one JSON message per line from the connection.
                let reader = BufReader::new(&stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<IpcMessage>(line) {
                        Ok(msg) => {
                            if let Ok(mut q) = messages.lock() {
                                q.push(msg);
                            }
                        }
                        Err(e) => {
                            log::warn!("Invalid IPC message: {e}");
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No pending connections — sleep briefly before polling again.
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                log::error!("IPC accept error: {e}");
                break;
            }
        }
    }
}
