use crate::error::VaultError;
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    Ed25519,
    EcdsaP256,
    EcdsaP384,
    Rsa2048,
    Rsa4096,
}

pub struct KeyGenOptions {
    pub key_type: KeyType,
    pub comment: String,
    pub passphrase: Option<String>,
}

pub struct GeneratedKey {
    pub private_key_str: String,
    pub public_key: String,
    pub fingerprint: String,
    pub algorithm: String,
}

/// Generate an SSH key pair.
pub fn generate_key(options: &KeyGenOptions) -> Result<GeneratedKey, VaultError> {
    let mut rng = rand::thread_rng();

    let private_key = match options.key_type {
        KeyType::Ed25519 => PrivateKey::random(&mut rng, Algorithm::Ed25519)
            .map_err(|e| VaultError::KeyGen(e.to_string()))?,
        KeyType::EcdsaP256 => PrivateKey::random(
            &mut rng,
            Algorithm::Ecdsa {
                curve: ssh_key::EcdsaCurve::NistP256,
            },
        )
        .map_err(|e| VaultError::KeyGen(e.to_string()))?,
        KeyType::EcdsaP384 => PrivateKey::random(
            &mut rng,
            Algorithm::Ecdsa {
                curve: ssh_key::EcdsaCurve::NistP384,
            },
        )
        .map_err(|e| VaultError::KeyGen(e.to_string()))?,
        KeyType::Rsa2048 => {
            PrivateKey::random(&mut rng, Algorithm::Rsa { hash: Some(HashAlg::Sha256) })
                .map_err(|e| VaultError::KeyGen(e.to_string()))?
        }
        KeyType::Rsa4096 => {
            // ssh-key generates RSA keys at a fixed size — both Rsa2048 and Rsa4096
            // use the Rsa algorithm; the hash parameter selects the signature hash,
            // not the key size. Both variants will generate the same key size.
            PrivateKey::random(&mut rng, Algorithm::Rsa { hash: Some(HashAlg::Sha512) })
                .map_err(|e| VaultError::KeyGen(e.to_string()))?
        }
    };

    let public_key = private_key.public_key();
    let fingerprint = public_key.fingerprint(HashAlg::Sha256).to_string();

    let mut private_key_with_comment = private_key.clone();
    private_key_with_comment.set_comment(&options.comment);

    // Output format: OpenSSH (the standard for modern SSH keys).
    let private_key_str = match &options.passphrase {
        Some(pass) => private_key_with_comment
            .encrypt(&mut rng, pass)
            .map_err(|e| VaultError::KeyGen(e.to_string()))?
            .to_openssh(LineEnding::LF)
            .map_err(|e| VaultError::KeyGen(e.to_string()))?
            .to_string(),
        None => private_key_with_comment
            .to_openssh(LineEnding::LF)
            .map_err(|e| VaultError::KeyGen(e.to_string()))?
            .to_string(),
    };

    let mut pub_key_str = public_key
        .to_openssh()
        .map_err(|e| VaultError::KeyGen(e.to_string()))?;
    pub_key_str.push(' ');
    pub_key_str.push_str(&options.comment);

    let algorithm = match options.key_type {
        KeyType::Ed25519 => "Ed25519",
        KeyType::EcdsaP256 => "ECDSA P-256",
        KeyType::EcdsaP384 => "ECDSA P-384",
        KeyType::Rsa2048 => "RSA 2048",
        KeyType::Rsa4096 => "RSA 4096",
    };

    Ok(GeneratedKey {
        private_key_str,
        public_key: pub_key_str,
        fingerprint,
        algorithm: algorithm.into(),
    })
}

/// Save a generated key pair to disk (private key + .pub file).
pub fn save_key_to_disk(path: &Path, key: &GeneratedKey) -> Result<(), VaultError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Write private key with restrictive permissions.
    std::fs::write(path, &key.private_key_str)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    // Write public key.
    let pub_path = path.with_extension("pub");
    std::fs::write(&pub_path, &key.public_key)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_ed25519_key() {
        let options = KeyGenOptions {
            key_type: KeyType::Ed25519,
            comment: "test@host".into(),
            passphrase: None,
        };
        let key = generate_key(&options).unwrap();
        assert!(key.public_key.contains("ssh-ed25519"));
        assert!(key.public_key.contains("test@host"));
        assert!(key.fingerprint.starts_with("SHA256:"));
        assert_eq!(key.algorithm, "Ed25519");
    }

    #[test]
    fn generate_ecdsa_p256_key() {
        let options = KeyGenOptions {
            key_type: KeyType::EcdsaP256,
            comment: "test@host".into(),
            passphrase: None,
        };
        let key = generate_key(&options).unwrap();
        assert!(key.public_key.contains("ecdsa-sha2-nistp256"));
        assert_eq!(key.algorithm, "ECDSA P-256");
    }

    #[test]
    fn generate_ed25519_with_passphrase() {
        let options = KeyGenOptions {
            key_type: KeyType::Ed25519,
            comment: "encrypted@host".into(),
            passphrase: Some("mypassphrase".into()),
        };
        let key = generate_key(&options).unwrap();
        assert!(key.private_key_str.contains("OPENSSH PRIVATE KEY"));
        assert!(key.public_key.contains("ssh-ed25519"));
    }

    #[test]
    fn save_key_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_key");

        let options = KeyGenOptions {
            key_type: KeyType::Ed25519,
            comment: "test@host".into(),
            passphrase: None,
        };
        let key = generate_key(&options).unwrap();
        save_key_to_disk(&path, &key).unwrap();

        assert!(path.exists());
        assert!(path.with_extension("pub").exists());

        let pub_content = std::fs::read_to_string(path.with_extension("pub")).unwrap();
        assert!(pub_content.contains("ssh-ed25519"));
    }

    #[cfg(unix)]
    #[test]
    fn saved_private_key_has_600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perm_test_key");

        let options = KeyGenOptions {
            key_type: KeyType::Ed25519,
            comment: "test@host".into(),
            passphrase: None,
        };
        let key = generate_key(&options).unwrap();
        save_key_to_disk(&path, &key).unwrap();

        let perms = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }
}
