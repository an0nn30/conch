//! File transfer Tauri commands — download, upload, cancel.
//!
//! Both transfers resolve the calling window through `session_caller_label`
//! for the same reason every SFTP command does: a popped-out panel host (or a
//! chooser window) transfers on behalf of its PARENT, and sessions are keyed
//! under the parent's label. Using `window.label()` raw here meant a
//! popped-out panel's upload/download failed with "No SSH session for
//! {own-label}:{pane}".

use std::sync::Arc;

use parking_lot::Mutex;

use super::RemoteState;
use super::sftp_commands::{get_ssh_handle, session_caller_label};

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn transfer_download(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    remote_path: String,
    local_path: String,
) -> Result<String, String> {
    let (ssh, transfer_id, progress_tx, registry) = {
        let state = remote.lock();
        let ssh = get_ssh_handle(&state, &session_caller_label(&window), pane_id)?;
        let tid = uuid::Uuid::new_v4().to_string();
        let ptx = state.transfer_progress_tx.clone();
        let reg = Arc::clone(&state.transfers);
        (ssh, tid, ptx, reg)
    };

    Ok(termlab_remote::transfer::start_download(
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
    pane_id: u32,
    local_path: String,
    remote_path: String,
) -> Result<String, String> {
    let (ssh, transfer_id, progress_tx, registry) = {
        let state = remote.lock();
        let ssh = get_ssh_handle(&state, &session_caller_label(&window), pane_id)?;
        let tid = uuid::Uuid::new_v4().to_string();
        let ptx = state.transfer_progress_tx.clone();
        let reg = Arc::clone(&state.transfers);
        (ssh, tid, ptx, reg)
    };

    Ok(termlab_remote::transfer::start_upload(
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
