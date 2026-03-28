use crate::error::VaultError;
use crate::model::{Vault, VaultAccount, VaultSettings};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use argon2::Argon2;
use rand::RngCore;
use std::path::Path;
use zeroize::{Zeroize, ZeroizeOnDrop};

const MAGIC: &[u8; 8] = b"CONCHVLT";
const FORMAT_VERSION: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

const ARGON2_M_COST: u32 = 65536;
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 4;

pub fn derive_key(password: &[u8], salt: &[u8]) -> Result<[u8; KEY_LEN], VaultError> {
    let params = argon2::Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(KEY_LEN))
        .map_err(|e| VaultError::Encryption(e.to_string()))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(password, salt, &mut key)
        .map_err(|e| VaultError::Encryption(e.to_string()))?;
    Ok(key)
}

pub fn encrypt_vault(vault: &Vault, password: &[u8]) -> Result<Vec<u8>, VaultError> {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let key = derive_key(password, &salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| VaultError::Encryption(e.to_string()))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let payload =
        bincode::serialize(vault).map_err(|e| VaultError::Serialization(e.to_string()))?;
    let ciphertext = cipher
        .encrypt(nonce, payload.as_ref())
        .map_err(|e| VaultError::Encryption(e.to_string()))?;
    let mut output = Vec::new();
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    output.extend_from_slice(&salt);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

pub fn decrypt_vault(data: &[u8], password: &[u8]) -> Result<Vault, VaultError> {
    let header_len = MAGIC.len() + 4 + SALT_LEN + NONCE_LEN;
    if data.len() < header_len {
        return Err(VaultError::Corrupted("file too short".into()));
    }
    if &data[..8] != MAGIC {
        return Err(VaultError::Corrupted("invalid magic bytes".into()));
    }
    let version = u32::from_le_bytes(
        data[8..12]
            .try_into()
            .map_err(|_| VaultError::Corrupted("invalid version header".into()))?,
    );
    if version != FORMAT_VERSION {
        return Err(VaultError::Corrupted(format!(
            "unsupported version: {version}"
        )));
    }
    let salt = &data[12..12 + SALT_LEN];
    let nonce_bytes = &data[12 + SALT_LEN..12 + SALT_LEN + NONCE_LEN];
    let ciphertext = &data[header_len..];
    let key = derive_key(password, salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| VaultError::Encryption(e.to_string()))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| VaultError::WrongPassword)?;
    deserialize_vault(&plaintext)
}

pub fn generate_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    salt
}

pub fn save_vault_file(path: &Path, vault: &Vault, password: &[u8]) -> Result<(), VaultError> {
    let data = encrypt_vault(vault, password)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("enc.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct CachedKey {
    pub derived_key: [u8; KEY_LEN],
    pub salt: [u8; SALT_LEN],
}

pub fn save_vault_file_with_key(
    path: &Path,
    vault: &Vault,
    cached: &CachedKey,
) -> Result<(), VaultError> {
    let cipher = Aes256Gcm::new_from_slice(&cached.derived_key)
        .map_err(|e| VaultError::Encryption(e.to_string()))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let payload =
        bincode::serialize(vault).map_err(|e| VaultError::Serialization(e.to_string()))?;
    let ciphertext = cipher
        .encrypt(nonce, payload.as_ref())
        .map_err(|e| VaultError::Encryption(e.to_string()))?;
    let mut output = Vec::new();
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    output.extend_from_slice(&cached.salt);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("enc.tmp");
    std::fs::write(&tmp, output)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load_vault_file(path: &Path, password: &[u8]) -> Result<(Vault, CachedKey), VaultError> {
    if !path.exists() {
        return Err(VaultError::NotFound);
    }
    let data = std::fs::read(path)?;
    let header_len = MAGIC.len() + 4 + SALT_LEN + NONCE_LEN;
    if data.len() < header_len {
        return Err(VaultError::Corrupted("file too short".into()));
    }
    if &data[..8] != MAGIC {
        return Err(VaultError::Corrupted("invalid magic bytes".into()));
    }
    let version = u32::from_le_bytes(
        data[8..12]
            .try_into()
            .map_err(|_| VaultError::Corrupted("invalid version header".into()))?,
    );
    if version != FORMAT_VERSION {
        return Err(VaultError::Corrupted(format!(
            "unsupported version: {version}"
        )));
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&data[12..12 + SALT_LEN]);
    let derived_key = derive_key(password, &salt)?;
    let nonce_bytes = &data[12 + SALT_LEN..12 + SALT_LEN + NONCE_LEN];
    let ciphertext = &data[header_len..];
    let cipher = Aes256Gcm::new_from_slice(&derived_key)
        .map_err(|e| VaultError::Encryption(e.to_string()))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| VaultError::WrongPassword)?;
    let vault = deserialize_vault(&plaintext)?;
    Ok((vault, CachedKey { derived_key, salt }))
}

/// Legacy vault format (before `generated_keys` was added).
/// Used for backward-compatible deserialization of existing vault files.
#[derive(serde::Serialize, serde::Deserialize)]
struct LegacyVault {
    version: u32,
    accounts: Vec<VaultAccount>,
    settings: VaultSettings,
}

impl From<LegacyVault> for Vault {
    fn from(v: LegacyVault) -> Self {
        Self {
            version: v.version,
            accounts: v.accounts,
            generated_keys: Vec::new(),
            settings: v.settings,
        }
    }
}

/// Legacy VaultSettings that included `os_keychain_enabled`.
/// Used for backward-compatible deserialization of vault files written before
/// keychain support was removed.
#[derive(serde::Serialize, serde::Deserialize)]
struct LegacyVaultSettingsWithKeychain {
    auto_lock_minutes: u16,
    push_to_system_agent: bool,
    os_keychain_enabled: bool,
    auto_save_passwords: crate::model::AutoSave,
}

impl From<LegacyVaultSettingsWithKeychain> for VaultSettings {
    fn from(s: LegacyVaultSettingsWithKeychain) -> Self {
        Self {
            auto_lock_minutes: s.auto_lock_minutes,
            push_to_system_agent: s.push_to_system_agent,
            auto_save_passwords: s.auto_save_passwords,
        }
    }
}

/// Vault format with `generated_keys` but the old `VaultSettings` (including
/// `os_keychain_enabled`). Used for deserialization of vault files written
/// before keychain support was removed.
#[derive(serde::Serialize, serde::Deserialize)]
struct LegacyVaultWithKeychain {
    version: u32,
    accounts: Vec<VaultAccount>,
    #[serde(default)]
    generated_keys: Vec<crate::model::GeneratedKeyEntry>,
    settings: LegacyVaultSettingsWithKeychain,
}

impl From<LegacyVaultWithKeychain> for Vault {
    fn from(v: LegacyVaultWithKeychain) -> Self {
        Self {
            version: v.version,
            accounts: v.accounts,
            generated_keys: v.generated_keys,
            settings: v.settings.into(),
        }
    }
}

/// Vault format without `generated_keys` but with the old `VaultSettings`
/// (including `os_keychain_enabled`).
#[derive(serde::Serialize, serde::Deserialize)]
struct LegacyVaultNoKeysWithKeychain {
    version: u32,
    accounts: Vec<VaultAccount>,
    settings: LegacyVaultSettingsWithKeychain,
}

impl From<LegacyVaultNoKeysWithKeychain> for Vault {
    fn from(v: LegacyVaultNoKeysWithKeychain) -> Self {
        Self {
            version: v.version,
            accounts: v.accounts,
            generated_keys: Vec::new(),
            settings: v.settings.into(),
        }
    }
}

/// Deserialize vault plaintext, trying the current format first, then falling
/// back to legacy formats for backward compatibility.
fn deserialize_vault(plaintext: &[u8]) -> Result<Vault, VaultError> {
    match bincode::deserialize::<Vault>(plaintext) {
        Ok(v) => return Ok(v),
        Err(e) => log::debug!("Current format failed, trying legacy: {e}"),
    }
    bincode::deserialize::<LegacyVaultWithKeychain>(plaintext)
        .map(Vault::from)
        .or_else(|_| bincode::deserialize::<LegacyVault>(plaintext).map(Vault::from))
        .or_else(|_| {
            bincode::deserialize::<LegacyVaultNoKeysWithKeychain>(plaintext).map(Vault::from)
        })
        .map_err(|e| VaultError::Serialization(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthMethod, VaultAccount, VaultSettings};

    fn make_test_vault() -> Vault {
        Vault {
            version: 1,
            accounts: vec![VaultAccount {
                id: uuid::Uuid::new_v4(),
                display_name: "Test".into(),
                username: "testuser".into(),
                auth: AuthMethod::Password("secret123".into()),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }],
            generated_keys: Vec::new(),
            settings: VaultSettings::default(),
        }
    }

    #[test]
    fn derive_key_deterministic_for_same_inputs() {
        let password = b"master-password";
        let salt = b"1234567890123456";
        let key1 = derive_key(password, salt).unwrap();
        let key2 = derive_key(password, salt).unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn derive_key_different_for_different_passwords() {
        let salt = b"1234567890123456";
        let key1 = derive_key(b"password1", salt).unwrap();
        let key2 = derive_key(b"password2", salt).unwrap();
        assert_ne!(key1, key2);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let vault = make_test_vault();
        let password = b"test-master-password";
        let encrypted = encrypt_vault(&vault, password).unwrap();
        let decrypted = decrypt_vault(&encrypted, password).unwrap();
        assert_eq!(decrypted.version, vault.version);
        assert_eq!(decrypted.accounts.len(), 1);
        assert_eq!(decrypted.accounts[0].username, "testuser");
    }

    #[test]
    fn decrypt_with_wrong_password_fails() {
        let vault = make_test_vault();
        let encrypted = encrypt_vault(&vault, b"correct").unwrap();
        let result = decrypt_vault(&encrypted, b"wrong");
        assert!(matches!(result, Err(VaultError::WrongPassword)));
    }

    #[test]
    fn decrypt_truncated_data_fails() {
        let result = decrypt_vault(b"too short", b"password");
        assert!(matches!(result, Err(VaultError::Corrupted(_))));
    }

    #[test]
    fn decrypt_bad_magic_fails() {
        let mut data = vec![0u8; 100];
        data[..8].copy_from_slice(b"BADMAGIC");
        let result = decrypt_vault(&data, b"password");
        assert!(matches!(result, Err(VaultError::Corrupted(_))));
    }

    #[test]
    fn decrypt_legacy_vault_without_generated_keys() {
        // Simulate a vault file written before the generated_keys field existed.
        let legacy = LegacyVault {
            version: 1,
            accounts: vec![VaultAccount {
                id: uuid::Uuid::new_v4(),
                display_name: "Legacy".into(),
                username: "olduser".into(),
                auth: AuthMethod::Password("pw".into()),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }],
            settings: VaultSettings::default(),
        };
        let password = b"legacy-test";
        // Encrypt the legacy format directly via bincode → AES-GCM.
        let payload = bincode::serialize(&legacy).unwrap();
        let salt = generate_salt();
        let key = derive_key(password, &salt).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher.encrypt(nonce, payload.as_ref()).unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        data.extend_from_slice(&salt);
        data.extend_from_slice(&nonce_bytes);
        data.extend_from_slice(&ciphertext);

        let vault = decrypt_vault(&data, password).unwrap();
        assert_eq!(vault.accounts.len(), 1);
        assert_eq!(vault.accounts[0].username, "olduser");
        assert!(vault.generated_keys.is_empty());
    }

    #[test]
    fn save_and_load_vault_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.enc");
        let vault = make_test_vault();
        let password = b"file-test-password";
        save_vault_file(&path, &vault, password).unwrap();
        assert!(path.exists());
        let (loaded, _cached) = load_vault_file(&path, password).unwrap();
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.accounts[0].username, "testuser");
    }

    #[test]
    fn load_vault_file_roundtrip_uses_deserialize_fallback() {
        // Verify load_vault_file can round-trip: save a current-format vault,
        // then load it back. This exercises the deserialize_vault() path
        // inside load_vault_file (which previously used raw bincode::deserialize
        // and would fail on legacy formats).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roundtrip.enc");
        let vault = make_test_vault();
        let password = b"roundtrip-password";

        save_vault_file(&path, &vault, password).unwrap();
        let (loaded, cached) = load_vault_file(&path, password).unwrap();

        assert_eq!(loaded.version, vault.version);
        assert_eq!(loaded.accounts.len(), vault.accounts.len());
        assert_eq!(loaded.accounts[0].username, "testuser");
        assert_eq!(loaded.accounts[0].display_name, "Test");
        assert_eq!(loaded.generated_keys.len(), vault.generated_keys.len());

        // Also verify the CachedKey can be reused for a subsequent save+load
        let mut vault2 = loaded.clone();
        vault2.accounts[0].username = "updated_user".into();
        save_vault_file_with_key(&path, &vault2, &cached).unwrap();
        let (reloaded, _) = load_vault_file(&path, password).unwrap();
        assert_eq!(reloaded.accounts[0].username, "updated_user");
    }

    #[test]
    fn load_nonexistent_vault_file_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.enc");
        let result = load_vault_file(&path, b"password");
        assert!(matches!(result, Err(VaultError::NotFound)));
    }

    #[test]
    fn cached_key_is_zeroized_on_drop() {
        let salt = [0xABu8; SALT_LEN];
        let derived_key = derive_key(b"password", &salt).unwrap();

        // Verify the key has non-zero content before drop
        assert_ne!(derived_key, [0u8; KEY_LEN]);

        let mut cached = CachedKey { derived_key, salt };
        // Manually zeroize and verify fields are zeroed
        cached.zeroize();
        assert_eq!(
            cached.derived_key, [0u8; KEY_LEN],
            "derived_key should be zeroed"
        );
        assert_eq!(cached.salt, [0u8; SALT_LEN], "salt should be zeroed");
    }

    #[test]
    fn save_vault_file_is_atomic_no_tmp_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.enc");
        let vault = make_test_vault();
        let password = b"atomic-test";

        save_vault_file(&path, &vault, password).unwrap();

        // The final file should exist
        assert!(path.exists(), "vault file should exist after save");
        // The temp file should not remain
        let tmp = path.with_extension("enc.tmp");
        assert!(!tmp.exists(), "temp file should not remain after save");
    }

    #[test]
    fn save_vault_file_with_key_is_atomic_no_tmp_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.enc");
        let vault = make_test_vault();
        let password = b"atomic-key-test";

        // First create via save_vault_file, then load to get a CachedKey
        save_vault_file(&path, &vault, password).unwrap();
        let (loaded, cached) = load_vault_file(&path, password).unwrap();

        // Re-save using the cached key
        let path2 = dir.path().join("vault2.enc");
        save_vault_file_with_key(&path2, &loaded, &cached).unwrap();

        assert!(
            path2.exists(),
            "vault file should exist after save_with_key"
        );
        let tmp2 = path2.with_extension("enc.tmp");
        assert!(
            !tmp2.exists(),
            "temp file should not remain after save_with_key"
        );

        // Verify the saved file is loadable
        let (reloaded, _) = load_vault_file(&path2, password).unwrap();
        assert_eq!(reloaded.accounts[0].username, "testuser");
    }
}
