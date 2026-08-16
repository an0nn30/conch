use serde::{Deserialize, Serialize};
use termlab_remote::config::{SavedTunnel, ServerEntry, ServerFolder};
use termlab_vault::model::VaultAccount;
use uuid::Uuid;
use zeroize::Zeroize;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleMetadata {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub source_host: String,
    pub termlab_version: String,
    /// Display only. The authoritative test is whether `vault.accounts` and
    /// `vault.keys` are both empty.
    pub includes_credentials: bool,
}

/// A private key carried inside a bundle. `material` is the raw key file's
/// bytes, base64-encoded by serde as a plain String.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledKey {
    pub id: Uuid,
    pub original_path: String,
    pub material: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_material: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    pub comment: String,
}

impl Zeroize for BundledKey {
    fn zeroize(&mut self) {
        self.material.zeroize();
        if let Some(p) = &mut self.passphrase {
            p.zeroize();
        }
    }
}

impl Drop for BundledKey {
    fn drop(&mut self) {
        self.zeroize();
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundledVault {
    #[serde(default)]
    pub accounts: Vec<VaultAccount>,
    #[serde(default)]
    pub keys: Vec<BundledKey>,
}

impl BundledVault {
    /// The authoritative credentials test named in the spec.
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.keys.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareBundle {
    pub schema_version: u32,
    pub metadata: BundleMetadata,
    #[serde(default)]
    pub folders: Vec<ServerFolder>,
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
    #[serde(default)]
    pub tunnels: Vec<SavedTunnel>,
    #[serde(default)]
    pub vault: BundledVault,
}
