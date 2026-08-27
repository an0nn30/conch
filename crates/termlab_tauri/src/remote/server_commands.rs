//! Server configuration CRUD commands — list, save, delete, folders, import/export.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
use ts_rs::TS;

use termlab_remote::config::{ExportPayload, SavedTunnel, ServerEntry, ServerFolder};

use super::RemoteState;
use crate::vault_commands::VaultState;

/// Emitted after any successful import (legacy JSON via `apply_legacy_import`
/// below, or a bundle via `share_commands::do_import`) so every open
/// `index.html` window — not just the one that ran the import — refreshes
/// its SSH panel. `RemoteState` is one `Arc<Mutex<_>>` shared by every
/// window's Tauri commands, but that only keeps the *backend* in sync;
/// `ssh-panel.js` has no polling or refresh-on-focus, so a second window
/// opened via `windows::create_new_window` would otherwise show stale
/// Hosts/Tunnels lists until the user manually refreshes it. Deliberately a
/// distinct name from `config-changed`, which is scoped to theme/font
/// hot-reload (`watcher.rs`, `settings.rs`) — reusing it would trigger
/// unrelated UI refresh logic.
pub(crate) const SSH_CONFIG_CHANGED_EVENT: &str = "ssh-config-changed";

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
    upsert_server(&mut state, entry, folder_id);
}

/// Insert-or-replace a server entry at `folder_id` (ungrouped when `None`) and
/// persist the config. The body of `remote_save_server`, extracted so callers
/// that already hold the state lock — or that must not move an entry between
/// folders — go through the exact same path.
fn upsert_server(state: &mut RemoteState, entry: ServerEntry, folder_id: Option<String>) {
    // Remove existing if updating.
    state.config.remove_server(&entry.id);
    if let Some(fid) = folder_id {
        state.config.add_server_to_folder(entry, &fid);
    } else {
        state.config.add_server(entry);
    }
    termlab_remote::config::save_config(&state.paths.config_dir, &state.config);
}

/// Persist a modified server entry, leaving it in whatever folder it already
/// occupies.
///
/// `remote_save_server` takes the folder as an explicit argument because its
/// caller is a form with a folder picker; a background edit (the SFTP connect
/// path linking a vault account, say) has no such intent and must not silently
/// move a foldered host to the ungrouped list.
pub(super) fn save_server_preserving_folder(state: &mut RemoteState, entry: ServerEntry) {
    let folder_id = state
        .config
        .find_server_folder(&entry.id)
        .map(str::to_string);
    upsert_server(state, entry, folder_id);
}

#[tauri::command]
pub(crate) fn remote_delete_server(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    server_id: String,
) {
    let mut state = remote.lock();
    state.config.remove_server(&server_id);
    termlab_remote::config::save_config(&state.paths.config_dir, &state.config);
}

#[tauri::command]
pub(crate) fn remote_add_folder(remote: tauri::State<'_, Arc<Mutex<RemoteState>>>, name: String) {
    let mut state = remote.lock();
    state.config.add_folder(&name);
    termlab_remote::config::save_config(&state.paths.config_dir, &state.config);
}

#[tauri::command]
pub(crate) fn remote_delete_folder(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    folder_id: String,
) {
    let mut state = remote.lock();
    state.config.remove_folder(&folder_id);
    termlab_remote::config::save_config(&state.paths.config_dir, &state.config);
}

#[tauri::command]
pub(crate) fn remote_import_ssh_config(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
) -> Vec<ServerEntry> {
    let mut state = remote.lock();
    state.ssh_config_entries = termlab_remote::config::parse_ssh_config();
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
    termlab_remote::config::save_config(&state.paths.config_dir, &state.config);
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
    termlab_remote::config::save_config(&state.paths.config_dir, &state.config);
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
        termlab_remote::config::save_config(&state.paths.config_dir, &state.config);
    }
}

/// Read and parse a legacy plaintext JSON export file — the file-reading
/// half of what used to be the (now-removed, 2026-08-16 review finding M17:
/// no frontend caller was left) `remote_import` command. Used by
/// `share_commands::share_import_apply`, which already has a path chosen by
/// `share_pick_import_file` and only needs the file read and validated.
pub(crate) fn read_legacy_export_payload(
    file_path: &std::path::Path,
) -> Result<ExportPayload, String> {
    let json =
        std::fs::read_to_string(file_path).map_err(|e| format!("Failed to read file: {e}"))?;
    let payload: ExportPayload =
        serde_json::from_str(&json).map_err(|e| format!("Invalid import file: {e}"))?;
    if payload.version != 1 {
        return Err(format!("Unsupported export version: {}", payload.version));
    }
    Ok(payload)
}

/// Apply an already-parsed legacy `payload` to `state`: merge it in
/// (regenerating ids, as `merge_import` always has), resolve newly
/// imported tunnels' session keys against known servers, optionally link
/// legacy entries to skeleton vault accounts (`vault_eager_import`), and
/// persist. Returns the same `(servers, folders, tunnels)` counts
/// `merge_import` reports.
///
/// This is the applying half of what used to be the removed `remote_import`
/// command — split out so `share_commands::share_import_apply` can drive the
/// exact same logic for a `legacy_json`-kind file it already has an open
/// path for, without duplicating any of it. Nothing about
/// `merge_import`, the regenerated-UUID semantics, or
/// `resolve_imported_tunnel_keys` changes here.
pub(crate) fn apply_legacy_import(
    state: &mut RemoteState,
    vault: &VaultState,
    app: &tauri::AppHandle,
    payload: ExportPayload,
) -> (usize, usize, usize) {
    let existing_tunnel_ids: Vec<uuid::Uuid> = state.config.tunnels.iter().map(|t| t.id).collect();

    // Capture pre-import lengths so we can find newly added entries afterwards.
    let ungrouped_before = state.config.ungrouped.len();
    let folders_before = state.config.folders.len();

    let (servers, folders, tunnels) = state.config.merge_import(payload);

    // Resolve session_keys of newly imported tunnels: if a tunnel's host
    // matches a known server with a different user, rewrite the session_key
    // so it matches on activation without needing an edit+save cycle.
    resolve_imported_tunnel_keys(state, &existing_tunnel_ids);

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
        let _ = (ungrouped_before, folders_before, vault);
    }

    termlab_remote::config::save_config(&state.paths.config_dir, &state.config);
    let _ = app.emit(SSH_CONFIG_CHANGED_EVENT, ());
    (servers, folders, tunnels)
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
        termlab_remote::config::save_config(&state.paths.config_dir, &state.config);
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
    active_sessions(&state)
}

/// Every live session, terminal-owned and detached alike — they live in the
/// same map, so a detached SFTP connection is visible to the chooser sidebar
/// and the panel without any special case here.
fn active_sessions(state: &RemoteState) -> Vec<ActiveSession> {
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
    vault: &termlab_vault::VaultManager,
    entries: &mut [&mut ServerEntry],
) -> Result<usize, String> {
    use std::path::PathBuf;
    let mut count = 0;
    for entry in entries.iter_mut() {
        if entry.vault_account_id.is_none() {
            if let Some(user) = &entry.user {
                let auth = match &entry.key_path {
                    Some(kp) => termlab_vault::AuthMethod::Key {
                        path: PathBuf::from(kp),
                        passphrase: None,
                    },
                    None => termlab_vault::AuthMethod::Password(String::new()),
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

    use termlab_remote::callbacks::RemotePaths;
    use termlab_remote::config::{ServerEntry, SshConfig};
    use termlab_remote::tunnel::TunnelManager;

    use super::super::PendingPrompts;

    /// Build a minimal RemoteState for testing (no config files, no SSH config).
    fn test_state_with(config: SshConfig, ssh_config_entries: Vec<ServerEntry>) -> RemoteState {
        RemoteState {
            sessions: HashMap::new(),
            connections: HashMap::new(),
            config,
            ssh_config_entries,
            pending_prompts: Arc::new(Mutex::new(PendingPrompts::new())),
            tunnel_manager: TunnelManager::new(),
            paths: RemotePaths {
                known_hosts_file: std::path::PathBuf::from("/tmp/test_known_hosts"),
                config_dir: std::path::PathBuf::from("/tmp/test_config"),
                default_key_paths: vec![],
            },
            pane_cwds: HashMap::new(),
            pane_cwd_buffers: HashMap::new(),
            pane_input_buffers: HashMap::new(),
            pane_prev_cwds: HashMap::new(),
            pane_cwd_needs_sync: HashMap::new(),
            pane_home_dirs: HashMap::new(),
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
    // Session listing + background entry saves
    // ---------------------------------------------------------------------------

    #[test]
    fn active_sessions_include_detached_sftp_sessions() {
        use super::super::detached_commands::{DETACHED_PANE_ID_BASE, detached_session};

        let mut state = test_state_with(SshConfig::default(), vec![]);
        state.sessions.insert(
            "main:1".to_string(),
            detached_session(
                "conn:main:1".to_string(),
                "tab.example.com".to_string(),
                "alice".to_string(),
                22,
                "entry-tab".to_string(),
            ),
        );
        let detached_key = format!("main:{DETACHED_PANE_ID_BASE}");
        state.sessions.insert(
            detached_key.clone(),
            detached_session(
                format!("conn:main:{DETACHED_PANE_ID_BASE}"),
                "panel.example.com".to_string(),
                "deploy".to_string(),
                2222,
                "entry-panel".to_string(),
            ),
        );

        let sessions = active_sessions(&state);
        assert_eq!(sessions.len(), 2);
        let detached = sessions
            .iter()
            .find(|s| s.key == detached_key)
            .expect("detached session must be listed like any other");
        assert_eq!(detached.host, "panel.example.com");
        assert_eq!(detached.user, "deploy");
        assert_eq!(detached.port, 2222);
    }

    #[test]
    fn saving_preserves_the_entrys_folder_and_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = SshConfig::default();
        cfg.add_folder("Work");
        let folder_id = cfg.folders[0].id.clone();
        let mut entry = make_server("build-box", "build.example.com", "deploy", 22);
        entry.id = "entry-1".to_string();
        cfg.add_server_to_folder(entry.clone(), &folder_id);

        let mut state = test_state_with(cfg, vec![]);
        state.paths.config_dir = dir.path().to_path_buf();

        let account_id = uuid::Uuid::new_v4();
        entry.vault_account_id = Some(account_id);
        save_server_preserving_folder(&mut state, entry);

        assert!(
            state.config.ungrouped.is_empty(),
            "a background save must not move the host out of its folder"
        );
        assert_eq!(state.config.folders[0].entries.len(), 1);
        assert_eq!(
            state.config.folders[0].entries[0].vault_account_id,
            Some(account_id)
        );

        // The link must outlive the process, not just the session.
        let reloaded = termlab_remote::config::load_config(dir.path());
        assert_eq!(
            reloaded.find_server("entry-1").unwrap().vault_account_id,
            Some(account_id)
        );
        assert_eq!(reloaded.find_server_folder("entry-1"), Some(&*folder_id));
    }

    #[test]
    fn saving_an_ungrouped_entry_keeps_it_ungrouped() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = SshConfig::default();
        cfg.add_folder("Work");
        let mut entry = make_server("solo", "solo.example.com", "root", 22);
        entry.id = "entry-2".to_string();
        cfg.add_server(entry.clone());

        let mut state = test_state_with(cfg, vec![]);
        state.paths.config_dir = dir.path().to_path_buf();

        entry.vault_account_id = Some(uuid::Uuid::new_v4());
        save_server_preserving_folder(&mut state, entry);

        assert_eq!(state.config.ungrouped.len(), 1);
        assert!(state.config.folders[0].entries.is_empty());
    }

    #[test]
    fn saving_an_unknown_entry_adds_it_ungrouped() {
        // An `~/.ssh/config` host has no config-owned home yet; linking one
        // promotes it into the config file as an ungrouped entry.
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with(SshConfig::default(), vec![]);
        state.paths.config_dir = dir.path().to_path_buf();

        let mut entry = make_server("imported", "imported.example.com", "admin", 22);
        entry.vault_account_id = Some(uuid::Uuid::new_v4());
        let entry_id = entry.id.clone();
        save_server_preserving_folder(&mut state, entry);

        assert_eq!(state.config.ungrouped.len(), 1);
        let reloaded = termlab_remote::config::load_config(dir.path());
        assert!(reloaded.find_server(&entry_id).is_some());
    }

    // ---------------------------------------------------------------------------
    // Vault eager import tests (feature-gated)
    // ---------------------------------------------------------------------------

    #[cfg(feature = "vault_eager_import")]
    #[test]
    fn eager_import_creates_vault_account_for_entry_with_user() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = termlab_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test-password").unwrap();

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
        let mgr = termlab_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test-password").unwrap();

        let mut entry = make_server("bastion", "bastion.example.com", "admin", 22);
        entry.key_path = Some("/home/admin/.ssh/id_ed25519".into());

        let mut entries: Vec<&mut ServerEntry> = vec![&mut entry];
        eagerly_create_vault_accounts(&mgr, &mut entries).unwrap();

        let accounts = mgr.list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        match &accounts[0].auth {
            termlab_vault::AuthMethod::Key { path, passphrase } => {
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
        let mgr = termlab_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test-password").unwrap();

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
        let mgr = termlab_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test-password").unwrap();
        let existing_id = mgr
            .add_account(
                "existing".into(),
                "root".into(),
                termlab_vault::AuthMethod::Password(String::new()),
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
