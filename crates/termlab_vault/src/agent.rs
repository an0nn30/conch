use crate::error::VaultError;
use crate::model::{AuthMethod, VaultAccount};
use parking_lot::Mutex;
use ssh_key::PrivateKey;
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

/// In-memory key store for vault-managed SSH keys.
pub struct SshAgent {
    /// Loaded private keys, keyed by vault account ID.
    keys: Mutex<HashMap<Uuid, LoadedKey>>,
}

pub struct LoadedKey {
    pub account_id: Uuid,
    pub username: String,
    pub private_key: PrivateKey,
}

impl SshAgent {
    pub fn new() -> Self {
        Self {
            keys: Mutex::new(HashMap::new()),
        }
    }

    /// Load keys from vault accounts. Reads key files from disk.
    pub fn load_keys(&self, accounts: &[VaultAccount]) {
        let mut keys = self.keys.lock();
        keys.clear();

        for account in accounts {
            let key_path = match &account.auth {
                AuthMethod::Key { path, .. } => Some(path),
                AuthMethod::KeyAndPassword { key_path, .. } => Some(key_path),
                AuthMethod::Password(_) => None,
            };

            if let Some(path) = key_path {
                match load_private_key(path, &account.auth) {
                    Ok(pk) => {
                        keys.insert(
                            account.id,
                            LoadedKey {
                                account_id: account.id,
                                username: account.username.clone(),
                                private_key: pk,
                            },
                        );
                        log::info!("agent: loaded key for account '{}'", account.display_name);
                    }
                    Err(e) => {
                        log::warn!(
                            "agent: failed to load key for '{}': {}",
                            account.display_name,
                            e
                        );
                    }
                }
            }
        }
    }

    /// Get a loaded key by vault account ID.
    pub fn get_key(&self, account_id: Uuid) -> Option<PrivateKey> {
        self.keys
            .lock()
            .get(&account_id)
            .map(|k| k.private_key.clone())
    }

    /// Get all loaded key account IDs.
    pub fn loaded_account_ids(&self) -> Vec<Uuid> {
        self.keys.lock().keys().cloned().collect()
    }

    /// Clear all loaded keys from memory.
    pub fn clear(&self) {
        self.keys.lock().clear();
    }

    /// Number of loaded keys.
    pub fn key_count(&self) -> usize {
        self.keys.lock().len()
    }
}

fn load_private_key(path: &Path, auth: &AuthMethod) -> Result<PrivateKey, VaultError> {
    let key_data = std::fs::read_to_string(path)?;

    let passphrase = match auth {
        AuthMethod::Key { passphrase, .. } => passphrase.as_deref(),
        AuthMethod::KeyAndPassword { passphrase, .. } => passphrase.as_deref(),
        _ => None,
    };

    match passphrase {
        Some(pass) => PrivateKey::from_openssh(key_data.as_bytes())
            .and_then(|k| k.decrypt(pass))
            .map_err(|e| VaultError::KeyGen(format!("failed to decrypt key: {e}"))),
        None => PrivateKey::from_openssh(key_data.as_bytes())
            .map_err(|e| VaultError::KeyGen(format!("failed to load key: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keygen::{KeyGenOptions, KeyType, generate_key, save_key_to_disk};

    fn make_test_account_with_key(dir: &Path) -> VaultAccount {
        let key_path = dir.join("test_key");
        let options = KeyGenOptions {
            key_type: KeyType::Ed25519,
            comment: "test@host".into(),
            passphrase: None,
        };
        let key = generate_key(&options).unwrap();
        save_key_to_disk(&key_path, &key).unwrap();

        VaultAccount {
            id: Uuid::new_v4(),
            display_name: "Test".into(),
            username: "testuser".into(),
            auth: AuthMethod::Key {
                path: key_path,
                passphrase: None,
            },
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn load_and_retrieve_key() {
        let dir = tempfile::tempdir().unwrap();
        let account = make_test_account_with_key(dir.path());
        let account_id = account.id;

        let agent = SshAgent::new();
        agent.load_keys(&[account]);

        assert_eq!(agent.key_count(), 1);
        assert!(agent.get_key(account_id).is_some());
    }

    #[test]
    fn password_only_account_not_loaded() {
        let account = VaultAccount {
            id: Uuid::new_v4(),
            display_name: "PW Only".into(),
            username: "user".into(),
            auth: AuthMethod::Password("pass".into()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let agent = SshAgent::new();
        agent.load_keys(&[account]);
        assert_eq!(agent.key_count(), 0);
    }

    #[test]
    fn clear_removes_all_keys() {
        let dir = tempfile::tempdir().unwrap();
        let account = make_test_account_with_key(dir.path());

        let agent = SshAgent::new();
        agent.load_keys(&[account]);
        assert_eq!(agent.key_count(), 1);

        agent.clear();
        assert_eq!(agent.key_count(), 0);
    }

    #[test]
    fn get_nonexistent_key_returns_none() {
        let agent = SshAgent::new();
        assert!(agent.get_key(Uuid::new_v4()).is_none());
    }
}
