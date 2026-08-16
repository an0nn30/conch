pub mod bundle;
pub mod codec;

pub const SCHEMA_VERSION: u32 = 1;
pub const BUNDLE_MAGIC: &[u8; 8] = b"TRMLBSHR";
pub const BUNDLE_EXTENSION: &str = "termlabshare";

#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error("Incorrect password")]
    WrongPassword,
    #[error("Not a valid TermLab share bundle")]
    NotABundle,
    #[error("This bundle was created by a newer version of TermLab")]
    UnsupportedVersion(u32),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Malformed(String),
}

pub use bundle::{BundleMetadata, BundledKey, BundledVault, ShareBundle};
