//! Server configuration CRUD commands — list, save, delete, folders, import/export.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;

use conch_remote::config::{ExportPayload, SavedTunnel, ServerEntry, ServerFolder};

use super::RemoteState;
use crate::vault_commands::VaultState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct ServerListResponse {
    folders: Vec<ServerFolder>,
    ungrouped: Vec<ServerEntry>,
    ssh_config: Vec<ServerEntry>,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct ActiveSession {
    key: String,
    host: String,
    user: String,
    port: u16,
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

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
pub(crate) fn remote_add_folder(remote: tauri::State<'_, Arc<Mutex<RemoteState>>>, name: String) {
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
        let mut payload = state
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
            let file_path = path
                .as_path()
                .ok_or_else(|| "Invalid file path".to_string())?;
            std::fs::write(file_path, &json).map_err(|e| format!("Failed to write file: {e}"))?;
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
    vault: tauri::State<'_, VaultState>,
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

    let file_path = path
        .as_path()
        .ok_or_else(|| "Invalid file path".to_string())?;
    let json =
        std::fs::read_to_string(file_path).map_err(|e| format!("Failed to read file: {e}"))?;

    let payload: ExportPayload =
        serde_json::from_str(&json).map_err(|e| format!("Invalid import file: {e}"))?;
    if payload.version != 1 {
        return Err(format!("Unsupported export version: {}", payload.version));
    }
    let mut state = remote.lock();
    let existing_tunnel_ids: Vec<uuid::Uuid> = state.config.tunnels.iter().map(|t| t.id).collect();

    // Capture pre-import lengths so we can find newly added entries afterwards.
    let ungrouped_before = state.config.ungrouped.len();
    let folders_before = state.config.folders.len();

    let (servers, folders, tunnels) = state.config.merge_import(payload);

    // Resolve session_keys of newly imported tunnels: if a tunnel's host
    // matches a known server with a different user, rewrite the session_key
    // so it matches on activation without needing an edit+save cycle.
    resolve_imported_tunnel_keys(&mut state, &existing_tunnel_ids);

    // With vault_eager_import: create skeleton vault accounts for imported
    // server entries that have user/key_path legacy fields but no vault link.
    #[cfg(feature = "vault_eager_import")]
    {
        let vault_mgr = vault.lock();
        if !vault_mgr.is_locked() {
            // Process ungrouped and folder entries separately to satisfy the
            // borrow checker (two distinct mutable fields of state.config).
            let mut linked = 0usize;
            {
                let mut new_ungrouped: Vec<&mut ServerEntry> = state
                    .config
                    .ungrouped
                    .iter_mut()
                    .skip(ungrouped_before)
                    .collect();
                linked +=
                    eagerly_create_vault_accounts(&*vault_mgr, &mut new_ungrouped).unwrap_or(0);
            }
            {
                for folder in state.config.folders.iter_mut().skip(folders_before) {
                    let mut folder_entries: Vec<&mut ServerEntry> =
                        folder.entries.iter_mut().collect();
                    linked += eagerly_create_vault_accounts(&*vault_mgr, &mut folder_entries)
                        .unwrap_or(0);
                }
            }
            if linked > 0 {
                log::info!(
                    "vault_eager_import: linked {linked} imported server(s) to new vault accounts"
                );
                if let Err(e) = vault_mgr.save() {
                    log::warn!("vault_eager_import: failed to save vault after eager import: {e}");
                }
            }
        }
    }
    // Suppress unused-variable warnings when feature is disabled.
    #[cfg(not(feature = "vault_eager_import"))]
    {
        let _ = (ungrouped_before, folders_before, &vault);
    }

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
// Helpers
// ---------------------------------------------------------------------------

/// Look up a server by its entry ID (exact match).
///
/// When a tunnel has a `server_entry_id` we can resolve the correct server
/// directly, avoiding ambiguity when multiple servers share the same
/// host/port but differ by user or vault account.
pub(super) fn find_server_by_entry_id(
    state: &RemoteState,
    entry_id: Option<&str>,
) -> Option<ServerEntry> {
    let id = entry_id?;
    state
        .config
        .all_servers()
        .chain(state.ssh_config_entries.iter())
        .find(|s| s.id == id)
        .cloned()
}

/// Find a server matching a tunnel's session_key.
pub(super) fn find_server_for_tunnel(
    state: &RemoteState,
    session_key: &str,
) -> Option<ServerEntry> {
    // First pass: exact session_key match.
    for s in state
        .config
        .all_servers()
        .chain(state.ssh_config_entries.iter())
    {
        let user = s.user.as_deref().unwrap_or("root");
        if SavedTunnel::make_session_key(user, &s.host, s.port) == session_key {
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
        user: Some(user),
        auth_method: Some("key".to_string()),
        key_path: None,
        vault_account_id: None,
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
        .map(|s| {
            SavedTunnel::make_session_key(s.user.as_deref().unwrap_or("root"), &s.host, s.port)
        })
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

        if let Some((_user, host_part, port)) = SavedTunnel::parse_session_key(&tunnel.session_key)
        {
            // Try host+port match (covers user mismatch).
            let matched = config_entries
                .iter()
                .chain(ssh_entries.iter())
                .find(|s| s.host == host_part && s.port == port)
                // Then try SSH config alias match.
                .or_else(|| ssh_entries.iter().find(|s| s.label == host_part));

            if let Some(entry) = matched {
                let new_key = SavedTunnel::make_session_key(
                    entry.user.as_deref().unwrap_or("root"),
                    &entry.host,
                    entry.port,
                );
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
// Vault eager import (feature-gated)
// ---------------------------------------------------------------------------

/// Create skeleton vault accounts for imported server entries that carry
/// `user` + optional `key_path` legacy fields but have no `vault_account_id`.
///
/// Only compiled when the `vault_eager_import` feature is enabled. The vault
/// must already be unlocked before calling this function.
///
/// Returns the number of accounts created.
#[cfg(feature = "vault_eager_import")]
fn eagerly_create_vault_accounts(
    vault: &conch_vault::VaultManager,
    entries: &mut [&mut ServerEntry],
) -> Result<usize, String> {
    use std::path::PathBuf;
    let mut count = 0;
    for entry in entries.iter_mut() {
        if entry.vault_account_id.is_none() {
            if let Some(user) = &entry.user {
                let auth = match &entry.key_path {
                    Some(kp) => conch_vault::AuthMethod::Key {
                        path: PathBuf::from(kp),
                        passphrase: None,
                    },
                    None => conch_vault::AuthMethod::Password(String::new()),
                };
                let display = format!("{}@{}", user, entry.host);
                match vault.add_account(display, user.clone(), auth) {
                    Ok(id) => {
                        entry.vault_account_id = Some(id);
                        count += 1;
                    }
                    Err(e) => {
                        log::warn!(
                            "vault_eager_import: failed to create account for {}: {e}",
                            entry.host
                        );
                    }
                }
            }
        }
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use parking_lot::Mutex;

    use conch_remote::callbacks::RemotePaths;
    use conch_remote::config::{ServerEntry, SshConfig};
    use conch_remote::transfer::TransferRegistry;
    use conch_remote::tunnel::TunnelManager;

    use super::super::PendingPrompts;

    /// Build a minimal RemoteState for testing (no config files, no SSH config).
    fn test_state_with(config: SshConfig, ssh_config_entries: Vec<ServerEntry>) -> RemoteState {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        RemoteState {
            sessions: HashMap::new(),
            connections: HashMap::new(),
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
            user: Some(user.to_string()),
            auth_method: Some("key".to_string()),
            key_path: None,
            vault_account_id: None,
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
        let mut ssh_entry = make_server("candice-pve", "bastion.nexxuscraft.com", "root", 22);
        ssh_entry.proxy_command = Some("cloudflared access ssh --hostname %h".to_string());
        let state = test_state_with(SshConfig::default(), vec![ssh_entry]);

        let result = find_server_for_tunnel(&state, "dustin@bastion.nexxuscraft.com:22");
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
        assert_eq!(server.proxy_command.as_deref(), Some("ssh -W %h:%p jump"),);
    }

    #[test]
    fn find_server_by_entry_id_exact() {
        let mut server_a = make_server("prod-a", "host.example.com", "alice", 22);
        server_a.id = "aaaaaaaa-1111-2222-3333-444444444444".to_string();
        let mut server_b = make_server("prod-b", "host.example.com", "bob", 22);
        server_b.id = "bbbbbbbb-1111-2222-3333-444444444444".to_string();

        let state = test_state_with(SshConfig::default(), vec![server_a, server_b]);

        // Should resolve to server_b by entry ID even though both share host/port.
        let result = find_server_by_entry_id(&state, Some("bbbbbbbb-1111-2222-3333-444444444444"));
        assert!(result.is_some(), "should find server by entry ID");
        let server = result.unwrap();
        assert_eq!(server.user.as_deref(), Some("bob"));
        assert_eq!(server.label, "prod-b");
    }

    #[test]
    fn find_server_by_entry_id_none_returns_none() {
        let server = make_server("prod", "host.example.com", "admin", 22);
        let state = test_state_with(SshConfig::default(), vec![server]);

        assert!(
            find_server_by_entry_id(&state, None).is_none(),
            "None entry_id should return None"
        );
    }

    #[test]
    fn find_server_by_entry_id_missing_id_returns_none() {
        let server = make_server("prod", "host.example.com", "admin", 22);
        let state = test_state_with(SshConfig::default(), vec![server]);

        assert!(
            find_server_by_entry_id(&state, Some("nonexistent-id")).is_none(),
            "unknown entry_id should return None"
        );
    }

    #[test]
    fn find_server_by_entry_id_prefers_config_servers() {
        // Place server in SshConfig (not ssh_config_entries) and verify it's found.
        let mut server = make_server("vault-host", "secure.example.com", "deploy", 22);
        server.id = "cccccccc-1111-2222-3333-444444444444".to_string();
        let mut cfg = SshConfig::default();
        cfg.add_server(server);
        let state = test_state_with(cfg, vec![]);

        let result = find_server_by_entry_id(&state, Some("cccccccc-1111-2222-3333-444444444444"));
        assert!(result.is_some());
        assert_eq!(result.unwrap().host, "secure.example.com");
    }

    #[test]
    fn resolve_imported_tunnel_keys_rewrites_user_mismatch() {
        let mut ssh_entry = make_server("candice-pve", "bastion.nexxuscraft.com", "root", 22);
        ssh_entry.proxy_command = Some("cloudflared access ssh --hostname %h".to_string());
        let mut cfg = SshConfig::default();
        cfg.tunnels.push(SavedTunnel {
            id: uuid::Uuid::new_v4(),
            label: "minecraft-local".to_string(),
            session_key: "dustin@bastion.nexxuscraft.com:22".to_string(),
            server_entry_id: None,
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
            server_entry_id: None,
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
            server_entry_id: None,
            local_port: 8080,
            remote_host: "localhost".to_string(),
            remote_port: 80,
            auto_start: false,
        });
        let mut state = test_state_with(cfg, vec![ssh_entry]);

        resolve_imported_tunnel_keys(&mut state, &[tunnel_id]);

        assert_eq!(state.config.tunnels[0].session_key, "admin@bastion:22",);
    }

    #[test]
    fn resolve_imported_tunnel_keys_preserves_already_matching() {
        let ssh_entry = make_server("bastion", "bastion.example.com", "admin", 22);
        let mut cfg = SshConfig::default();
        cfg.tunnels.push(SavedTunnel {
            id: uuid::Uuid::new_v4(),
            label: "good tunnel".to_string(),
            session_key: "admin@bastion.example.com:22".to_string(),
            server_entry_id: None,
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

    // ---------------------------------------------------------------------------
    // Vault eager import tests (feature-gated)
    // ---------------------------------------------------------------------------

    #[cfg(feature = "vault_eager_import")]
    #[test]
    fn eager_import_creates_vault_account_for_entry_with_user() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = conch_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();

        let mut entry = make_server("prod", "prod.example.com", "deploy", 22);
        assert!(entry.vault_account_id.is_none());

        let mut entries: Vec<&mut ServerEntry> = vec![&mut entry];
        let count = eagerly_create_vault_accounts(&mgr, &mut entries).unwrap();

        assert_eq!(count, 1);
        assert!(entry.vault_account_id.is_some());

        // Verify the account was actually stored in the vault.
        let accounts = mgr.list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].username, "deploy");
        assert_eq!(accounts[0].display_name, "deploy@prod.example.com");
    }

    #[cfg(feature = "vault_eager_import")]
    #[test]
    fn eager_import_uses_key_auth_when_key_path_present() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = conch_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();

        let mut entry = make_server("bastion", "bastion.example.com", "admin", 22);
        entry.key_path = Some("/home/admin/.ssh/id_ed25519".into());

        let mut entries: Vec<&mut ServerEntry> = vec![&mut entry];
        eagerly_create_vault_accounts(&mgr, &mut entries).unwrap();

        let accounts = mgr.list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        match &accounts[0].auth {
            conch_vault::AuthMethod::Key { path, passphrase } => {
                assert_eq!(path.to_str().unwrap(), "/home/admin/.ssh/id_ed25519");
                assert!(passphrase.is_none());
            }
            other => panic!("expected Key auth, got {other:?}"),
        }
    }

    #[cfg(feature = "vault_eager_import")]
    #[test]
    fn eager_import_skips_entry_without_user() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = conch_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();

        // Entry with no user — should be skipped.
        let mut entry = ServerEntry {
            id: "s1".into(),
            label: "no-user".into(),
            host: "host.example.com".into(),
            port: 22,
            user: None,
            auth_method: None,
            key_path: None,
            vault_account_id: None,
            proxy_command: None,
            proxy_jump: None,
        };

        let mut entries: Vec<&mut ServerEntry> = vec![&mut entry];
        let count = eagerly_create_vault_accounts(&mgr, &mut entries).unwrap();

        assert_eq!(count, 0);
        assert!(entry.vault_account_id.is_none());
        assert!(mgr.list_accounts().unwrap().is_empty());
    }

    #[cfg(feature = "vault_eager_import")]
    #[test]
    fn eager_import_skips_entry_already_linked() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = conch_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();
        let existing_id = mgr
            .add_account(
                "existing".into(),
                "root".into(),
                conch_vault::AuthMethod::Password(String::new()),
            )
            .unwrap();

        let mut entry = make_server("srv", "srv.example.com", "root", 22);
        entry.vault_account_id = Some(existing_id);

        let mut entries: Vec<&mut ServerEntry> = vec![&mut entry];
        let count = eagerly_create_vault_accounts(&mgr, &mut entries).unwrap();

        // Should not create a second account.
        assert_eq!(count, 0);
        assert_eq!(entry.vault_account_id, Some(existing_id));
        assert_eq!(mgr.list_accounts().unwrap().len(), 1);
    }
}
