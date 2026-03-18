//! Remote module — unified SSH connections, SFTP, and file operations.
//!
//! Exposes Tauri commands for SSH session lifecycle. The frontend sees
//! the same `pty-output` / `pty-exit` events as local PTY tabs — xterm.js
//! doesn't care whether bytes come from a local shell or an SSH channel.

pub(crate) mod config;
mod known_hosts;
pub(crate) mod local_fs;
pub(crate) mod sftp;
pub(crate) mod ssh;
pub(crate) mod transfer;
pub(crate) mod tunnel;

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
struct PendingPrompts {
    host_key: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    password: HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>,
}

impl PendingPrompts {
    fn new() -> Self {
        Self {
            host_key: HashMap::new(),
            password: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// A live SSH session tracked by the backend.
pub(crate) struct SshSession {
    pub input_tx: mpsc::UnboundedSender<ChannelInput>,
    pub ssh_handle: Arc<russh::client::Handle<SshHandler>>,
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
    /// Pending auth prompts waiting for frontend responses.
    pending_prompts: PendingPrompts,
    /// Active tunnel manager.
    pub tunnel_manager: tunnel::TunnelManager,
    /// Active file transfers.
    pub transfers: Arc<Mutex<transfer::TransferRegistry>>,
    /// Channel for transfer progress events (forwarded to Tauri events).
    pub transfer_progress_tx: mpsc::UnboundedSender<transfer::TransferProgress>,
}

impl RemoteState {
    pub fn new(
        transfer_progress_tx: mpsc::UnboundedSender<transfer::TransferProgress>,
    ) -> Self {
        let config = config::load_config();
        let ssh_config_entries = config::parse_ssh_config();
        Self {
            sessions: HashMap::new(),
            config,
            ssh_config_entries,
            pending_prompts: PendingPrompts::new(),
            tunnel_manager: tunnel::TunnelManager::new(),
            transfers: Arc::new(Mutex::new(transfer::TransferRegistry::new())),
            transfer_progress_tx,
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
    password: Option<String>,
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

    let app_handle = app.clone();
    let server_clone = server.clone();
    let key_clone = key.clone();
    let remote_clone = Arc::clone(&*remote);

    let (result_tx, result_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let result =
            ssh::connect_and_open_shell(&server_clone, password, prompt_tx).await;
        let _ = result_tx.send(result);
    });

    // Bridge auth prompts to the frontend via Tauri events.
    // When the SSH handler needs user interaction, it sends an AuthPrompt.
    // We emit a Tauri event and store the reply channel so the frontend
    // can respond via `auth_respond_host_key` / `auth_respond_password`.
    let prompt_app = app.clone();
    let prompt_remote = Arc::clone(&*remote);
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
                            let prompt_id = uuid::Uuid::new_v4().to_string();
                            prompt_remote.lock().pending_prompts.host_key.insert(
                                prompt_id.clone(), reply,
                            );
                            let _ = prompt_app.emit("ssh-host-key-prompt", HostKeyPromptEvent {
                                prompt_id,
                                message,
                                detail,
                            });
                        }
                        Some(AuthPrompt::PasswordPrompt { reply, message }) => {
                            let prompt_id = uuid::Uuid::new_v4().to_string();
                            prompt_remote.lock().pending_prompts.password.insert(
                                prompt_id.clone(), reply,
                            );
                            let _ = prompt_app.emit("ssh-password-prompt", PasswordPromptEvent {
                                prompt_id,
                                message,
                            });
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
                ssh_handle: Arc::new(ssh_handle),
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
    password: Option<String>,
) -> Result<(), String> {
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

    // Don't persist quick-connect entries to config — they're ephemeral.
    // Connect directly using the entry instead of going through ssh_connect's
    // server lookup.
    let window_label = window.label().to_string();
    let key = session_key(&window_label, tab_id);

    {
        let state = remote.lock();
        if state.sessions.contains_key(&key) {
            return Err(format!("Tab {tab_id} already has an SSH session on window {window_label}"));
        }
    }

    let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel::<AuthPrompt>();
    let app_handle = app.clone();
    let entry_clone = entry.clone();
    let remote_clone = Arc::clone(&*remote);

    let (result_tx, result_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let result = ssh::connect_and_open_shell(&entry_clone, password, prompt_tx).await;
        let _ = result_tx.send(result);
    });

    let prompt_app = app.clone();
    let prompt_remote = Arc::clone(&*remote);
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
                            let prompt_id = uuid::Uuid::new_v4().to_string();
                            prompt_remote.lock().pending_prompts.host_key.insert(prompt_id.clone(), reply);
                            let _ = prompt_app.emit("ssh-host-key-prompt", HostKeyPromptEvent { prompt_id, message, detail });
                        }
                        Some(AuthPrompt::PasswordPrompt { reply, message }) => {
                            let prompt_id = uuid::Uuid::new_v4().to_string();
                            prompt_remote.lock().pending_prompts.password.insert(prompt_id.clone(), reply);
                            let _ = prompt_app.emit("ssh-password-prompt", PasswordPromptEvent { prompt_id, message });
                        }
                        None => { continue; }
                    }
                }
            }
        }
    })
    .await
    .map_err(|e| format!("Connection task panicked: {e}"))?;

    let (ssh_handle, channel) = connection_result?;

    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, mut output_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let _ = input_tx.send(ChannelInput::Resize { cols, rows });

    {
        let mut state = remote_clone.lock();
        state.sessions.insert(
            key.clone(),
            SshSession {
                input_tx,
                ssh_handle: Arc::new(ssh_handle),
                host: entry.host.clone(),
                user: entry.user.clone(),
                port: entry.port,
            },
        );
    }

    let remote_for_loop = Arc::clone(&remote_clone);
    let key_for_loop = key.clone();
    let wl = window_label.clone();
    tokio::spawn(async move {
        let exited_naturally = ssh::channel_loop(channel, input_rx, output_tx).await;
        remote_for_loop.lock().sessions.remove(&key_for_loop);
        if exited_naturally {
            let _ = app_handle.emit_to(&wl, "pty-exit", PtyExitEvent { window_label: wl.clone(), tab_id });
        }
    });

    let wl2 = window_label.clone();
    let app2 = app.clone();
    tokio::spawn(async move {
        while let Some(data) = output_rx.recv().await {
            let text = String::from_utf8_lossy(&data).into_owned();
            let _ = app2.emit_to(&wl2, "pty-output", PtyOutputEvent { window_label: wl2.clone(), tab_id, data: text });
        }
    });

    Ok(())
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
// Auth prompt responses from frontend
// ---------------------------------------------------------------------------

/// Frontend responds to a host key confirmation prompt.
#[tauri::command]
pub(crate) fn auth_respond_host_key(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    prompt_id: String,
    accepted: bool,
) {
    let mut state = remote.lock();
    if let Some(reply) = state.pending_prompts.host_key.remove(&prompt_id) {
        let _ = reply.send(accepted);
    }
}

/// Frontend responds to a password prompt.
#[tauri::command]
pub(crate) fn auth_respond_password(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    prompt_id: String,
    password: Option<String>,
) {
    let mut state = remote.lock();
    if let Some(reply) = state.pending_prompts.password.remove(&prompt_id) {
        let _ = reply.send(password);
    }
}

// ---------------------------------------------------------------------------
// Active sessions query
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct ActiveSession {
    key: String,
    host: String,
    user: String,
    port: u16,
}

/// List all active SSH sessions.
#[tauri::command]
pub(crate) fn remote_get_sessions(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
) -> Vec<ActiveSession> {
    let state = remote.lock();
    state
        .sessions
        .iter()
        .map(|(key, session)| ActiveSession {
            key: key.clone(),
            host: session.host.clone(),
            user: session.user.clone(),
            port: session.port,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Additional server config commands
// ---------------------------------------------------------------------------

/// Rename a folder.
#[tauri::command]
pub(crate) fn remote_rename_folder(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    folder_id: String,
    new_name: String,
) {
    let mut state = remote.lock();
    if let Some(folder) = state.config.folders.iter_mut().find(|f| f.id == folder_id) {
        folder.name = new_name;
    }
    config::save_config(&state.config);
}

/// Toggle folder expanded/collapsed state.
#[tauri::command]
pub(crate) fn remote_set_folder_expanded(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    folder_id: String,
    expanded: bool,
) {
    let mut state = remote.lock();
    state.config.set_folder_expanded(&folder_id, expanded);
    config::save_config(&state.config);
}

/// Move a server to a different folder (or ungrouped if folder_id is None).
#[tauri::command]
pub(crate) fn remote_move_server(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    server_id: String,
    folder_id: Option<String>,
) {
    let mut state = remote.lock();
    // Find and remove the server from its current location.
    let entry = state.config.find_server(&server_id).cloned();
    if let Some(entry) = entry {
        state.config.remove_server(&server_id);
        if let Some(fid) = folder_id {
            state.config.add_server_to_folder(entry, &fid);
        } else {
            state.config.add_server(entry);
        }
        config::save_config(&state.config);
    }
}

/// Duplicate a server entry.
#[tauri::command]
pub(crate) fn remote_duplicate_server(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    server_id: String,
) -> Option<ServerEntry> {
    let mut state = remote.lock();
    let entry = state.config.find_server(&server_id).cloned();
    if let Some(mut dup) = entry {
        dup.id = uuid::Uuid::new_v4().to_string();
        dup.label = format!("{} (copy)", dup.label);
        let result = dup.clone();
        state.config.add_server(dup);
        config::save_config(&state.config);
        Some(result)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// SFTP commands
// ---------------------------------------------------------------------------

/// Helper to get the SSH handle for a session by window/tab.
fn get_ssh_handle(
    state: &RemoteState,
    window_label: &str,
    tab_id: u32,
) -> Result<Arc<russh::client::Handle<SshHandler>>, String> {
    let key = session_key(window_label, tab_id);
    state
        .sessions
        .get(&key)
        .map(|s| Arc::clone(&s.ssh_handle))
        .ok_or_else(|| format!("No SSH session for {key}"))
}

#[tauri::command]
pub(crate) async fn sftp_list_dir(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
) -> Result<Vec<sftp::FileEntry>, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    sftp::list_dir(&ssh, &path).await
}

#[tauri::command]
pub(crate) async fn sftp_stat(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
) -> Result<sftp::FileEntry, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    sftp::stat(&ssh, &path).await
}

#[tauri::command]
pub(crate) async fn sftp_read_file(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
    offset: u64,
    length: u64,
) -> Result<sftp::ReadFileResult, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    sftp::read_file(&ssh, &path, offset, length as usize).await
}

#[tauri::command]
pub(crate) async fn sftp_write_file(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
    data: String,
) -> Result<u64, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    sftp::write_file(&ssh, &path, &data).await
}

#[tauri::command]
pub(crate) async fn sftp_mkdir(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
) -> Result<(), String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    sftp::mkdir(&ssh, &path).await
}

#[tauri::command]
pub(crate) async fn sftp_rename(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    from: String,
    to: String,
) -> Result<(), String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    sftp::rename(&ssh, &from, &to).await
}

#[tauri::command]
pub(crate) async fn sftp_remove(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
    is_dir: bool,
) -> Result<(), String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    sftp::remove(&ssh, &path, is_dir).await
}

#[tauri::command]
pub(crate) async fn sftp_realpath(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
) -> Result<String, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    sftp::realpath(&ssh, &path).await
}

// ---------------------------------------------------------------------------
// Local filesystem commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn local_list_dir(path: String) -> Result<Vec<sftp::FileEntry>, String> {
    local_fs::list_dir(&path)
}

#[tauri::command]
pub(crate) fn local_stat(path: String) -> Result<sftp::FileEntry, String> {
    local_fs::stat(&path)
}

#[tauri::command]
pub(crate) fn local_mkdir(path: String) -> Result<(), String> {
    local_fs::mkdir(&path)
}

#[tauri::command]
pub(crate) fn local_rename(from: String, to: String) -> Result<(), String> {
    local_fs::rename(&from, &to)
}

#[tauri::command]
pub(crate) fn local_remove(path: String, is_dir: bool) -> Result<(), String> {
    local_fs::remove(&path, is_dir)
}

// ---------------------------------------------------------------------------
// Transfer commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn transfer_download(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    remote_path: String,
    local_path: String,
) -> Result<String, String> {
    let state = remote.lock();
    let ssh = get_ssh_handle(&state, window.label(), tab_id)?;
    let transfer_id = uuid::Uuid::new_v4().to_string();
    let progress_tx = state.transfer_progress_tx.clone();
    let registry = Arc::clone(&state.transfers);
    drop(state);

    Ok(transfer::start_download(
        transfer_id,
        ssh,
        remote_path,
        local_path,
        progress_tx,
        registry,
    ))
}

#[tauri::command]
pub(crate) fn transfer_upload(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    local_path: String,
    remote_path: String,
) -> Result<String, String> {
    let state = remote.lock();
    let ssh = get_ssh_handle(&state, window.label(), tab_id)?;
    let transfer_id = uuid::Uuid::new_v4().to_string();
    let progress_tx = state.transfer_progress_tx.clone();
    let registry = Arc::clone(&state.transfers);
    drop(state);

    Ok(transfer::start_upload(
        transfer_id,
        ssh,
        local_path,
        remote_path,
        progress_tx,
        registry,
    ))
}

#[tauri::command]
pub(crate) fn transfer_cancel(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    transfer_id: String,
) -> bool {
    remote.lock().transfers.lock().cancel(&transfer_id)
}

// ---------------------------------------------------------------------------
// Tunnel commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn tunnel_start(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tunnel_id: String,
) -> Result<(), String> {
    let tunnel_uuid =
        uuid::Uuid::parse_str(&tunnel_id).map_err(|e| format!("Invalid tunnel ID: {e}"))?;

    // Clear any previous error state so this is a fresh attempt.
    {
        let mgr = remote.lock().tunnel_manager.clone();
        mgr.clear_error(&tunnel_uuid).await;
    }

    // Get tunnel definition and matching server.
    let (tunnel_def, server) = {
        let state = remote.lock();
        let tunnel = state
            .config
            .find_tunnel(&tunnel_uuid)
            .cloned()
            .ok_or_else(|| format!("Tunnel '{tunnel_id}' not found"))?;

        let server = find_server_for_tunnel(&state, &tunnel.session_key)
            .ok_or_else(|| format!("No server configured for {}", tunnel.session_key))?;

        (tunnel, server)
    };

    let mgr = remote.lock().tunnel_manager.clone();
    mgr.set_connecting(tunnel_uuid).await;

    // Set up prompt channel — tunnel prompts are auto-accepted for known hosts,
    // otherwise emitted as Tauri events (same pattern as ssh_connect).
    let (prompt_tx, mut prompt_rx) = mpsc::channel::<tunnel::TunnelPrompt>(4);
    let remote_for_prompts = Arc::clone(&*remote);
    let prompt_app = app.clone();

    // Spawn a task to service prompts.
    tokio::spawn(async move {
        while let Some(prompt) = prompt_rx.recv().await {
            match prompt {
                tunnel::TunnelPrompt::ConfirmHostKey {
                    reply,
                    message,
                    detail,
                } => {
                    let prompt_id = uuid::Uuid::new_v4().to_string();
                    remote_for_prompts
                        .lock()
                        .pending_prompts
                        .host_key
                        .insert(prompt_id.clone(), reply);
                    let _ = prompt_app.emit(
                        "ssh-host-key-prompt",
                        HostKeyPromptEvent {
                            prompt_id,
                            message,
                            detail,
                        },
                    );
                }
                tunnel::TunnelPrompt::Password { reply, message } => {
                    let prompt_id = uuid::Uuid::new_v4().to_string();
                    remote_for_prompts
                        .lock()
                        .pending_prompts
                        .password
                        .insert(prompt_id.clone(), reply);
                    let _ = prompt_app.emit(
                        "ssh-password-prompt",
                        PasswordPromptEvent {
                            prompt_id,
                            message,
                        },
                    );
                }
            }
        }
    });

    let result = mgr
        .start_tunnel(
            tunnel_uuid,
            &server,
            tunnel_def.local_port,
            tunnel_def.remote_host.clone(),
            tunnel_def.remote_port,
            prompt_tx,
        )
        .await;

    if let Err(ref e) = result {
        mgr.set_error(&tunnel_uuid, e.clone()).await;
    }

    result
}

#[tauri::command]
pub(crate) async fn tunnel_stop(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tunnel_id: String,
) -> Result<(), String> {
    let tunnel_uuid =
        uuid::Uuid::parse_str(&tunnel_id).map_err(|e| format!("Invalid tunnel ID: {e}"))?;
    let mgr = remote.lock().tunnel_manager.clone();
    mgr.stop(&tunnel_uuid).await;
    Ok(())
}

#[tauri::command]
pub(crate) fn tunnel_save(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tunnel: config::SavedTunnel,
) {
    let mut state = remote.lock();
    // Update if exists, otherwise add.
    if state.config.find_tunnel(&tunnel.id).is_some() {
        state.config.update_tunnel(tunnel);
    } else {
        state.config.add_tunnel(tunnel);
    }
    config::save_config(&state.config);
}

#[tauri::command]
pub(crate) async fn tunnel_delete(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tunnel_id: String,
) -> Result<(), String> {
    let tunnel_uuid =
        uuid::Uuid::parse_str(&tunnel_id).map_err(|e| format!("Invalid tunnel ID: {e}"))?;

    // Stop if running.
    let mgr = remote.lock().tunnel_manager.clone();
    mgr.stop(&tunnel_uuid).await;

    let mut state = remote.lock();
    state.config.remove_tunnel(&tunnel_uuid);
    config::save_config(&state.config);
    Ok(())
}

#[tauri::command]
pub(crate) async fn tunnel_get_all(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
) -> Result<Vec<TunnelWithStatus>, String> {
    let (tunnels, mgr) = {
        let state = remote.lock();
        (state.config.tunnels.clone(), state.tunnel_manager.clone())
    };

    let mut result = Vec::new();
    for t in &tunnels {
        let status = mgr.status(&t.id).await;
        result.push(TunnelWithStatus {
            tunnel: t.clone(),
            status: status.map(|s| match s {
                tunnel::TunnelStatus::Connecting => "connecting".to_string(),
                tunnel::TunnelStatus::Active => "active".to_string(),
                tunnel::TunnelStatus::Error(e) => format!("error: {e}"),
            }),
        });
    }

    Ok(result)
}

#[derive(Serialize)]
pub(crate) struct TunnelWithStatus {
    #[serde(flatten)]
    tunnel: config::SavedTunnel,
    status: Option<String>,
}

/// Find a server matching a tunnel's session_key.
fn find_server_for_tunnel(state: &RemoteState, session_key: &str) -> Option<ServerEntry> {
    let all_servers = state
        .config
        .all_servers()
        .chain(state.ssh_config_entries.iter());

    for s in all_servers {
        if config::SavedTunnel::make_session_key(&s.user, &s.host, s.port) == session_key {
            return Some(s.clone());
        }
    }

    // Fallback: parse the session_key and create a minimal entry.
    config::SavedTunnel::parse_session_key(session_key).map(|(user, host, port)| ServerEntry {
        id: String::new(),
        label: session_key.to_string(),
        host,
        port,
        user,
        auth_method: "key".to_string(),
        key_path: None,
        proxy_command: None,
        proxy_jump: None,
    })
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
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = RemoteState::new(tx);
        assert!(state.sessions.is_empty());
    }
}
