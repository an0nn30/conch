//! OS keychain integration — REMOVED.
//!
//! Keychain/Touch ID functionality has been removed. These stubs remain so that
//! the public API surface stays intact for any callers that haven't been cleaned
//! up yet. Every function returns an error or `false`.

use crate::error::VaultError;

/// Always returns an error — keychain storage has been removed.
pub fn store_master_key(_key: &[u8]) -> Result<(), VaultError> {
    Err(VaultError::Keychain(
        "keychain support has been removed".into(),
    ))
}

/// Always returns an error — keychain retrieval has been removed.
pub fn retrieve_master_key() -> Result<Vec<u8>, VaultError> {
    Err(VaultError::Keychain(
        "keychain support has been removed".into(),
    ))
}

/// Always returns an error — keychain deletion has been removed.
pub fn delete_master_key() -> Result<(), VaultError> {
    Err(VaultError::Keychain(
        "keychain support has been removed".into(),
    ))
}

/// Always returns false — keychain support has been removed.
pub fn has_master_key() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_returns_error() {
        assert!(store_master_key(b"test").is_err());
    }

    #[test]
    fn retrieve_returns_error() {
        assert!(retrieve_master_key().is_err());
    }

    #[test]
    fn delete_returns_error() {
        assert!(delete_master_key().is_err());
    }

    #[test]
    fn has_master_key_returns_false() {
        assert!(!has_master_key());
    }
}
