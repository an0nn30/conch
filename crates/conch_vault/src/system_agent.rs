//! System ssh-agent bridge. Adds/removes keys from the running ssh-agent
//! via the SSH_AUTH_SOCK Unix socket. macOS and Linux only (Windows deferred).

use crate::error::VaultError;
use std::collections::HashSet;
use uuid::Uuid;

/// Tracks which keys have been pushed to the system agent.
pub struct SystemAgentBridge {
    /// Account IDs whose keys are currently in the system agent.
    pushed_ids: parking_lot::Mutex<HashSet<Uuid>>,
}

impl SystemAgentBridge {
    pub fn new() -> Self {
        Self {
            pushed_ids: parking_lot::Mutex::new(HashSet::new()),
        }
    }

    /// Check if the system ssh-agent is available.
    pub fn is_available() -> bool {
        std::env::var("SSH_AUTH_SOCK").is_ok()
    }

    /// Add a key to the system agent using ssh-add.
    /// This is a simple implementation using the ssh-add command.
    pub fn add_key(&self, account_id: Uuid, key_path: &std::path::Path) -> Result<(), VaultError> {
        if !Self::is_available() {
            return Err(VaultError::Keychain("SSH_AUTH_SOCK not set".into()));
        }

        let output = std::process::Command::new("ssh-add")
            .arg(key_path)
            .output()
            .map_err(|e| VaultError::Keychain(format!("failed to run ssh-add: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VaultError::Keychain(format!("ssh-add failed: {stderr}")));
        }

        self.pushed_ids.lock().insert(account_id);
        log::info!("system agent: added key for account {account_id}");
        Ok(())
    }

    /// Remove a key from the system agent using ssh-add -d.
    pub fn remove_key(&self, account_id: Uuid, key_path: &std::path::Path) -> Result<(), VaultError> {
        if !Self::is_available() {
            return Ok(()); // Nothing to remove if agent isn't running
        }

        let pub_path = key_path.with_extension("pub");
        let path_to_remove = if pub_path.exists() { &pub_path } else { key_path };

        let output = std::process::Command::new("ssh-add")
            .arg("-d")
            .arg(path_to_remove)
            .output()
            .map_err(|e| VaultError::Keychain(format!("failed to run ssh-add -d: {e}")))?;

        if !output.status.success() {
            log::warn!(
                "system agent: failed to remove key for {}: {}",
                account_id,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        self.pushed_ids.lock().remove(&account_id);
        Ok(())
    }

    /// Remove all pushed keys from the system agent.
    pub fn clear_all(&self, key_paths: &[(Uuid, std::path::PathBuf)]) {
        for (id, path) in key_paths {
            let _ = self.remove_key(*id, path);
        }
        self.pushed_ids.lock().clear();
    }

    /// Returns the set of account IDs currently pushed to the system agent.
    pub fn pushed_ids(&self) -> HashSet<Uuid> {
        self.pushed_ids.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_starts_empty() {
        let bridge = SystemAgentBridge::new();
        assert!(bridge.pushed_ids().is_empty());
    }

    #[test]
    fn is_available_checks_env() {
        // Just verify it doesn't panic; actual value depends on test environment
        let _ = SystemAgentBridge::is_available();
    }
}
