use crate::{BUNDLE_MAGIC, SCHEMA_VERSION, ShareError, bundle::ShareBundle};
use termlab_vault::encryption::{decrypt_blob, encrypt_blob};
use termlab_vault::error::VaultError;

/// Serialise and encrypt a bundle. The payload is JSON, not bincode: bundles
/// cross machines and versions, so the wire format stays self-describing.
pub fn encode(bundle: &ShareBundle, password: &[u8]) -> Result<Vec<u8>, ShareError> {
    let json = serde_json::to_vec(bundle).map_err(|e| ShareError::Malformed(e.to_string()))?;
    encrypt_blob(BUNDLE_MAGIC, bundle.schema_version, &json, password).map_err(map_vault_err)
}

pub fn decode(data: &[u8], password: &[u8]) -> Result<ShareBundle, ShareError> {
    let (version, plaintext) =
        decrypt_blob(BUNDLE_MAGIC, None, data, password).map_err(map_vault_err)?;
    if version > SCHEMA_VERSION {
        return Err(ShareError::UnsupportedVersion(version));
    }
    let bundle: ShareBundle =
        serde_json::from_slice(&plaintext).map_err(|e| ShareError::Malformed(e.to_string()))?;
    if bundle.schema_version > SCHEMA_VERSION {
        return Err(ShareError::UnsupportedVersion(bundle.schema_version));
    }
    Ok(bundle)
}

fn map_vault_err(e: VaultError) -> ShareError {
    match e {
        VaultError::WrongPassword => ShareError::WrongPassword,
        VaultError::Corrupted(_) => ShareError::NotABundle,
        other => ShareError::Io(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{BundleMetadata, BundledVault, ShareBundle};

    fn sample() -> ShareBundle {
        ShareBundle {
            schema_version: crate::SCHEMA_VERSION,
            metadata: BundleMetadata {
                created_at: chrono::Utc::now(),
                source_host: "test-host".into(),
                termlab_version: "0.0.0-test".into(),
                includes_credentials: false,
            },
            folders: Vec::new(),
            servers: Vec::new(),
            tunnels: Vec::new(),
            vault: BundledVault::default(),
        }
    }

    #[test]
    fn round_trips() {
        let bytes = encode(&sample(), b"hunter2").unwrap();
        let back = decode(&bytes, b"hunter2").unwrap();
        assert_eq!(back.metadata.source_host, "test-host");
        assert_eq!(back.schema_version, crate::SCHEMA_VERSION);
    }

    #[test]
    fn wrong_password_is_reported_as_such() {
        let bytes = encode(&sample(), b"right").unwrap();
        assert!(matches!(
            decode(&bytes, b"wrong"),
            Err(ShareError::WrongPassword)
        ));
    }

    #[test]
    fn foreign_file_is_not_a_bundle() {
        assert!(matches!(
            decode(b"this is not a bundle at all, not even close", b"pw"),
            Err(ShareError::NotABundle)
        ));
    }

    #[test]
    fn future_schema_version_is_rejected_by_name() {
        let mut b = sample();
        b.schema_version = 999;
        let bytes = encode(&b, b"pw").unwrap();
        assert!(matches!(
            decode(&bytes, b"pw"),
            Err(ShareError::UnsupportedVersion(999))
        ));
    }

    #[test]
    fn envelope_starts_with_the_share_magic() {
        let bytes = encode(&sample(), b"pw").unwrap();
        assert_eq!(&bytes[..8], b"TRMLBSHR");
    }
}
