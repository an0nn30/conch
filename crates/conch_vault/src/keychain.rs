//! OS keychain integration for storing the vault master key.
//! macOS: Keychain (Touch ID). Linux: Secret Service. Windows: deferred.

use crate::error::VaultError;

const SERVICE_NAME: &str = "conch-vault";
const ACCOUNT_NAME: &str = "master-key";

/// Store the derived encryption key in the OS keychain.
pub fn store_master_key(key: &[u8]) -> Result<(), VaultError> {
    let encoded = base64_encode(key);
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| VaultError::Keychain(e.to_string()))?;
    entry
        .set_password(&encoded)
        .map_err(|e| VaultError::Keychain(e.to_string()))?;
    log::info!("keychain: master key stored");
    Ok(())
}

/// Retrieve the derived encryption key from the OS keychain.
pub fn retrieve_master_key() -> Result<Vec<u8>, VaultError> {
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| VaultError::Keychain(e.to_string()))?;
    let encoded = entry
        .get_password()
        .map_err(|e| VaultError::Keychain(e.to_string()))?;
    base64_decode(&encoded)
}

/// Delete the master key from the OS keychain.
pub fn delete_master_key() -> Result<(), VaultError> {
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| VaultError::Keychain(e.to_string()))?;
    entry
        .delete_credential()
        .map_err(|e| VaultError::Keychain(e.to_string()))?;
    log::info!("keychain: master key deleted");
    Ok(())
}

/// Check if a master key is stored in the OS keychain.
pub fn has_master_key() -> bool {
    let entry = match keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME) {
        Ok(e) => e,
        Err(_) => return false,
    };
    entry.get_password().is_ok()
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(data: &str) -> Result<Vec<u8>, VaultError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| VaultError::Keychain(format!("base64 decode error: {e}")))
}

// Note: keychain tests require actual OS keychain access and are not run in CI.
// Manual test: run `cargo test -p conch_vault keychain -- --ignored` on a machine
// with keychain access.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        let data = b"test-key-material-32-bytes-long!";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }
}
