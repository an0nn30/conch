//! Auth prompt response commands — host key and password prompt replies.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;

use super::RemoteState;

/// Prompts are broadcast to every window, so every window may be showing the
/// same dialog. The first response consumes the oneshot; this follow-up
/// broadcast lets the other windows close their now-answered copies.
#[derive(Clone, Serialize)]
struct AuthPromptResolvedEvent {
    prompt_id: String,
}

fn emit_prompt_resolved(app: &tauri::AppHandle, prompt_id: &str) {
    let _ = app.emit(
        "ssh-auth-prompt-resolved",
        AuthPromptResolvedEvent {
            prompt_id: prompt_id.to_string(),
        },
    );
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Frontend responds to a host key confirmation prompt.
#[tauri::command]
pub(crate) fn auth_respond_host_key(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    prompt_id: String,
    accepted: bool,
) {
    let reply = {
        let state = remote.lock();
        let mut prompts = state.pending_prompts.lock();
        prompts.host_key.remove(&prompt_id)
    };
    if let Some(reply) = reply {
        let _ = reply.send(accepted);
        emit_prompt_resolved(&app, &prompt_id);
    }
}

/// Frontend responds to a password prompt.
#[tauri::command]
pub(crate) fn auth_respond_password(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    prompt_id: String,
    password: Option<String>,
) {
    let reply = {
        let state = remote.lock();
        let mut prompts = state.pending_prompts.lock();
        prompts.password.remove(&prompt_id)
    };
    if let Some(reply) = reply {
        let _ = reply.send(password);
        emit_prompt_resolved(&app, &prompt_id);
    }
}
