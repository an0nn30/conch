//! Import execution: apply an [`ImportPlan`] to disk and to the local
//! config/vault. This is the only unit in `termlab_share` allowed to touch
//! the filesystem or mutate user state during import — the planner
//! ([`crate::import_planner`]) stays pure so the `Add`/`Replace` decision is
//! testable without any of this.
//!
//! Apply order is load-bearing: keys, then accounts, then folders, then
//! servers, then tunnels, so that by the time a later stage references an
//! id created by an earlier one (an account's rewritten key path, a
//! tunnel's host), that id already resolves.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use uuid::Uuid;
use zeroize::Zeroizing;

use termlab_remote::config::{SavedTunnel, ServerEntry, ServerFolder, SshConfig};
use termlab_vault::model::{AuthMethod, VaultAccount};

use crate::ShareError;
use crate::bundle::BundledKey;
use crate::import_planner::{ImportPlan, PlannedItem};

/// Where imported vault accounts go. Implemented against the real vault by
/// the caller (Task 6); tests supply a fake so `import_executor`'s own unit
/// tests don't need a real vault open.
pub trait VaultSink {
    fn upsert_account(&mut self, account: VaultAccount) -> Result<(), String>;
}

pub struct ImportOutcome {
    pub servers: usize,
    pub tunnels: usize,
    pub credentials: usize,
    pub skipped: Vec<String>,
    pub credentials_held_back: bool,
}

/// Apply `plan` to `config` (folders, servers, tunnels), to `key_dir`
/// (materialised key files), and to `vault_sink` (accounts), in that
/// dependency order — see the module doc for why the order matters.
pub fn execute(
    plan: ImportPlan,
    config: &mut SshConfig,
    key_dir: &Path,
    vault_sink: Option<&mut dyn VaultSink>,
) -> Result<ImportOutcome, ShareError> {
    let ImportPlan {
        folders,
        servers,
        tunnels,
        accounts,
        keys,
        skipped,
    } = plan;

    // Counts reported to the caller: total server entries touched,
    // including those nested inside a planned folder (folders themselves
    // have no counter of their own in `ImportOutcome`).
    let servers_count = servers.len() + folders.iter().map(|f| f.item.entries.len()).sum::<usize>();
    let tunnels_count = tunnels.len();

    // 1. Keys first, so accounts (step 2) and any later inspection have a
    // real path to rewrite the `termlab-bundle:<key-id>` marker to.
    let key_paths = materialise_keys(&keys, key_dir)?;

    // 2. Accounts. With no vault open on this machine yet (v1: no inline
    // vault creation during import), every account is skipped and the
    // caller is told credentials were held back; hosts and tunnels still
    // import below.
    let (credentials, credentials_held_back) = match vault_sink {
        None => (0, true),
        Some(sink) => {
            let mut imported = 0usize;
            for planned in accounts {
                let mut account = planned.item;
                rewrite_key_marker(&mut account.auth, &key_paths);
                sink.upsert_account(account).map_err(ShareError::Io)?;
                imported += 1;
            }
            (imported, false)
        }
    };

    // 3. Folders, then servers, then tunnels, so a tunnel added last can
    // reference a server or folder entry added just before it.
    for planned in folders {
        apply_folder(config, planned);
    }
    for planned in servers {
        apply_server(config, planned);
    }
    for planned in tunnels {
        apply_tunnel(config, planned);
    }

    Ok(ImportOutcome {
        servers: servers_count,
        tunnels: tunnels_count,
        credentials,
        skipped,
        credentials_held_back,
    })
}

/// Write each bundled key's material to `key_dir/<id>`, creating the file
/// with owner-only permissions before any bytes are written (never write
/// then chmod — a world-readable window, even briefly, defeats the point).
///
/// Key ids are UUIDs, so a collision with a file already on disk means this
/// exact key was materialised by a previous import; it is left alone and
/// reused rather than rewritten.
fn materialise_keys(
    keys: &[BundledKey],
    key_dir: &Path,
) -> Result<HashMap<Uuid, PathBuf>, ShareError> {
    let mut paths = HashMap::with_capacity(keys.len());
    for key in keys {
        let private_path = key_dir.join(key.id.to_string());
        match create_owner_only_file(&private_path).map_err(|e| ShareError::Io(e.to_string()))? {
            Some(mut file) => {
                let decoded: Zeroizing<Vec<u8>> = Zeroizing::new(
                    base64::engine::general_purpose::STANDARD
                        .decode(&key.material)
                        .map_err(|e| ShareError::Malformed(e.to_string()))?,
                );
                file.write_all(&decoded)
                    .map_err(|e| ShareError::Io(e.to_string()))?;

                if let Some(public) = &key.public_material {
                    let decoded_pub = base64::engine::general_purpose::STANDARD
                        .decode(public)
                        .map_err(|e| ShareError::Malformed(e.to_string()))?;
                    fs::write(key_dir.join(format!("{}.pub", key.id)), decoded_pub)
                        .map_err(|e| ShareError::Io(e.to_string()))?;
                }
            }
            None => {
                // Already materialised by a previous import of the same
                // key id — reuse it, do not touch its bytes or its
                // permissions.
            }
        }
        paths.insert(key.id, private_path);
    }
    Ok(paths)
}

/// Create `path` for exclusive writing with owner-only (`0600`) permissions
/// on Unix, set at creation time via `OpenOptions` rather than a later
/// `chmod`. Returns `Ok(None)` if the file already exists (treated as an
/// intentional collision-is-reuse case, not an error).
fn create_owner_only_file(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(path) {
        Ok(file) => Ok(Some(file)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(e) => Err(e),
    }
}

/// Rewrite the export planner's `termlab-bundle:<key-id>` marker (see
/// `export_planner::embed_key_for_account`) to the real, materialised path
/// from `key_paths`. A path that isn't the marker format (e.g. a legacy
/// server's stale absolute path the vault account was synthesised from) is
/// left untouched — see the module-level note on `vault_account_id` being
/// authoritative over a stale `key_path`.
fn rewrite_key_marker(auth: &mut AuthMethod, key_paths: &HashMap<Uuid, PathBuf>) {
    match auth {
        AuthMethod::Key { path, .. } => rewrite_marker_path(path, key_paths),
        AuthMethod::KeyAndPassword { key_path, .. } => rewrite_marker_path(key_path, key_paths),
        AuthMethod::Password(_) => {}
    }
}

fn rewrite_marker_path(path: &mut PathBuf, key_paths: &HashMap<Uuid, PathBuf>) {
    let Some(marker) = path
        .to_str()
        .and_then(|s| s.strip_prefix("termlab-bundle:"))
    else {
        return;
    };
    let Ok(id) = Uuid::parse_str(marker) else {
        return;
    };
    if let Some(real_path) = key_paths.get(&id) {
        *path = real_path.clone();
    }
}

// These three apply functions deliberately re-derive Add-vs-Replace from
// `config`'s actual current contents by id, rather than blindly trusting
// `planned.action`. `plan()` only knows the config as it stood at planning
// time; if anything has changed by the time `execute` runs — including,
// trivially, this same bundle having already been imported once — a stale
// `Add` must not turn into a duplicate. Matching by id at apply time is what
// makes `execute` idempotent on its own, independent of how fresh the plan
// is. `planned.action` remains meaningful as information for the caller
// (e.g. a UI summary of what will change) even though it is unused here.

fn apply_folder(config: &mut SshConfig, planned: PlannedItem<ServerFolder>) {
    match config
        .folders
        .iter_mut()
        .find(|existing| existing.id == planned.item.id)
    {
        Some(existing) => *existing = planned.item,
        None => config.folders.push(planned.item),
    }
}

fn apply_server(config: &mut SshConfig, planned: PlannedItem<ServerEntry>) {
    match config
        .ungrouped
        .iter_mut()
        .find(|existing| existing.id == planned.item.id)
    {
        Some(existing) => *existing = planned.item,
        None => config.ungrouped.push(planned.item),
    }
}

fn apply_tunnel(config: &mut SshConfig, planned: PlannedItem<SavedTunnel>) {
    match config
        .tunnels
        .iter_mut()
        .find(|existing| existing.id == planned.item.id)
    {
        Some(existing) => *existing = planned.item,
        None => config.tunnels.push(planned.item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{BundleMetadata, BundledVault, ShareBundle};
    use crate::import_planner::plan;

    /// Fixed so tests can predict the written key's filename.
    const KEY_ID: Uuid = Uuid::from_u128(0x1234_5678_9abc_4def_8123_4567_89ab_cdef);

    fn sample_metadata() -> BundleMetadata {
        BundleMetadata {
            created_at: chrono::Utc::now(),
            source_host: "test-host".into(),
            termlab_version: "0.0.0-test".into(),
            includes_credentials: true,
        }
    }

    fn sample_server() -> ServerEntry {
        ServerEntry {
            id: "s1".into(),
            label: "prod".into(),
            host: "prod.example.com".into(),
            port: 22,
            user: Some("u".into()),
            auth_method: None,
            key_path: None,
            vault_account_id: None,
            proxy_command: None,
            proxy_jump: None,
        }
    }

    /// One server, one bundled key, no accounts/tunnels/folders — just
    /// enough to exercise key materialisation on its own.
    fn sample_bundle() -> ShareBundle {
        ShareBundle {
            schema_version: crate::SCHEMA_VERSION,
            metadata: sample_metadata(),
            folders: Vec::new(),
            servers: vec![sample_server()],
            tunnels: Vec::new(),
            vault: BundledVault::default(),
        }
    }

    /// Run the real planner against an empty config, so the resulting plan
    /// is exactly what `execute` would receive in production, not a
    /// hand-built stand-in for it.
    fn plan_from(bundle: &ShareBundle) -> ImportPlan {
        plan(bundle, &SshConfig::default(), &[])
    }

    fn plan_with_one_bundled_key() -> ImportPlan {
        let mut bundle = sample_bundle();
        bundle.vault.keys.push(BundledKey {
            id: KEY_ID,
            original_path: "/home/u/.ssh/id_ed25519".into(),
            material: base64::engine::general_purpose::STANDARD.encode(b"PRIVATE-KEY-BYTES"),
            public_material: None,
            passphrase: None,
            comment: "id_ed25519".into(),
        });
        plan_from(&bundle)
    }

    #[test]
    fn materialises_bundled_keys_with_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_with_one_bundled_key();
        let mut config = SshConfig::default();
        let out = execute(plan, &mut config, dir.path(), None).unwrap();
        let written = std::fs::read(dir.path().join(format!("{KEY_ID}"))).unwrap();
        assert_eq!(written, b"PRIVATE-KEY-BYTES");
        assert!(
            out.credentials_held_back,
            "no vault sink means credentials are held back"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(format!("{KEY_ID}")))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "key must be owner-read/write only");
        }
    }

    #[test]
    fn importing_the_same_bundle_twice_does_not_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = SshConfig::default();
        execute(plan_from(&sample_bundle()), &mut config, dir.path(), None).unwrap();
        let after_first = config.ungrouped.len();
        execute(plan_from(&sample_bundle()), &mut config, dir.path(), None).unwrap();
        assert_eq!(
            config.ungrouped.len(),
            after_first,
            "second import must replace, not append"
        );
    }
}
