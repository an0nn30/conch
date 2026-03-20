//! Remote module — unified SSH connections, SFTP, and file operations.
//!
//! Exposes Tauri commands for SSH session lifecycle. The frontend sees
//! the same `pty-output` / `pty-exit` events as local PTY tabs — xterm.js
//! doesn't care whether bytes come from a local shell or an SSH channel.
//!
//! All SSH/SFTP/transfer/tunnel logic is delegated to `conch_remote`.
//! This module provides the Tauri command wrappers and the
//! `TauriRemoteCallbacks` implementation that bridges `RemoteCallbacks`
//! to Tauri events + oneshot prompt channels.

pub(crate) mod local_fs;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::mpsc;

use conch_remote::callbacks::{RemoteCallbacks, RemotePaths};
use conch_remote::config::{ExportPayload, SavedTunnel, ServerEntry, ServerFolder, SshConfig};
use conch_remote::handler::ConchSshHandler;
use conch_remote::ssh::ChannelInput;
use conch_remote::transfer::{TransferProgress, TransferRegistry};
use conch_remote::tunnel::{TunnelManager, TunnelStatus};

use crate::{PtyExitEvent, PtyOutputEvent};

// ---------------------------------------------------------------------------
// TauriRemoteCallbacks — bridges RemoteCallbacks to Tauri events
// ---------------------------------------------------------------------------

/// Bridges `conch_remote::callbacks::RemoteCallbacks` to Tauri events and
/// oneshot prompt channels. When the SSH handler needs user interaction
/// (host key confirmation, password entry), this implementation emits a
/// Tauri event and waits on a oneshot channel that the frontend will
/// resolve via `auth_respond_host_key` / `auth_respond_password`.
pub(crate) struct TauriRemoteCallbacks {
    pub app: tauri::AppHandle,
    pub pending_prompts: Arc<Mutex<PendingPrompts>>,
}

#[async_trait::async_trait]
impl RemoteCallbacks for TauriRemoteCallbacks {
    async fn verify_host_key(&self, message: &str, fingerprint: &str) -> bool {
        let prompt_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_prompts
            .lock()
            .host_key
            .insert(prompt_id.clone(), tx);
        let _ = self.app.emit(
            "ssh-host-key-prompt",
            HostKeyPromptEvent {
                prompt_id,
                message: message.to_string(),
                detail: fingerprint.to_string(),
            },
        );
        rx.await.unwrap_or(false)
    }

    async fn prompt_password(&self, message: &str) -> Option<String> {
        let prompt_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_prompts
            .lock()
            .password
            .insert(prompt_id.clone(), tx);
        let _ = self.app.emit(
            "ssh-password-prompt",
            PasswordPromptEvent {
                prompt_id,
                message: message.to_string(),
            },
        );
        rx.await.unwrap_or(None)
    }

    fn on_transfer_progress(&self, _transfer_id: &str, _bytes: u64, _total: Option<u64>) {
        // Transfer progress is handled via the existing mpsc channel pattern
        // in the transfer module, not through callbacks.
    }
}

// ---------------------------------------------------------------------------
// Desktop paths
// ---------------------------------------------------------------------------

/// Build the `RemotePaths` for a desktop environment.
fn desktop_remote_paths() -> RemotePaths {
    let home = dirs::home_dir().unwrap_or_default();
    let ssh_dir = home.join(".ssh");
    RemotePaths {
        known_hosts_file: ssh_dir.join("known_hosts"),
        config_dir: conch_core::config::config_dir().join("remote"),
        default_key_paths: vec![
            ssh_dir.join("id_ed25519"),
            ssh_dir.join("id_rsa"),
            ssh_dir.join("id_ecdsa"),
        ],
    }
}

/// Spawn an async task that drains `output_rx` and emits `pty-output` events,
/// buffering partial UTF-8 sequences between channel messages.
fn spawn_output_forwarder(
    app: &tauri::AppHandle,
    window_label: &str,
    tab_id: u32,
    mut output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let app = app.clone();
    let wl = window_label.to_owned();
    tokio::spawn(async move {
        let mut utf8 = crate::utf8_stream::Utf8Accumulator::new();
        while let Some(data) = output_rx.recv().await {
            let text = utf8.push(&data);
            if text.is_empty() {
                continue;
            }
            let _ = app.emit_to(
                &wl,
                "pty-output",
                PtyOutputEvent {
                    window_label: wl.clone(),
                    tab_id,
                    data: text,
                },
            );
        }
    });
}

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
pub(crate) struct PendingPrompts {
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
    pub ssh_handle: Arc<conch_remote::russh::client::Handle<ConchSshHandler>>,
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
    pub pending_prompts: Arc<Mutex<PendingPrompts>>,
    /// Active tunnel manager.
    pub tunnel_manager: TunnelManager,
    /// Active file transfers.
    pub transfers: Arc<Mutex<TransferRegistry>>,
    /// Channel for transfer progress events (forwarded to Tauri events).
    pub transfer_progress_tx: mpsc::UnboundedSender<TransferProgress>,
    /// Platform-specific paths for SSH operations.
    pub paths: RemotePaths,
}

impl RemoteState {
    pub fn new(transfer_progress_tx: mpsc::UnboundedSender<TransferProgress>) -> Self {
        let paths = desktop_remote_paths();
        let config = conch_remote::config::load_config(&paths.config_dir);
        let ssh_config_entries = conch_remote::config::parse_ssh_config();
        Self {
            sessions: HashMap::new(),
            config,
            ssh_config_entries,
            pending_prompts: Arc::new(Mutex::new(PendingPrompts::new())),
            tunnel_manager: TunnelManager::new(),
            transfers: Arc::new(Mutex::new(TransferRegistry::new())),
            transfer_progress_tx,
            paths,
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
    let (pending_prompts, paths) = {
        let state = remote.lock();
        if state.sessions.contains_key(&key) {
            return Err(format!(
                "Tab {tab_id} already has an SSH session on window {window_label}"
            ));
        }
        (Arc::clone(&state.pending_prompts), state.paths.clone())
    };

    // Build callbacks and connect via conch_remote.
    let callbacks: Arc<dyn RemoteCallbacks> = Arc::new(TauriRemoteCallbacks {
        app: app.clone(),
        pending_prompts: Arc::clone(&pending_prompts),
    });

    let (ssh_handle, channel) =
        conch_remote::ssh::connect_and_open_shell(&server, password, callbacks, &paths).await?;

    // Set up the channel I/O loop.
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Request initial resize.
    let _ = input_tx.send(ChannelInput::Resize { cols, rows });

    // Store the session.
    let remote_clone = Arc::clone(&*remote);
    {
        let mut state = remote_clone.lock();
        state.sessions.insert(
            key.clone(),
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
    let key_for_loop = key.clone();
    let wl = window_label.clone();
    let app_handle = app.clone();
    tokio::spawn(async move {
        let exited_naturally =
            conch_remote::ssh::channel_loop(channel, input_rx, output_tx).await;

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

    spawn_output_forwarder(&app, &window_label, tab_id, output_rx);

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
    let window_label = window.label().to_string();
    let key = session_key(&window_label, tab_id);

    let (pending_prompts, paths) = {
        let state = remote.lock();
        if state.sessions.contains_key(&key) {
            return Err(format!(
                "Tab {tab_id} already has an SSH session on window {window_label}"
            ));
        }
        (Arc::clone(&state.pending_prompts), state.paths.clone())
    };

    let callbacks: Arc<dyn RemoteCallbacks> = Arc::new(TauriRemoteCallbacks {
        app: app.clone(),
        pending_prompts: Arc::clone(&pending_prompts),
    });

    let (ssh_handle, channel) =
        conch_remote::ssh::connect_and_open_shell(&entry, password, callbacks, &paths).await?;

    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let _ = input_tx.send(ChannelInput::Resize { cols, rows });

    let remote_clone = Arc::clone(&*remote);
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
    let app_handle = app.clone();
    tokio::spawn(async move {
        let exited_naturally =
            conch_remote::ssh::channel_loop(channel, input_rx, output_tx).await;
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

    spawn_output_forwarder(&app, &window_label, tab_id, output_rx);

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
    folders: Vec<ServerFolder>,
    ungrouped: Vec<ServerEntry>,
    ssh_config: Vec<ServerEntry>,
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
    conch_remote::config::save_config(&state.paths.config_dir, &state.config);
}

#[tauri::command]
pub(crate) fn remote_delete_server(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    server_id: String,
) {
    let mut state = remote.lock();
    state.config.remove_server(&server_id);
    conch_remote::config::save_config(&state.paths.config_dir, &state.config);
}

#[tauri::command]
pub(crate) fn remote_add_folder(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    name: String,
) {
    let mut state = remote.lock();
    state.config.add_folder(&name);
    conch_remote::config::save_config(&state.paths.config_dir, &state.config);
}

#[tauri::command]
pub(crate) fn remote_delete_folder(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    folder_id: String,
) {
    let mut state = remote.lock();
    state.config.remove_folder(&folder_id);
    conch_remote::config::save_config(&state.paths.config_dir, &state.config);
}

#[tauri::command]
pub(crate) fn remote_import_ssh_config(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
) -> Vec<ServerEntry> {
    let mut state = remote.lock();
    state.ssh_config_entries = conch_remote::config::parse_ssh_config();
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
    let state = remote.lock();
    let mut prompts = state.pending_prompts.lock();
    if let Some(reply) = prompts.host_key.remove(&prompt_id) {
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
    let state = remote.lock();
    let mut prompts = state.pending_prompts.lock();
    if let Some(reply) = prompts.password.remove(&prompt_id) {
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
    conch_remote::config::save_config(&state.paths.config_dir, &state.config);
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
    conch_remote::config::save_config(&state.paths.config_dir, &state.config);
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
        conch_remote::config::save_config(&state.paths.config_dir, &state.config);
    }
}

/// Export servers and tunnels to a file chosen via native save dialog.
/// If `server_ids` or `tunnel_ids` are provided, only those items are included.
#[tauri::command]
pub(crate) async fn remote_export(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    server_ids: Option<Vec<String>>,
    tunnel_ids: Option<Vec<String>>,
) -> Result<String, String> {
    let json = {
        let state = remote.lock();
        let mut payload =
            state
                .config
                .to_export_filtered(server_ids.as_deref(), tunnel_ids.as_deref());
        // Include any selected ~/.ssh/config entries in the export.
        if let Some(ref ids) = server_ids {
            for entry in &state.ssh_config_entries {
                if ids.contains(&entry.id) {
                    payload.ungrouped.push(entry.clone());
                }
            }
        }
        serde_json::to_string_pretty(&payload).map_err(|e| format!("Export failed: {e}"))?
    };

    use tauri_plugin_dialog::DialogExt;
    let path = app
        .dialog()
        .file()
        .set_file_name("conch-connections.json")
        .add_filter("JSON", &["json"])
        .blocking_save_file();

    match path {
        Some(path) => {
            std::fs::write(path.as_path().unwrap(), &json)
                .map_err(|e| format!("Failed to write file: {e}"))?;
            Ok("Exported successfully".to_string())
        }
        None => Err("Export cancelled".to_string()),
    }
}

/// Import servers and tunnels from a file chosen via native open dialog.
#[tauri::command]
pub(crate) async fn remote_import(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;
    let path = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .blocking_pick_file();

    let path = match path {
        Some(p) => p,
        None => return Err("Import cancelled".to_string()),
    };

    let json = std::fs::read_to_string(path.as_path().unwrap())
        .map_err(|e| format!("Failed to read file: {e}"))?;

    let payload: ExportPayload =
        serde_json::from_str(&json).map_err(|e| format!("Invalid import file: {e}"))?;
    if payload.version != 1 {
        return Err(format!("Unsupported export version: {}", payload.version));
    }
    let mut state = remote.lock();
    let existing_tunnel_ids: Vec<uuid::Uuid> =
        state.config.tunnels.iter().map(|t| t.id).collect();
    let (servers, folders, tunnels) = state.config.merge_import(payload);

    // Resolve session_keys of newly imported tunnels: if a tunnel's host
    // matches a known server with a different user, rewrite the session_key
    // so it matches on activation without needing an edit+save cycle.
    resolve_imported_tunnel_keys(&mut state, &existing_tunnel_ids);

    conch_remote::config::save_config(&state.paths.config_dir, &state.config);
    Ok(format!(
        "Imported {servers} server(s), {folders} folder(s), {tunnels} tunnel(s)"
    ))
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
        conch_remote::config::save_config(&state.paths.config_dir, &state.config);
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
) -> Result<Arc<conch_remote::russh::client::Handle<ConchSshHandler>>, String> {
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
) -> Result<Vec<conch_remote::sftp::FileEntry>, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    conch_remote::sftp::list_dir(&ssh, &path).await
}

#[tauri::command]
pub(crate) async fn sftp_stat(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
) -> Result<conch_remote::sftp::FileEntry, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    conch_remote::sftp::stat(&ssh, &path).await
}

#[tauri::command]
pub(crate) async fn sftp_read_file(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
    offset: u64,
    length: u64,
) -> Result<conch_remote::sftp::ReadFileResult, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), tab_id)?
    };
    conch_remote::sftp::read_file(&ssh, &path, offset, length as usize).await
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
    conch_remote::sftp::write_file(&ssh, &path, &data).await
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
    conch_remote::sftp::mkdir(&ssh, &path).await
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
    conch_remote::sftp::rename(&ssh, &from, &to).await
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
    conch_remote::sftp::remove(&ssh, &path, is_dir).await
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
    conch_remote::sftp::realpath(&ssh, &path).await
}

// ---------------------------------------------------------------------------
// Local filesystem commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn local_list_dir(path: String) -> Result<Vec<conch_remote::sftp::FileEntry>, String> {
    local_fs::list_dir(&path)
}

#[tauri::command]
pub(crate) fn local_stat(path: String) -> Result<conch_remote::sftp::FileEntry, String> {
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
pub(crate) async fn transfer_download(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    remote_path: String,
    local_path: String,
) -> Result<String, String> {
    let (ssh, transfer_id, progress_tx, registry) = {
        let state = remote.lock();
        let ssh = get_ssh_handle(&state, window.label(), tab_id)?;
        let tid = uuid::Uuid::new_v4().to_string();
        let ptx = state.transfer_progress_tx.clone();
        let reg = Arc::clone(&state.transfers);
        (ssh, tid, ptx, reg)
    };

    Ok(conch_remote::transfer::start_download(
        transfer_id,
        ssh,
        remote_path,
        local_path,
        progress_tx,
        registry,
    ))
}

#[tauri::command]
pub(crate) async fn transfer_upload(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    tab_id: u32,
    local_path: String,
    remote_path: String,
) -> Result<String, String> {
    let (ssh, transfer_id, progress_tx, registry) = {
        let state = remote.lock();
        let ssh = get_ssh_handle(&state, window.label(), tab_id)?;
        let tid = uuid::Uuid::new_v4().to_string();
        let ptx = state.transfer_progress_tx.clone();
        let reg = Arc::clone(&state.transfers);
        (ssh, tid, ptx, reg)
    };

    Ok(conch_remote::transfer::start_upload(
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
    let (tunnel_def, server, pending_prompts, paths) = {
        let state = remote.lock();
        let tunnel = state
            .config
            .find_tunnel(&tunnel_uuid)
            .cloned()
            .ok_or_else(|| format!("Tunnel '{tunnel_id}' not found"))?;

        let server = find_server_for_tunnel(&state, &tunnel.session_key)
            .ok_or_else(|| format!("No server configured for {}", tunnel.session_key))?;

        (
            tunnel,
            server,
            Arc::clone(&state.pending_prompts),
            state.paths.clone(),
        )
    };

    let mgr = remote.lock().tunnel_manager.clone();
    mgr.set_connecting(tunnel_uuid).await;

    let callbacks: Arc<dyn RemoteCallbacks> = Arc::new(TauriRemoteCallbacks {
        app: app.clone(),
        pending_prompts,
    });

    let result = mgr
        .start_tunnel(
            tunnel_uuid,
            &server,
            tunnel_def.local_port,
            tunnel_def.remote_host.clone(),
            tunnel_def.remote_port,
            callbacks,
            &paths,
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
    tunnel: SavedTunnel,
) {
    let mut state = remote.lock();
    // Update if exists, otherwise add.
    if state.config.find_tunnel(&tunnel.id).is_some() {
        state.config.update_tunnel(tunnel);
    } else {
        state.config.add_tunnel(tunnel);
    }
    conch_remote::config::save_config(&state.paths.config_dir, &state.config);
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
    conch_remote::config::save_config(&state.paths.config_dir, &state.config);
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
                TunnelStatus::Connecting => "connecting".to_string(),
                TunnelStatus::Active => "active".to_string(),
                TunnelStatus::Error(e) => format!("error: {e}"),
            }),
        });
    }

    Ok(result)
}

#[derive(Serialize)]
pub(crate) struct TunnelWithStatus {
    #[serde(flatten)]
    tunnel: SavedTunnel,
    status: Option<String>,
}

/// Find a server matching a tunnel's session_key.
fn find_server_for_tunnel(state: &RemoteState, session_key: &str) -> Option<ServerEntry> {
    // First pass: exact session_key match.
    for s in state
        .config
        .all_servers()
        .chain(state.ssh_config_entries.iter())
    {
        if SavedTunnel::make_session_key(&s.user, &s.host, s.port) == session_key {
            return Some(s.clone());
        }
    }

    // Second pass: fuzzy matching — the session_key may reference the same
    // host with a different user, or use an SSH config Host alias as the
    // hostname.  Try progressively looser matches so we inherit the correct
    // proxy/key settings instead of falling back to a bare entry.
    if let Some((_user, host_part, port)) = SavedTunnel::parse_session_key(session_key) {
        // 2a. Match by host + port (ignoring user).
        for s in state
            .config
            .all_servers()
            .chain(state.ssh_config_entries.iter())
        {
            if s.host == host_part && s.port == port {
                return Some(s.clone());
            }
        }

        // 2b. Match SSH config Host alias (label).
        for s in state.ssh_config_entries.iter() {
            if s.label == host_part {
                return Some(s.clone());
            }
        }
    }

    // Fallback: parse the session_key and create a minimal entry.
    SavedTunnel::parse_session_key(session_key).map(|(user, host, port)| ServerEntry {
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

/// Resolve session_keys of newly imported tunnels against known servers.
///
/// When a tunnel's session_key doesn't exactly match any known server, try
/// progressively looser matching (host+port, then SSH config alias) and
/// rewrite the session_key to the canonical form so it matches on activation.
fn resolve_imported_tunnel_keys(state: &mut RemoteState, existing_ids: &[uuid::Uuid]) {
    // Build a set of all known canonical session_keys for quick lookup.
    let known_keys: Vec<String> = state
        .config
        .all_servers()
        .chain(state.ssh_config_entries.iter())
        .map(|s| SavedTunnel::make_session_key(&s.user, &s.host, s.port))
        .collect();

    // Snapshot entries for matching (avoid borrow conflict).
    let ssh_entries: Vec<ServerEntry> = state.ssh_config_entries.clone();
    let config_entries: Vec<ServerEntry> = state.config.all_servers().cloned().collect();

    for tunnel in &mut state.config.tunnels {
        if existing_ids.contains(&tunnel.id) {
            continue;
        }
        if known_keys.contains(&tunnel.session_key) {
            continue; // already matches a known server
        }

        if let Some((_user, host_part, port)) =
            SavedTunnel::parse_session_key(&tunnel.session_key)
        {
            // Try host+port match (covers user mismatch).
            let matched = config_entries
                .iter()
                .chain(ssh_entries.iter())
                .find(|s| s.host == host_part && s.port == port)
                // Then try SSH config alias match.
                .or_else(|| ssh_entries.iter().find(|s| s.label == host_part));

            if let Some(entry) = matched {
                let new_key =
                    SavedTunnel::make_session_key(&entry.user, &entry.host, entry.port);
                log::info!(
                    "resolve_imported_tunnel_keys: '{}' -> '{}' via server '{}'",
                    tunnel.session_key,
                    new_key,
                    entry.label
                );
                tunnel.session_key = new_key;
            }
        }
    }
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

    /// Build a minimal RemoteState for testing (no config files, no SSH config).
    fn test_state_with(
        config: SshConfig,
        ssh_config_entries: Vec<ServerEntry>,
    ) -> RemoteState {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        RemoteState {
            sessions: HashMap::new(),
            config,
            ssh_config_entries,
            pending_prompts: Arc::new(Mutex::new(PendingPrompts::new())),
            tunnel_manager: TunnelManager::new(),
            transfers: Arc::new(Mutex::new(TransferRegistry::new())),
            transfer_progress_tx: tx,
            paths: RemotePaths {
                known_hosts_file: std::path::PathBuf::from("/tmp/test_known_hosts"),
                config_dir: std::path::PathBuf::from("/tmp/test_config"),
                default_key_paths: vec![],
            },
        }
    }

    fn make_server(label: &str, host: &str, user: &str, port: u16) -> ServerEntry {
        ServerEntry {
            id: format!("sshconfig_{label}"),
            label: label.to_string(),
            host: host.to_string(),
            port,
            user: user.to_string(),
            auth_method: "key".to_string(),
            key_path: None,
            proxy_command: None,
            proxy_jump: None,
        }
    }

    #[test]
    fn find_server_exact_match() {
        let ssh_entry = make_server("bastion", "bastion.example.com", "admin", 22);
        let state = test_state_with(SshConfig::default(), vec![ssh_entry]);

        let result = find_server_for_tunnel(&state, "admin@bastion.example.com:22");
        assert!(result.is_some());
        assert_eq!(result.unwrap().host, "bastion.example.com");
    }

    #[test]
    fn find_server_user_mismatch_matches_by_host_port() {
        let mut ssh_entry =
            make_server("candice-pve", "bastion.nexxuscraft.com", "root", 22);
        ssh_entry.proxy_command =
            Some("cloudflared access ssh --hostname %h".to_string());
        let state = test_state_with(SshConfig::default(), vec![ssh_entry]);

        let result =
            find_server_for_tunnel(&state, "dustin@bastion.nexxuscraft.com:22");
        assert!(
            result.is_some(),
            "should match by host+port despite user mismatch"
        );
        let server = result.unwrap();
        assert_eq!(server.host, "bastion.nexxuscraft.com");
        assert_eq!(
            server.proxy_command.as_deref(),
            Some("cloudflared access ssh --hostname %h"),
            "should inherit proxy from SSH config entry"
        );
    }

    #[test]
    fn find_server_alias_no_false_positive() {
        let ssh_entry = make_server("prod-db", "db.example.com", "admin", 22);
        let state = test_state_with(SshConfig::default(), vec![ssh_entry]);

        let result = find_server_for_tunnel(&state, "admin@bastion:22");
        assert!(result.is_some(), "fallback should still return something");
        assert_eq!(result.unwrap().host, "bastion");
    }

    #[test]
    fn find_server_by_ssh_alias() {
        let mut ssh_entry = make_server("bastion", "bastion.example.com", "admin", 22);
        ssh_entry.proxy_command = Some("ssh -W %h:%p jump".to_string());
        let state = test_state_with(SshConfig::default(), vec![ssh_entry]);

        let result = find_server_for_tunnel(&state, "admin@bastion:22");
        assert!(result.is_some(), "should match via SSH config alias");
        let server = result.unwrap();
        assert_eq!(server.host, "bastion.example.com");
        assert_eq!(
            server.proxy_command.as_deref(),
            Some("ssh -W %h:%p jump"),
        );
    }

    #[test]
    fn resolve_imported_tunnel_keys_rewrites_user_mismatch() {
        let mut ssh_entry =
            make_server("candice-pve", "bastion.nexxuscraft.com", "root", 22);
        ssh_entry.proxy_command =
            Some("cloudflared access ssh --hostname %h".to_string());
        let mut cfg = SshConfig::default();
        cfg.tunnels.push(SavedTunnel {
            id: uuid::Uuid::new_v4(),
            label: "minecraft-local".to_string(),
            session_key: "dustin@bastion.nexxuscraft.com:22".to_string(),
            local_port: 25565,
            remote_host: "10.0.1.31".to_string(),
            remote_port: 25580,
            auto_start: false,
        });
        let mut state = test_state_with(cfg, vec![ssh_entry]);

        resolve_imported_tunnel_keys(&mut state, &[]);

        assert_eq!(
            state.config.tunnels[0].session_key,
            "root@bastion.nexxuscraft.com:22",
        );
    }

    #[test]
    fn resolve_imported_tunnel_keys_rewrites_alias() {
        let ssh_entry = make_server("bastion", "bastion.example.com", "admin", 22);
        let mut cfg = SshConfig::default();
        cfg.tunnels.push(SavedTunnel {
            id: uuid::Uuid::new_v4(),
            label: "test tunnel".to_string(),
            session_key: "admin@bastion:22".to_string(),
            local_port: 8080,
            remote_host: "localhost".to_string(),
            remote_port: 80,
            auto_start: false,
        });
        let mut state = test_state_with(cfg, vec![ssh_entry]);

        resolve_imported_tunnel_keys(&mut state, &[]);

        assert_eq!(
            state.config.tunnels[0].session_key,
            "admin@bastion.example.com:22",
        );
    }

    #[test]
    fn resolve_imported_tunnel_keys_skips_existing() {
        let ssh_entry = make_server("bastion", "bastion.example.com", "admin", 22);
        let tunnel_id = uuid::Uuid::new_v4();
        let mut cfg = SshConfig::default();
        cfg.tunnels.push(SavedTunnel {
            id: tunnel_id,
            label: "existing tunnel".to_string(),
            session_key: "admin@bastion:22".to_string(),
            local_port: 8080,
            remote_host: "localhost".to_string(),
            remote_port: 80,
            auto_start: false,
        });
        let mut state = test_state_with(cfg, vec![ssh_entry]);

        resolve_imported_tunnel_keys(&mut state, &[tunnel_id]);

        assert_eq!(
            state.config.tunnels[0].session_key, "admin@bastion:22",
        );
    }

    #[test]
    fn resolve_imported_tunnel_keys_preserves_already_matching() {
        let ssh_entry = make_server("bastion", "bastion.example.com", "admin", 22);
        let mut cfg = SshConfig::default();
        cfg.tunnels.push(SavedTunnel {
            id: uuid::Uuid::new_v4(),
            label: "good tunnel".to_string(),
            session_key: "admin@bastion.example.com:22".to_string(),
            local_port: 9090,
            remote_host: "localhost".to_string(),
            remote_port: 443,
            auto_start: false,
        });
        let mut state = test_state_with(cfg, vec![ssh_entry]);

        resolve_imported_tunnel_keys(&mut state, &[]);

        assert_eq!(
            state.config.tunnels[0].session_key,
            "admin@bastion.example.com:22",
        );
    }

    #[test]
    fn pending_prompts_new_is_empty() {
        let prompts = PendingPrompts::new();
        assert!(prompts.host_key.is_empty());
        assert!(prompts.password.is_empty());
    }

    #[test]
    fn desktop_remote_paths_populated() {
        let paths = desktop_remote_paths();
        // Should have 3 default key paths.
        assert_eq!(paths.default_key_paths.len(), 3);
        assert!(paths.known_hosts_file.to_str().unwrap().contains("known_hosts"));
        assert!(paths.config_dir.to_str().unwrap().contains("remote"));
    }
}
