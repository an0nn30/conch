use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;
use zeroize::Zeroize;

pub const VAULT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vault {
    pub version: u32,
    pub accounts: Vec<VaultAccount>,
    #[serde(default)]
    pub generated_keys: Vec<GeneratedKeyEntry>,
    pub settings: VaultSettings,
}

impl Default for Vault {
    fn default() -> Self {
        Self {
            version: VAULT_VERSION,
            accounts: Vec::new(),
            generated_keys: Vec::new(),
            settings: VaultSettings::default(),
        }
    }
}

/// Metadata for an SSH key generated through the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedKeyEntry {
    pub id: Uuid,
    pub algorithm: String,
    pub fingerprint: String,
    pub comment: String,
    pub private_path: PathBuf,
    pub public_path: PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultAccount {
    pub id: Uuid,
    pub display_name: String,
    pub username: String,
    pub auth: AuthMethod,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthMethod {
    Password(String),
    Key {
        path: PathBuf,
        passphrase: Option<String>,
    },
    KeyAndPassword {
        key_path: PathBuf,
        passphrase: Option<String>,
        password: String,
    },
}

impl Zeroize for AuthMethod {
    fn zeroize(&mut self) {
        match self {
            AuthMethod::Password(p) => p.zeroize(),
            AuthMethod::Key { passphrase, .. } => {
                if let Some(p) = passphrase {
                    p.zeroize();
                }
            }
            AuthMethod::KeyAndPassword {
                passphrase,
                password,
                ..
            } => {
                if let Some(p) = passphrase {
                    p.zeroize();
                }
                password.zeroize();
            }
        }
    }
}

impl Drop for AuthMethod {
    fn drop(&mut self) {
        self.zeroize();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultSettings {
    pub auto_lock_minutes: u16,
    pub push_to_system_agent: bool,
    pub auto_save_passwords: AutoSave,
}

impl Default for VaultSettings {
    fn default() -> Self {
        Self {
            auto_lock_minutes: 15,
            push_to_system_agent: false,
            auto_save_passwords: AutoSave::Ask,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoSave {
    Always,
    Ask,
    Never,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_default_has_version_1() {
        let vault = Vault::default();
        assert_eq!(vault.version, VAULT_VERSION);
        assert!(vault.accounts.is_empty());
    }

    #[test]
    fn vault_settings_defaults() {
        let settings = VaultSettings::default();
        assert_eq!(settings.auto_lock_minutes, 15);
        assert!(!settings.push_to_system_agent);
        assert_eq!(settings.auto_save_passwords, AutoSave::Ask);
    }

    #[test]
    fn vault_settings_backward_compat_ignores_unknown_fields() {
        // Old vault files may contain os_keychain_enabled — serde(default) handles it.
        let json = r#"{"auto_lock_minutes":10,"push_to_system_agent":true,"os_keychain_enabled":true,"auto_save_passwords":"Always"}"#;
        let settings: VaultSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.auto_lock_minutes, 10);
        assert!(settings.push_to_system_agent);
        assert_eq!(settings.auto_save_passwords, AutoSave::Always);
    }

    #[test]
    fn vault_account_serde_roundtrip() {
        let account = VaultAccount {
            id: Uuid::new_v4(),
            display_name: "Test Account".into(),
            username: "testuser".into(),
            auth: AuthMethod::Password("secret".into()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&account).unwrap();
        let deserialized: VaultAccount = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, account.id);
        assert_eq!(deserialized.username, "testuser");
    }

    #[test]
    fn auth_method_key_serde_roundtrip() {
        let auth = AuthMethod::Key {
            path: std::path::PathBuf::from("/home/user/.ssh/id_ed25519"),
            passphrase: Some("mypass".into()),
        };
        let json = serde_json::to_string(&auth).unwrap();
        let deserialized: AuthMethod = serde_json::from_str(&json).unwrap();
        match deserialized {
            AuthMethod::Key {
                ref path,
                ref passphrase,
            } => {
                assert_eq!(path.to_str().unwrap(), "/home/user/.ssh/id_ed25519");
                assert_eq!(passphrase.as_deref().unwrap(), "mypass");
            }
            _ => panic!("expected AuthMethod::Key"),
        }
    }

    #[test]
    fn auth_method_key_and_password_serde_roundtrip() {
        let auth = AuthMethod::KeyAndPassword {
            key_path: std::path::PathBuf::from("/home/user/.ssh/id_rsa"),
            passphrase: None,
            password: "serverpass".into(),
        };
        let json = serde_json::to_string(&auth).unwrap();
        let deserialized: AuthMethod = serde_json::from_str(&json).unwrap();
        match deserialized {
            AuthMethod::KeyAndPassword {
                ref key_path,
                ref passphrase,
                ref password,
            } => {
                assert_eq!(key_path.to_str().unwrap(), "/home/user/.ssh/id_rsa");
                assert!(passphrase.is_none());
                assert_eq!(password, "serverpass");
            }
            _ => panic!("expected AuthMethod::KeyAndPassword"),
        }
    }
}
