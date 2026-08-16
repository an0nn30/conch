use crate::{BUNDLE_MAGIC, SCHEMA_VERSION, ShareError, bundle::ShareBundle};
use termlab_vault::encryption::{decrypt_blob, encrypt_blob};
use termlab_vault::error::VaultError;
use zeroize::Zeroizing;

/// Serialise and encrypt a bundle. The payload is JSON, not bincode: bundles
/// cross machines and versions, so the wire format stays self-describing.
///
/// The intermediate JSON plaintext (which can hold key material and
/// passphrases from `BundledKey`) is held in a `Zeroizing` buffer so it is
/// wiped on drop rather than left in freed-but-unzeroed heap.
pub fn encode(bundle: &ShareBundle, password: &[u8]) -> Result<Vec<u8>, ShareError> {
    let json: Zeroizing<Vec<u8>> = Zeroizing::new(
        serde_json::to_vec(bundle).map_err(|e| ShareError::Malformed(e.to_string()))?,
    );
    encrypt_blob(BUNDLE_MAGIC, bundle.schema_version, &json, password).map_err(map_vault_err)
}

/// Decrypt and deserialise a bundle.
///
/// The envelope's version (from the unauthenticated header) and the JSON
/// body's `schema_version` are written from the same field at encode time,
/// so they cannot legitimately disagree; both are checked here, plus their
/// equality, because a hand-edited file could make them disagree. The
/// decrypted plaintext is held in a `Zeroizing` buffer for the same reason
/// as in `encode`: it can contain key material and passphrases that must
/// not survive as unzeroed freed heap.
pub fn decode(data: &[u8], password: &[u8]) -> Result<ShareBundle, ShareError> {
    let (version, plaintext) =
        decrypt_blob(BUNDLE_MAGIC, None, data, password).map_err(map_vault_err)?;
    let plaintext = Zeroizing::new(plaintext);
    if version > SCHEMA_VERSION {
        return Err(ShareError::UnsupportedVersion(version));
    }
    let bundle: ShareBundle =
        serde_json::from_slice(&plaintext).map_err(|e| ShareError::Malformed(e.to_string()))?;
    if bundle.schema_version > SCHEMA_VERSION {
        return Err(ShareError::UnsupportedVersion(bundle.schema_version));
    }
    if version != bundle.schema_version {
        return Err(ShareError::Malformed(format!(
            "envelope version ({version}) does not match bundle schema_version ({})",
            bundle.schema_version
        )));
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

    #[test]
    fn envelope_version_disagreeing_with_body_is_rejected() {
        let mut bytes = encode(&sample(), b"pw").unwrap();
        // Header layout: magic(8) | version(4, LE) | salt(16) | nonce(12) | ciphertext.
        // Patch only the envelope's version field, leaving the encrypted JSON
        // body (and its schema_version) untouched, simulating a hand-edited
        // file where the two disagree. AES-GCM here has no AAD, so the header
        // is not authenticated and this tamper does not break decryption.
        bytes[8..12].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            decode(&bytes, b"pw"),
            Err(ShareError::Malformed(_))
        ));
    }
}
