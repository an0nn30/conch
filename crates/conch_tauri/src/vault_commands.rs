use conch_vault::{AuthMethod, VaultAccount, VaultManager, VaultSettings};
use conch_vault::keygen::{generate_key, save_key_to_disk, KeyGenOptions, KeyType};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::Mutex;
use uuid::Uuid;

use crate::remote::RemoteState;

pub(crate) type VaultState = Arc<Mutex<VaultManager>>;

// --- Request/Response types for frontend ---

#[derive(Deserialize)]
pub(crate) struct CreateVaultRequest {
    pub password: String,
    pub enable_keychain: bool,
}

#[derive(Deserialize)]
pub(crate) struct UnlockVaultRequest {
    pub password: String,
}

#[derive(Serialize)]
pub(crate) struct VaultStatusResponse {
    pub exists: bool,
    pub locked: bool,
    pub seconds_remaining: u64,
}

#[derive(Serialize)]
pub(crate) struct AccountResponse {
    pub id: Uuid,
    pub display_name: String,
    pub username: String,
    pub auth_type: String,
    pub key_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<VaultAccount> for AccountResponse {
    fn from(a: VaultAccount) -> Self {
        let (auth_type, key_path) = match &a.auth {
            AuthMethod::Password(_) => ("password".into(), None),
            AuthMethod::Key { path, .. } => ("key".into(), Some(path.display().to_string())),
            AuthMethod::KeyAndPassword { key_path, .. } => {
                ("key_and_password".into(), Some(key_path.display().to_string()))
            }
        };
        Self {
            id: a.id,
            display_name: a.display_name,
            username: a.username,
            auth_type,
            key_path,
            created_at: a.created_at.to_rfc3339(),
            updated_at: a.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct AddAccountRequest {
    pub display_name: String,
    pub username: String,
    pub auth_type: String,
    pub password: Option<String>,
    pub key_path: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct UpdateAccountRequest {
    pub id: Uuid,
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub auth_type: Option<String>,
    pub password: Option<String>,
    pub key_path: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct KeyGenRequest {
    pub key_type: String,
    pub comment: String,
    pub passphrase: Option<String>,
    pub save_path: String,
}

#[derive(Serialize)]
pub(crate) struct KeyGenResponse {
    pub fingerprint: String,
    pub public_key: String,
    pub algorithm: String,
    pub private_path: String,
    pub public_path: String,
}

// --- Tauri commands ---

#[tauri::command]
pub(crate) fn vault_status(vault: tauri::State<'_, VaultState>) -> VaultStatusResponse {
    let mgr = vault.lock();
    mgr.check_timeout();
    VaultStatusResponse {
        exists: mgr.vault_exists(),
        locked: mgr.is_locked(),
        seconds_remaining: mgr.seconds_remaining(),
    }
}

#[tauri::command]
pub(crate) fn vault_create(
    vault: tauri::State<'_, VaultState>,
    request: CreateVaultRequest,
) -> Result<(), String> {
    let mgr = vault.lock();
    mgr.create(request.password.as_bytes()).map_err(|e| e.to_string())?;

    if request.enable_keychain {
        // Store derived key in OS keychain for biometric unlock.
        // This is a best-effort operation.
        if let Err(e) = conch_vault::keychain::store_master_key(request.password.as_bytes()) {
            log::warn!("failed to store master key in keychain: {e}");
        }
    }

    Ok(())
}

#[tauri::command]
pub(crate) fn vault_unlock(
    vault: tauri::State<'_, VaultState>,
    request: UnlockVaultRequest,
) -> Result<(), String> {
    let mgr = vault.lock();
    mgr.unlock(request.password.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn vault_lock(vault: tauri::State<'_, VaultState>) {
    vault.lock().lock();
}

#[tauri::command]
pub(crate) fn vault_list_accounts(
    vault: tauri::State<'_, VaultState>,
) -> Result<Vec<AccountResponse>, String> {
    let mgr = vault.lock();
    let accounts = mgr.list_accounts().map_err(|e| e.to_string())?;
    Ok(accounts.into_iter().map(AccountResponse::from).collect())
}

#[tauri::command]
pub(crate) fn vault_get_account(
    vault: tauri::State<'_, VaultState>,
    id: Uuid,
) -> Result<AccountResponse, String> {
    let mgr = vault.lock();
    let account = mgr.get_account(id).map_err(|e| e.to_string())?;
    Ok(AccountResponse::from(account))
}

#[tauri::command]
pub(crate) fn vault_add_account(
    vault: tauri::State<'_, VaultState>,
    request: AddAccountRequest,
) -> Result<Uuid, String> {
    let auth = parse_auth_method(&request.auth_type, &request)?;
    let mgr = vault.lock();
    mgr.add_account(request.display_name, request.username, auth)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn vault_update_account(
    vault: tauri::State<'_, VaultState>,
    request: UpdateAccountRequest,
) -> Result<(), String> {
    let auth = request.auth_type.as_ref().map(|at| {
        parse_auth_method(at, &AddAccountRequest {
            display_name: String::new(),
            username: String::new(),
            auth_type: at.clone(),
            password: request.password.clone(),
            key_path: request.key_path.clone(),
            passphrase: request.passphrase.clone(),
        })
    }).transpose().map_err(|e: String| e)?;

    let mgr = vault.lock();
    mgr.update_account(request.id, request.display_name, request.username, auth)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn vault_delete_account(
    vault: tauri::State<'_, VaultState>,
    id: Uuid,
) -> Result<bool, String> {
    let mgr = vault.lock();
    mgr.delete_account(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn vault_get_settings(
    vault: tauri::State<'_, VaultState>,
) -> Result<VaultSettings, String> {
    let mgr = vault.lock();
    mgr.get_settings().map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn vault_update_settings(
    vault: tauri::State<'_, VaultState>,
    settings: VaultSettings,
) -> Result<(), String> {
    let mgr = vault.lock();
    mgr.update_settings(settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn vault_generate_key(
    request: KeyGenRequest,
) -> Result<KeyGenResponse, String> {
    let key_type = match request.key_type.as_str() {
        "ed25519" => KeyType::Ed25519,
        "ecdsa-p256" => KeyType::EcdsaP256,
        "ecdsa-p384" => KeyType::EcdsaP384,
        "rsa-2048" => KeyType::Rsa2048,
        "rsa-4096" => KeyType::Rsa4096,
        other => return Err(format!("unknown key type: {other}")),
    };
    let options = KeyGenOptions {
        key_type,
        comment: request.comment,
        passphrase: request.passphrase,
    };

    let key = generate_key(&options).map_err(|e| e.to_string())?;
    let save_path = PathBuf::from(&request.save_path);
    save_key_to_disk(&save_path, &key).map_err(|e| e.to_string())?;

    Ok(KeyGenResponse {
        fingerprint: key.fingerprint,
        public_key: key.public_key,
        algorithm: key.algorithm,
        private_path: save_path.display().to_string(),
        public_path: save_path.with_extension("pub").display().to_string(),
    })
}

/// Migrate legacy server entries (those with plain-text `user`/`key_path` fields
/// but no vault account) to the credential vault.
///
/// Steps:
/// 1. Collect unique (user, key_path) combinations from legacy entries.
/// 2. Create a vault account for each unique combination.
/// 3. Link each server entry to the matching vault account and clear legacy fields.
/// 4. Link tunnel `session_key` values to server entries where possible.
/// 5. Back up `servers.json` → `servers.json.bak` and save the updated config.
///
/// Returns the number of vault accounts created.
///
/// **The vault must be unlocked before calling this command.** If the vault is
/// locked the command returns an error.
#[tauri::command]
pub(crate) fn vault_migrate_legacy(
    vault: tauri::State<'_, VaultState>,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
) -> Result<usize, String> {
    let mut state = remote.lock();

    // Collect unique credentials from the existing config.
    let unique_creds = state.config.collect_unique_credentials();
    if unique_creds.is_empty() {
        return Ok(0);
    }

    // Build a mapping from (user, key_path) → vault account UUID.
    let vault_mgr = vault.lock();
    let mut cred_to_uuid: std::collections::HashMap<(String, Option<String>), Uuid> =
        std::collections::HashMap::new();

    for (user, key_path, hint) in &unique_creds {
        let auth = match key_path {
            Some(kp) => AuthMethod::Key {
                path: PathBuf::from(kp),
                passphrase: None,
            },
            None => AuthMethod::Password(String::new()),
        };
        let id = vault_mgr
            .add_account(hint.clone(), user.clone(), auth)
            .map_err(|e| format!("failed to create vault account for '{hint}': {e}"))?;
        cred_to_uuid.insert((user.clone(), key_path.clone()), id);
    }
    // Save after all accounts are written.
    vault_mgr.save().map_err(|e| format!("failed to save vault: {e}"))?;
    drop(vault_mgr);

    // Link each legacy server entry to its vault account and clear legacy fields.
    for entry in state.config.ungrouped.iter_mut() {
        if entry.vault_account_id.is_some() {
            continue;
        }
        if let Some(user) = entry.user.clone() {
            let key = (user, entry.key_path.clone());
            if let Some(&uid) = cred_to_uuid.get(&key) {
                entry.vault_account_id = Some(uid);
                entry.user = None;
                entry.auth_method = None;
                entry.key_path = None;
            }
        }
    }
    for folder in state.config.folders.iter_mut() {
        for entry in folder.entries.iter_mut() {
            if entry.vault_account_id.is_some() {
                continue;
            }
            if let Some(user) = entry.user.clone() {
                let key = (user, entry.key_path.clone());
                if let Some(&uid) = cred_to_uuid.get(&key) {
                    entry.vault_account_id = Some(uid);
                    entry.user = None;
                    entry.auth_method = None;
                    entry.key_path = None;
                }
            }
        }
    }

    // Migrate tunnel session_keys → server_entry_ids where possible.
    // session_key format: "user@host:port"
    // Build the host→id lookup table first (borrows config immutably), then
    // apply it to the tunnels (mutable borrow) in a separate pass.
    let tunnel_id_map: Vec<(usize, String)> = state
        .config
        .tunnels
        .iter()
        .enumerate()
        .filter(|(_, t)| t.server_entry_id.is_none() && !t.session_key.is_empty())
        .filter_map(|(idx, t)| {
            let (_user, host, port) =
                conch_remote::config::SavedTunnel::parse_session_key(&t.session_key)?;
            let matched_id = state
                .config
                .all_servers()
                .find(|s| s.host == host && s.port == port)
                .map(|s| s.id.clone())?;
            Some((idx, matched_id))
        })
        .collect();

    for (idx, server_id) in tunnel_id_map {
        state.config.tunnels[idx].server_entry_id = Some(server_id);
    }

    // Back up servers.json → servers.json.bak, then save the updated config.
    let servers_json = state.paths.config_dir.join("servers.json");
    let bak = state.paths.config_dir.join("servers.json.bak");
    if servers_json.exists() {
        if let Err(e) = std::fs::copy(&servers_json, &bak) {
            log::warn!("could not back up servers.json: {e}");
        }
    }
    conch_remote::config::save_config(&state.paths.config_dir, &state.config);

    Ok(unique_creds.len())
}

fn parse_auth_method(auth_type: &str, req: &AddAccountRequest) -> Result<AuthMethod, String> {
    match auth_type {
        "password" => {
            let pw = req.password.clone().unwrap_or_default();
            Ok(AuthMethod::Password(pw))
        }
        "key" => {
            let path = req.key_path.as_ref().ok_or("key_path required for key auth")?;
            Ok(AuthMethod::Key {
                path: PathBuf::from(path),
                passphrase: req.passphrase.clone(),
            })
        }
        "key_and_password" => {
            let key_path = req.key_path.as_ref().ok_or("key_path required")?;
            let password = req.password.clone().unwrap_or_default();
            Ok(AuthMethod::KeyAndPassword {
                key_path: PathBuf::from(key_path),
                passphrase: req.passphrase.clone(),
                password,
            })
        }
        other => Err(format!("unknown auth type: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conch_vault::VaultManager;

    #[test]
    fn parse_auth_method_password() {
        let req = AddAccountRequest {
            display_name: "Test".into(),
            username: "user".into(),
            auth_type: "password".into(),
            password: Some("secret".into()),
            key_path: None,
            passphrase: None,
        };
        let auth = parse_auth_method("password", &req).unwrap();
        assert!(matches!(auth, AuthMethod::Password(ref p) if p == "secret"));
    }

    #[test]
    fn parse_auth_method_key() {
        let req = AddAccountRequest {
            display_name: "Test".into(),
            username: "user".into(),
            auth_type: "key".into(),
            password: None,
            key_path: Some("/home/user/.ssh/id_ed25519".into()),
            passphrase: None,
        };
        let auth = parse_auth_method("key", &req).unwrap();
        match auth {
            AuthMethod::Key { ref path, ref passphrase } => {
                assert_eq!(path.to_str().unwrap(), "/home/user/.ssh/id_ed25519");
                assert!(passphrase.is_none());
            }
            _ => panic!("expected AuthMethod::Key"),
        }
    }

    #[test]
    fn parse_auth_method_key_requires_path() {
        let req = AddAccountRequest {
            display_name: "Test".into(),
            username: "user".into(),
            auth_type: "key".into(),
            password: None,
            key_path: None,
            passphrase: None,
        };
        let result = parse_auth_method("key", &req);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("key_path required"));
    }

    #[test]
    fn parse_auth_method_key_and_password() {
        let req = AddAccountRequest {
            display_name: "Test".into(),
            username: "user".into(),
            auth_type: "key_and_password".into(),
            password: Some("serverpass".into()),
            key_path: Some("/home/user/.ssh/id_rsa".into()),
            passphrase: Some("keypass".into()),
        };
        let auth = parse_auth_method("key_and_password", &req).unwrap();
        match auth {
            AuthMethod::KeyAndPassword { ref key_path, ref passphrase, ref password } => {
                assert_eq!(key_path.to_str().unwrap(), "/home/user/.ssh/id_rsa");
                assert_eq!(passphrase.as_deref().unwrap(), "keypass");
                assert_eq!(password, "serverpass");
            }
            _ => panic!("expected AuthMethod::KeyAndPassword"),
        }
    }

    #[test]
    fn parse_auth_method_unknown_returns_error() {
        let req = AddAccountRequest {
            display_name: "Test".into(),
            username: "user".into(),
            auth_type: "unknown".into(),
            password: None,
            key_path: None,
            passphrase: None,
        };
        let result = parse_auth_method("unknown", &req);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown auth type"));
    }

    fn make_account_via_manager(
        display_name: &str,
        username: &str,
        auth: AuthMethod,
    ) -> VaultAccount {
        let dir = tempfile::tempdir().unwrap();
        let mgr = VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();
        let id = mgr.add_account(display_name.into(), username.into(), auth).unwrap();
        mgr.get_account(id).unwrap()
    }

    #[test]
    fn account_response_from_password_account() {
        let account = make_account_via_manager(
            "My Server",
            "root",
            AuthMethod::Password("pass".into()),
        );
        let resp = AccountResponse::from(account);
        assert_eq!(resp.auth_type, "password");
        assert!(resp.key_path.is_none());
        assert_eq!(resp.username, "root");
    }

    #[test]
    fn account_response_from_key_account() {
        let account = make_account_via_manager(
            "Key Auth",
            "deploy",
            AuthMethod::Key {
                path: PathBuf::from("/home/deploy/.ssh/id_ed25519"),
                passphrase: None,
            },
        );
        let resp = AccountResponse::from(account);
        assert_eq!(resp.auth_type, "key");
        assert_eq!(resp.key_path.as_deref().unwrap(), "/home/deploy/.ssh/id_ed25519");
    }

    #[test]
    fn vault_status_response_serializes() {
        let resp = VaultStatusResponse {
            exists: true,
            locked: false,
            seconds_remaining: 900,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"exists\":true"));
        assert!(json.contains("\"locked\":false"));
        assert!(json.contains("\"seconds_remaining\":900"));
    }
}
