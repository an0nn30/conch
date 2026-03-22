pub mod agent;
pub mod encryption;
pub mod error;
pub mod keychain;
pub mod keygen;
pub mod lock;
pub mod model;
pub mod system_agent;

pub use error::VaultError;
pub use model::*;

use lock::LockManager;
use parking_lot::Mutex;
use std::path::PathBuf;
use uuid::Uuid;
use zeroize::Zeroize;

/// Central vault manager. Holds the decrypted vault in memory while unlocked.
pub struct VaultManager {
    vault_path: PathBuf,
    lock_manager: LockManager,
    /// Decrypted vault data. None when locked.
    vault: Mutex<Option<Vault>>,
    /// Cached key material (derived key + salt). Cleared on lock.
    cached_key: Mutex<Option<encryption::CachedKey>>,
}

impl VaultManager {
    pub fn new(vault_path: PathBuf) -> Self {
        Self {
            vault_path,
            lock_manager: LockManager::new(15),
            vault: Mutex::new(None),
            cached_key: Mutex::new(None),
        }
    }

    /// Returns true if the vault file exists on disk.
    pub fn vault_exists(&self) -> bool {
        self.vault_path.exists()
    }

    pub fn is_locked(&self) -> bool {
        self.lock_manager.is_locked()
    }

    pub fn seconds_remaining(&self) -> u64 {
        self.lock_manager.seconds_remaining()
    }

    /// Check inactivity timeout. Returns true if vault was auto-locked.
    pub fn check_timeout(&self) -> bool {
        let did_lock = self.lock_manager.check_timeout();
        if did_lock {
            self.clear_memory();
        }
        did_lock
    }

    /// Create a new vault with the given master password. Saves to disk.
    pub fn create(&self, password: &[u8]) -> Result<(), VaultError> {
        if self.vault_exists() {
            return Err(VaultError::Corrupted("vault already exists".into()));
        }
        let vault = Vault::default();
        // Derive and cache the encryption key so save() doesn't need the password
        let salt = encryption::generate_salt();
        let derived_key = encryption::derive_key(password, &salt)?;
        let cached = encryption::CachedKey { derived_key, salt };
        encryption::save_vault_file_with_key(&self.vault_path, &vault, &cached)?;
        *self.vault.lock() = Some(vault);
        *self.cached_key.lock() = Some(cached);
        self.lock_manager.unlock();
        Ok(())
    }

    /// Unlock an existing vault with the master password.
    pub fn unlock(&self, password: &[u8]) -> Result<(), VaultError> {
        let (vault, cached) = encryption::load_vault_file(&self.vault_path, password)?;
        let timeout = vault.settings.auto_lock_minutes;
        *self.vault.lock() = Some(vault);
        *self.cached_key.lock() = Some(cached);
        self.lock_manager.set_timeout_minutes(timeout);
        self.lock_manager.unlock();
        Ok(())
    }

    /// Lock the vault and clear decrypted data from memory.
    pub fn lock(&self) {
        self.lock_manager.lock();
        self.clear_memory();
    }

    /// Save current vault state to disk using the cached derived key.
    /// Vault must be unlocked.
    pub fn save(&self) -> Result<(), VaultError> {
        let vault_guard = self.vault.lock();
        let vault = vault_guard.as_ref().ok_or(VaultError::Locked)?;
        let key_guard = self.cached_key.lock();
        let cached = key_guard.as_ref().ok_or(VaultError::Locked)?;
        encryption::save_vault_file_with_key(&self.vault_path, vault, cached)?;
        self.lock_manager.touch();
        Ok(())
    }

    // --- Account CRUD ---

    /// List all accounts. Vault must be unlocked.
    pub fn list_accounts(&self) -> Result<Vec<VaultAccount>, VaultError> {
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(VaultError::Locked)?;
        self.lock_manager.touch();
        Ok(vault.accounts.clone())
    }

    /// Get a single account by ID.
    pub fn get_account(&self, id: Uuid) -> Result<VaultAccount, VaultError> {
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(VaultError::Locked)?;
        self.lock_manager.touch();
        vault
            .accounts
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or(VaultError::AccountNotFound(id))
    }

    /// Add a new account. Returns the assigned UUID.
    pub fn add_account(
        &self,
        display_name: String,
        username: String,
        auth: AuthMethod,
    ) -> Result<Uuid, VaultError> {
        let mut guard = self.vault.lock();
        let vault = guard.as_mut().ok_or(VaultError::Locked)?;
        let now = chrono::Utc::now();
        let id = Uuid::new_v4();
        vault.accounts.push(VaultAccount {
            id,
            display_name,
            username,
            auth,
            created_at: now,
            updated_at: now,
        });
        self.lock_manager.touch();
        Ok(id)
    }

    /// Update an existing account's fields.
    pub fn update_account(
        &self,
        id: Uuid,
        display_name: Option<String>,
        username: Option<String>,
        auth: Option<AuthMethod>,
    ) -> Result<(), VaultError> {
        let mut guard = self.vault.lock();
        let vault = guard.as_mut().ok_or(VaultError::Locked)?;
        let account = vault
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(VaultError::AccountNotFound(id))?;
        if let Some(name) = display_name {
            account.display_name = name;
        }
        if let Some(user) = username {
            account.username = user;
        }
        if let Some(a) = auth {
            account.auth = a;
        }
        account.updated_at = chrono::Utc::now();
        self.lock_manager.touch();
        Ok(())
    }

    /// Delete an account by ID. Returns true if found and removed.
    pub fn delete_account(&self, id: Uuid) -> Result<bool, VaultError> {
        let mut guard = self.vault.lock();
        let vault = guard.as_mut().ok_or(VaultError::Locked)?;
        let len_before = vault.accounts.len();
        vault.accounts.retain(|a| a.id != id);
        self.lock_manager.touch();
        Ok(vault.accounts.len() < len_before)
    }

    /// Get vault settings.
    pub fn get_settings(&self) -> Result<VaultSettings, VaultError> {
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(VaultError::Locked)?;
        self.lock_manager.touch();
        Ok(vault.settings.clone())
    }

    /// Update vault settings.
    pub fn update_settings(&self, settings: VaultSettings) -> Result<(), VaultError> {
        let mut guard = self.vault.lock();
        let vault = guard.as_mut().ok_or(VaultError::Locked)?;
        self.lock_manager.set_timeout_minutes(settings.auto_lock_minutes);
        vault.settings = settings;
        self.lock_manager.touch();
        Ok(())
    }

    /// Find accounts matching a username.
    pub fn find_accounts_by_username(&self, username: &str) -> Result<Vec<VaultAccount>, VaultError> {
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(VaultError::Locked)?;
        self.lock_manager.touch();
        Ok(vault
            .accounts
            .iter()
            .filter(|a| a.username == username)
            .cloned()
            .collect())
    }

    fn clear_memory(&self) {
        let mut guard = self.vault.lock();
        if let Some(ref mut vault) = *guard {
            for account in &mut vault.accounts {
                account.auth.zeroize();
            }
        }
        *guard = None;
        *self.cached_key.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager() -> (VaultManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.enc");
        (VaultManager::new(path), dir)
    }

    #[test]
    fn create_and_unlock_vault() {
        let (mgr, _dir) = make_manager();
        assert!(!mgr.vault_exists());

        mgr.create(b"master").unwrap();
        assert!(mgr.vault_exists());
        assert!(!mgr.is_locked());

        mgr.lock();
        assert!(mgr.is_locked());

        mgr.unlock(b"master").unwrap();
        assert!(!mgr.is_locked());
    }

    #[test]
    fn unlock_wrong_password_fails() {
        let (mgr, _dir) = make_manager();
        mgr.create(b"correct").unwrap();
        mgr.lock();

        let result = mgr.unlock(b"wrong");
        assert!(matches!(result, Err(VaultError::WrongPassword)));
        assert!(mgr.is_locked());
    }

    #[test]
    fn crud_accounts() {
        let (mgr, _dir) = make_manager();
        mgr.create(b"master").unwrap();

        // Add
        let id = mgr
            .add_account("Deploy".into(), "deploy".into(), AuthMethod::Password("pass".into()))
            .unwrap();

        // List
        let accounts = mgr.list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].username, "deploy");

        // Get
        let account = mgr.get_account(id).unwrap();
        assert_eq!(account.display_name, "Deploy");

        // Update
        mgr.update_account(id, Some("Prod Deploy".into()), None, None)
            .unwrap();
        let updated = mgr.get_account(id).unwrap();
        assert_eq!(updated.display_name, "Prod Deploy");

        // Delete
        assert!(mgr.delete_account(id).unwrap());
        assert!(mgr.list_accounts().unwrap().is_empty());
    }

    #[test]
    fn operations_fail_when_locked() {
        let (mgr, _dir) = make_manager();
        mgr.create(b"master").unwrap();
        mgr.lock();

        assert!(matches!(mgr.list_accounts(), Err(VaultError::Locked)));
        assert!(matches!(
            mgr.add_account("x".into(), "x".into(), AuthMethod::Password("x".into())),
            Err(VaultError::Locked)
        ));
    }

    #[test]
    fn find_accounts_by_username() {
        let (mgr, _dir) = make_manager();
        mgr.create(b"master").unwrap();

        mgr.add_account("A".into(), "root".into(), AuthMethod::Password("p1".into())).unwrap();
        mgr.add_account("B".into(), "deploy".into(), AuthMethod::Password("p2".into())).unwrap();
        mgr.add_account("C".into(), "root".into(), AuthMethod::Password("p3".into())).unwrap();

        let roots = mgr.find_accounts_by_username("root").unwrap();
        assert_eq!(roots.len(), 2);

        let deploys = mgr.find_accounts_by_username("deploy").unwrap();
        assert_eq!(deploys.len(), 1);

        let nones = mgr.find_accounts_by_username("nobody").unwrap();
        assert!(nones.is_empty());
    }

    #[test]
    fn get_nonexistent_account_returns_error() {
        let (mgr, _dir) = make_manager();
        mgr.create(b"master").unwrap();
        let result = mgr.get_account(Uuid::new_v4());
        assert!(matches!(result, Err(VaultError::AccountNotFound(_))));
    }
}
