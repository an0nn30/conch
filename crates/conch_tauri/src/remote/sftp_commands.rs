//! SFTP and local filesystem Tauri commands.

use std::sync::Arc;

use parking_lot::Mutex;

use conch_remote::handler::ConchSshHandler;

use super::{RemoteState, session_key};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Helper to get the SSH handle for a session by window/pane.
///
/// Looks up the session's `connection_id` and retrieves the shared handle
/// from the connections map.
pub(super) fn get_ssh_handle(
    state: &RemoteState,
    window_label: &str,
    pane_id: u32,
) -> Result<Arc<conch_remote::russh::client::Handle<ConchSshHandler>>, String> {
    let key = session_key(window_label, pane_id);
    let session = state
        .sessions
        .get(&key)
        .ok_or_else(|| format!("No SSH session for {key}"))?;
    state
        .connections
        .get(&session.connection_id)
        .map(|c| Arc::clone(&c.ssh_handle))
        .ok_or_else(|| format!("No SSH connection for {}", session.connection_id))
}

// ---------------------------------------------------------------------------
// SFTP Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn sftp_list_dir(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    path: String,
) -> Result<Vec<conch_remote::sftp::FileEntry>, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), pane_id)?
    };
    conch_remote::sftp::list_dir(&ssh, &path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_stat(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    path: String,
) -> Result<conch_remote::sftp::FileEntry, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), pane_id)?
    };
    conch_remote::sftp::stat(&ssh, &path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_read_file(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    path: String,
    offset: u64,
    length: u64,
) -> Result<conch_remote::sftp::ReadFileResult, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), pane_id)?
    };
    conch_remote::sftp::read_file(&ssh, &path, offset, length as usize)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_write_file(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    path: String,
    data: String,
) -> Result<u64, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), pane_id)?
    };
    conch_remote::sftp::write_file(&ssh, &path, &data)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_mkdir(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    path: String,
) -> Result<(), String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), pane_id)?
    };
    conch_remote::sftp::mkdir(&ssh, &path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_rename(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    from: String,
    to: String,
) -> Result<(), String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), pane_id)?
    };
    conch_remote::sftp::rename(&ssh, &from, &to)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_remove(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    path: String,
    is_dir: bool,
) -> Result<(), String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), pane_id)?
    };
    conch_remote::sftp::remove(&ssh, &path, is_dir)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_realpath(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    path: String,
) -> Result<String, String> {
    let ssh = {
        let state = remote.lock();
        get_ssh_handle(&state, window.label(), pane_id)?
    };
    conch_remote::sftp::realpath(&ssh, &path)
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Local filesystem commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn local_list_dir(path: String) -> Result<Vec<conch_remote::sftp::FileEntry>, String> {
    super::local_fs::list_dir(&path)
}

#[tauri::command]
pub(crate) fn local_stat(path: String) -> Result<conch_remote::sftp::FileEntry, String> {
    super::local_fs::stat(&path)
}

#[tauri::command]
pub(crate) fn local_mkdir(path: String) -> Result<(), String> {
    super::local_fs::mkdir(&path)
}

#[tauri::command]
pub(crate) fn local_rename(from: String, to: String) -> Result<(), String> {
    super::local_fs::rename(&from, &to)
}

#[tauri::command]
pub(crate) fn local_remove(path: String, is_dir: bool) -> Result<(), String> {
    super::local_fs::remove(&path, is_dir)
}
