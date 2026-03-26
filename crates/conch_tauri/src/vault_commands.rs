use conch_vault::keygen::{KeyGenOptions, KeyType, generate_key, save_key_to_disk};
use conch_vault::{AuthMethod, GeneratedKeyEntry, VaultAccount, VaultManager, VaultSettings};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use crate::remote::RemoteState;

pub(crate) type VaultState = Arc<Mutex<VaultManager>>;

// --- Request/Response types for frontend ---

#[derive(Deserialize)]
pub(crate) struct CreateVaultRequest {
    pub password: String,
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
            AuthMethod::KeyAndPassword { key_path, .. } => (
                "key_and_password".into(),
                Some(key_path.display().to_string()),
            ),
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
pub(crate) async fn vault_create(
    vault: tauri::State<'_, VaultState>,
    request: CreateVaultRequest,
) -> Result<(), String> {
    let vault = vault.inner().clone();
    tokio::task::spawn_blocking(move || {
        let mgr = vault.lock();
        mgr.create(request.password.as_bytes())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn vault_unlock(
    vault: tauri::State<'_, VaultState>,
    request: UnlockVaultRequest,
) -> Result<(), String> {
    let vault = vault.inner().clone();
    tokio::task::spawn_blocking(move || {
        let mgr = vault.lock();
        mgr.unlock(request.password.as_bytes())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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
    let auth = parse_auth_method(
        &request.auth_type,
        request.password.as_deref(),
        request.key_path.as_deref(),
        request.passphrase.as_deref(),
    )?;
    let mgr = vault.lock();
    let id = mgr
        .add_account(request.display_name, request.username, auth)
        .map_err(|e| e.to_string())?;
    mgr.save().map_err(|e| e.to_string())?;
    Ok(id)
}

#[tauri::command]
pub(crate) fn vault_update_account(
    vault: tauri::State<'_, VaultState>,
    request: UpdateAccountRequest,
) -> Result<(), String> {
    let auth = request
        .auth_type
        .as_ref()
        .map(|at| {
            parse_auth_method(
                at,
                request.password.as_deref(),
                request.key_path.as_deref(),
                request.passphrase.as_deref(),
            )
        })
        .transpose()
        .map_err(|e: String| e)?;

    let mgr = vault.lock();
    mgr.update_account(request.id, request.display_name, request.username, auth)
        .map_err(|e| e.to_string())?;
    mgr.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) fn vault_delete_account(
    vault: tauri::State<'_, VaultState>,
    id: Uuid,
) -> Result<bool, String> {
    let mgr = vault.lock();
    let removed = mgr.delete_account(id).map_err(|e| e.to_string())?;
    mgr.save().map_err(|e| e.to_string())?;
    Ok(removed)
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
    mgr.update_settings(settings).map_err(|e| e.to_string())?;
    mgr.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn vault_pick_key_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_file(move |path| {
        let _ = tx.send(path.and_then(|p| p.as_path().map(|pp| pp.display().to_string())));
    });
    rx.await.map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn vault_check_path_exists(path: String) -> bool {
    let expanded = conch_remote::ssh::expand_tilde(&path);
    expanded.exists() || expanded.with_extension("pub").exists()
}

#[tauri::command]
pub(crate) fn vault_generate_key(
    vault: tauri::State<'_, VaultState>,
    request: KeyGenRequest,
) -> Result<KeyGenResponse, String> {
    let key_type = match request.key_type.as_str() {
        "ed25519" => KeyType::Ed25519,
        "ecdsa-p256" => KeyType::EcdsaP256,
        "ecdsa-p384" => KeyType::EcdsaP384,
        "rsa-sha256" => KeyType::RsaSha256,
        "rsa-sha512" => KeyType::RsaSha512,
        other => return Err(format!("unknown key type: {other}")),
    };
    let comment = request.comment.clone();
    let options = KeyGenOptions {
        key_type,
        comment: request.comment,
        passphrase: request.passphrase,
    };

    // Check vault is unlocked before touching the filesystem.
    {
        let mgr = vault.lock();
        if mgr.is_locked() {
            return Err("Vault is locked — unlock it before generating keys".into());
        }
    }

    let key = generate_key(&options).map_err(|e| e.to_string())?;
    let save_path = conch_remote::ssh::expand_tilde(&request.save_path);
    save_key_to_disk(&save_path, &key).map_err(|e| e.to_string())?;

    let public_path = save_path.with_extension("pub");

    // Record the generated key in the vault.
    let mgr = vault.lock();
    mgr.add_generated_key(
        key.algorithm.clone(),
        key.fingerprint.clone(),
        comment,
        save_path.clone(),
        public_path.clone(),
    )
    .map_err(|e| e.to_string())?;
    mgr.save().map_err(|e| e.to_string())?;

    Ok(KeyGenResponse {
        fingerprint: key.fingerprint,
        public_key: key.public_key,
        algorithm: key.algorithm,
        private_path: save_path.display().to_string(),
        public_path: public_path.display().to_string(),
    })
}

#[derive(Serialize)]
pub(crate) struct GeneratedKeyResponse {
    pub id: Uuid,
    pub algorithm: String,
    pub fingerprint: String,
    pub comment: String,
    pub private_path: String,
    pub public_path: String,
    pub created_at: String,
}

impl From<GeneratedKeyEntry> for GeneratedKeyResponse {
    fn from(k: GeneratedKeyEntry) -> Self {
        Self {
            id: k.id,
            algorithm: k.algorithm,
            fingerprint: k.fingerprint,
            comment: k.comment,
            private_path: k.private_path.display().to_string(),
            public_path: k.public_path.display().to_string(),
            created_at: k.created_at.to_rfc3339(),
        }
    }
}

#[tauri::command]
pub(crate) fn vault_list_keys(
    vault: tauri::State<'_, VaultState>,
) -> Result<Vec<GeneratedKeyResponse>, String> {
    let mgr = vault.lock();
    let keys = mgr.list_generated_keys().map_err(|e| e.to_string())?;
    Ok(keys.into_iter().map(GeneratedKeyResponse::from).collect())
}

#[tauri::command]
pub(crate) fn vault_delete_key(
    vault: tauri::State<'_, VaultState>,
    id: Uuid,
) -> Result<bool, String> {
    let mgr = vault.lock();
    let removed = mgr.delete_generated_key(id).map_err(|e| e.to_string())?;
    mgr.save().map_err(|e| e.to_string())?;
    Ok(removed)
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
        if matches!(&auth, AuthMethod::Password(p) if p.is_empty()) {
            log::warn!(
                "Migrated server '{}' with empty password — will prompt on connect",
                hint
            );
        }
        let id = vault_mgr
            .add_account(hint.clone(), user.clone(), auth)
            .map_err(|e| format!("failed to create vault account for '{hint}': {e}"))?;
        cred_to_uuid.insert((user.clone(), key_path.clone()), id);
    }
    // Save after all accounts are written.
    vault_mgr
        .save()
        .map_err(|e| format!("failed to save vault: {e}"))?;
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

fn parse_auth_method(
    auth_type: &str,
    password: Option<&str>,
    key_path: Option<&str>,
    passphrase: Option<&str>,
) -> Result<AuthMethod, String> {
    match auth_type {
        "password" => {
            let pw = password.unwrap_or_default().to_owned();
            Ok(AuthMethod::Password(pw))
        }
        "key" => {
            let path = key_path.ok_or("key_path required for key auth")?;
            Ok(AuthMethod::Key {
                path: PathBuf::from(path),
                passphrase: passphrase.map(str::to_owned),
            })
        }
        "key_and_password" => {
            let kp = key_path.ok_or("key_path required")?;
            let pw = password.unwrap_or_default().to_owned();
            Ok(AuthMethod::KeyAndPassword {
                key_path: PathBuf::from(kp),
                passphrase: passphrase.map(str::to_owned),
                password: pw,
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
        let auth = parse_auth_method("password", Some("secret"), None, None).unwrap();
        assert!(matches!(auth, AuthMethod::Password(ref p) if p == "secret"));
    }

    #[test]
    fn parse_auth_method_key() {
        let auth =
            parse_auth_method("key", None, Some("/home/user/.ssh/id_ed25519"), None).unwrap();
        match auth {
            AuthMethod::Key {
                ref path,
                ref passphrase,
            } => {
                assert_eq!(path.to_str().unwrap(), "/home/user/.ssh/id_ed25519");
                assert!(passphrase.is_none());
            }
            _ => panic!("expected AuthMethod::Key"),
        }
    }

    #[test]
    fn parse_auth_method_key_requires_path() {
        let result = parse_auth_method("key", None, None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("key_path required"));
    }

    #[test]
    fn parse_auth_method_key_and_password() {
        let auth = parse_auth_method(
            "key_and_password",
            Some("serverpass"),
            Some("/home/user/.ssh/id_rsa"),
            Some("keypass"),
        )
        .unwrap();
        match auth {
            AuthMethod::KeyAndPassword {
                ref key_path,
                ref passphrase,
                ref password,
            } => {
                assert_eq!(key_path.to_str().unwrap(), "/home/user/.ssh/id_rsa");
                assert_eq!(passphrase.as_deref().unwrap(), "keypass");
                assert_eq!(password, "serverpass");
            }
            _ => panic!("expected AuthMethod::KeyAndPassword"),
        }
    }

    #[test]
    fn parse_auth_method_unknown_returns_error() {
        let result = parse_auth_method("unknown", None, None, None);
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
        let id = mgr
            .add_account(display_name.into(), username.into(), auth)
            .unwrap();
        mgr.get_account(id).unwrap()
    }

    #[test]
    fn account_response_from_password_account() {
        let account =
            make_account_via_manager("My Server", "root", AuthMethod::Password("pass".into()));
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
        assert_eq!(
            resp.key_path.as_deref().unwrap(),
            "/home/deploy/.ssh/id_ed25519"
        );
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

    #[test]
    fn check_path_exists_detects_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id_test");
        std::fs::write(&path, "key data").unwrap();
        assert!(vault_check_path_exists(path.display().to_string()));
    }

    #[test]
    fn check_path_exists_detects_pub_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id_test");
        let pub_path = path.with_extension("pub");
        std::fs::write(&pub_path, "pub key data").unwrap();
        assert!(vault_check_path_exists(path.display().to_string()));
    }

    #[test]
    fn check_path_exists_false_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent_key");
        assert!(!vault_check_path_exists(path.display().to_string()));
    }
}
