//! SSH tunnel management Tauri commands — start, stop, save, delete, list.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;

use conch_remote::callbacks::RemoteCallbacks;
use conch_remote::config::SavedTunnel;
use conch_remote::tunnel::TunnelStatus;

use super::server_commands::{find_server_by_entry_id, find_server_for_tunnel};
use super::ssh_commands::{credentials_from_server, try_vault_credentials};
use super::{RemoteState, TauriRemoteCallbacks};
use crate::vault_commands::VaultState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct TunnelWithStatus {
    #[serde(flatten)]
    tunnel: SavedTunnel,
    status: Option<String>,
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn tunnel_start(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
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

        let server = find_server_by_entry_id(&state, tunnel.server_entry_id.as_deref())
            .or_else(|| find_server_for_tunnel(&state, &tunnel.session_key))
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

    // Try vault credentials first, fall back to legacy fields.
    let credentials = match try_vault_credentials(&vault, &server) {
        Err(e) => return Err(e),
        Ok(Some(creds)) => creds,
        Ok(None) => credentials_from_server(&server, None),
    };

    let result = mgr
        .start_tunnel(
            tunnel_uuid,
            &server,
            &credentials,
            tunnel_def.local_port,
            tunnel_def.remote_host.clone(),
            tunnel_def.remote_port,
            callbacks,
            &paths,
        )
        .await
        .map_err(|e| e.to_string());

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
pub(crate) fn tunnel_save(remote: tauri::State<'_, Arc<Mutex<RemoteState>>>, tunnel: SavedTunnel) {
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
