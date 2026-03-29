//! Auth prompt response commands — host key and password prompt replies.

use std::sync::Arc;

use parking_lot::Mutex;

use super::RemoteState;

// ---------------------------------------------------------------------------
// Tauri commands
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
