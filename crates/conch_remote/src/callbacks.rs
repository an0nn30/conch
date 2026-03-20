//! Platform callback trait and path configuration.
//!
//! Each app crate (desktop, mobile) provides its own implementation.

use std::path::PathBuf;

/// Platform-specific paths injected at initialization.
///
/// Desktop uses `~/.ssh/known_hosts`, `~/.config/conch/remote/`, `~/.ssh/id_*`.
/// iOS uses app sandbox / iCloud container paths.
#[derive(Debug, Clone)]
pub struct RemotePaths {
    /// Path to the known_hosts file (e.g., `~/.ssh/known_hosts`).
    pub known_hosts_file: PathBuf,
    /// Directory for server config persistence (e.g., `~/.config/conch/remote/`).
    pub config_dir: PathBuf,
    /// Default SSH key file paths to try during key auth.
    /// Empty on iOS (keys come from Keychain).
    pub default_key_paths: Vec<PathBuf>,
}

/// Callbacks for platform-specific user interaction during SSH operations.
///
/// Implemented by each app crate to bridge prompts to their UI framework.
#[async_trait::async_trait]
pub trait RemoteCallbacks: Send + Sync {
    /// Prompt the user to accept or reject a host key.
    async fn verify_host_key(&self, message: &str, fingerprint: &str) -> bool;

    /// Prompt the user for a password.
    async fn prompt_password(&self, message: &str) -> Option<String>;

    /// Report file transfer progress.
    fn on_transfer_progress(&self, transfer_id: &str, bytes: u64, total: Option<u64>);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn remote_paths_default_key_paths() {
        let paths = RemotePaths {
            known_hosts_file: PathBuf::from("/tmp/known_hosts"),
            config_dir: PathBuf::from("/tmp/config"),
            default_key_paths: vec![
                PathBuf::from("/tmp/keys/id_ed25519"),
                PathBuf::from("/tmp/keys/id_rsa"),
            ],
        };
        assert_eq!(paths.default_key_paths.len(), 2);
        assert_eq!(paths.known_hosts_file, PathBuf::from("/tmp/known_hosts"));
    }

    #[test]
    fn remote_paths_empty_keys() {
        let paths = RemotePaths {
            known_hosts_file: PathBuf::from("/tmp/known_hosts"),
            config_dir: PathBuf::from("/tmp/config"),
            default_key_paths: vec![],
        };
        assert!(paths.default_key_paths.is_empty());
    }
}
