use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("vault is locked")]
    Locked,
    #[error("vault is already unlocked")]
    AlreadyUnlocked,
    #[error("vault file not found")]
    NotFound,
    #[error("incorrect master password")]
    WrongPassword,
    #[error("vault file corrupted: {0}")]
    Corrupted(String),
    #[error("account not found: {0}")]
    AccountNotFound(uuid::Uuid),
    #[error("encryption error: {0}")]
    Encryption(String),
    #[error("key generation error: {0}")]
    KeyGen(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("keychain error: {0}")]
    Keychain(String),
}
