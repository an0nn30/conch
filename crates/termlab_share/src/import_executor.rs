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
use crate::import_planner::{ImportPlan, ItemAction, PlannedFolder, PlannedItem};

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
    // have no counter of their own in `ImportOutcome`). Counted from
    // `.action`, not `.len()` — a `Skip` row (e.g. a `ReferenceBroken`
    // tunnel from `import_planner::plan`) still lives in the plan so the
    // frontend can show it, but `apply_tunnel`/`apply_server` write nothing
    // for it, so counting it here used to report "1 tunnel imported" for a
    // tunnel that never touched `config` at all.
    let servers_count = servers
        .iter()
        .filter(|p| actually_writes(&p.action))
        .count()
        + folders
            .iter()
            .flat_map(|f| f.entries.iter())
            .filter(|p| actually_writes(&p.action))
            .count();
    let tunnels_count = tunnels
        .iter()
        .filter(|p| actually_writes(&p.action))
        .count();

    // 1 & 2. Keys, then accounts — but only when a vault is actually open to
    // receive them. Keys used to be materialised to disk unconditionally,
    // before this check, so a locked or absent vault still got every
    // bundled private key written out even while the frontend told the user
    // "credentials were not imported" (2026-08-16 review finding I6). The
    // permissions on those files were correct, but writing secret material
    // the summary claims was never imported is the wrong side to be wrong
    // on, so keys are now held back right alongside the accounts they
    // belong to.
    let bundle_has_credentials = !keys.is_empty() || !accounts.is_empty();
    let (credentials, credentials_held_back) = match vault_sink {
        None => (0, bundle_has_credentials),
        Some(sink) => {
            // Keys first, so accounts have a real path to rewrite the
            // `termlab-bundle:<key-id>` marker to.
            let key_paths = materialise_keys(&keys, key_dir)?;
            let mut imported = 0usize;
            for planned in accounts {
                // Same reasoning as the server/tunnel counts above: a
                // `Skip` row must not be written or counted. No account is
                // planned as `Skip` today (the planner never produces
                // `LabelCollision` for accounts and always has a caller-
                // supplied id list to check `SameId` against), but this
                // keeps writing and counting consistent with `.action`
                // rather than relying on that staying true.
                if !actually_writes(&planned.action) {
                    continue;
                }
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
///
/// The write itself goes to a `.tmp` sibling first, then is renamed onto
/// `private_path` — never straight to the final name. A previous version of
/// this function used `create_new` directly on `private_path`, so a process
/// that died partway through `write_all` (disk full, killed mid-import) left
/// a *truncated* file already sitting at the final name; every later import
/// of the same key id then saw "already exists" and reused that truncated
/// file forever, since existence was the only signal, not completeness
/// (2026-08-16 review finding M14). With the temp-then-rename write, a
/// partial write can only ever leave the `.tmp` behind — `private_path`
/// itself either has the complete key or does not exist at all — so a
/// re-import after an interruption correctly writes it again instead of
/// reusing garbage.
fn materialise_keys(
    keys: &[BundledKey],
    key_dir: &Path,
) -> Result<HashMap<Uuid, PathBuf>, ShareError> {
    let mut paths = HashMap::with_capacity(keys.len());
    for key in keys {
        let private_path = key_dir.join(key.id.to_string());
        if private_path.exists() {
            // Already materialised (completely — see the doc comment above
            // on why existence at this exact path now implies completeness)
            // by a previous import of the same key id — reuse it, do not
            // touch its bytes. Its permissions are a different story: a
            // file left at this exact path by an interrupted earlier run is
            // not guaranteed to still be 0600 — trusting it blindly would
            // silently import a private key that's group/world-readable.
            // Tighten it before `paths` below hands the path to an
            // account's auth. Unix-only — Windows uses default ACLs here by
            // design, no counterpart needed.
            #[cfg(unix)]
            tighten_permissions_if_needed(&private_path)?;
        } else {
            let decoded: Zeroizing<Vec<u8>> = Zeroizing::new(
                base64::engine::general_purpose::STANDARD
                    .decode(&key.material)
                    .map_err(|e| ShareError::Malformed(e.to_string()))?,
            );
            write_owner_only_atomically(&private_path, &decoded)
                .map_err(|e| ShareError::Io(e.to_string()))?;

            if let Some(public) = &key.public_material {
                let decoded_pub = base64::engine::general_purpose::STANDARD
                    .decode(public)
                    .map_err(|e| ShareError::Malformed(e.to_string()))?;
                fs::write(key_dir.join(format!("{}.pub", key.id)), decoded_pub)
                    .map_err(|e| ShareError::Io(e.to_string()))?;
            }
        }
        paths.insert(key.id, private_path);
    }
    Ok(paths)
}

/// If `path`'s current permission bits aren't exactly owner-only (`0600`),
/// tighten them. Used only for the reuse branch of `materialise_keys` — a
/// freshly created file already gets `0600` atomically via
/// `create_owner_only_file`'s `OpenOptions::mode`, so this never runs on
/// that path.
#[cfg(unix)]
fn tighten_permissions_if_needed(path: &Path) -> Result<(), ShareError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path).map_err(|e| ShareError::Io(e.to_string()))?;
    if metadata.permissions().mode() & 0o777 != 0o600 {
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| ShareError::Io(e.to_string()))?;
    }
    Ok(())
}

/// Write `data` to `path` with owner-only (`0600`) permissions on Unix, set
/// at creation time via `OpenOptions` rather than a later `chmod` (never
/// write then chmod — a world-readable window, even briefly, defeats the
/// point), by writing to a `<path>.tmp` sibling first and renaming it onto
/// `path` only once the write has fully succeeded. Any stale `.tmp` left by
/// a previous interrupted attempt at this exact path is removed first so it
/// cannot block `create_new`.
fn write_owner_only_atomically(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);
    let _ = fs::remove_file(&tmp_path);

    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        let mut file = opts.open(&tmp_path)?;
        file.write_all(data)?;
    }
    fs::rename(&tmp_path, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp_path);
    })
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

/// Whether `action` actually writes the item somewhere (`Add`/`Replace`/
/// `Rename`) as opposed to leaving `config`/the vault untouched (`Skip`).
/// Used both by the `apply_*` functions below and by `execute`'s outcome
/// counts, so the two can never drift apart the way they did before a
/// `ReferenceBroken` tunnel's `Skip` row was still counted as imported.
fn actually_writes(action: &ItemAction) -> bool {
    !matches!(action, ItemAction::Skip)
}

// These apply functions switch on `planned.action` rather than re-deriving
// Add-vs-Replace from `config`'s live contents. `ItemAction` is on
// `PlannedItem` precisely so a consumer (sub-project 2: the per-row
// Skip/Replace/Rename picker) can override the planner's default and have
// that choice honoured verbatim — an executor that recomputes the decision
// from live state cannot respect an explicit override. `Replace` still
// falls back to a push if the id is unexpectedly absent (e.g. it was
// removed locally between planning and executing), so this never silently
// drops an item.

/// Upsert a bundled folder into `config`: create it if no local folder has
/// this id, otherwise update its metadata (name, expanded) in place — but
/// never replace its `entries` wholesale. Each entry is applied
/// individually via [`apply_server_entry`], so a local entry that was never
/// part of this bundle (the sender exported a narrower subset of a folder
/// they don't own alone) survives the import untouched. See
/// [`crate::import_planner::PlannedFolder`] for why the planner already
/// hands entries over one at a time rather than as a whole folder.
fn apply_folder(config: &mut SshConfig, planned: PlannedFolder) {
    let folder_idx = match config.folders.iter().position(|f| f.id == planned.id) {
        Some(idx) => {
            config.folders[idx].name = planned.name;
            config.folders[idx].expanded = planned.expanded;
            idx
        }
        None => {
            config.folders.push(ServerFolder {
                id: planned.id,
                name: planned.name,
                expanded: planned.expanded,
                entries: Vec::new(),
            });
            config.folders.len() - 1
        }
    };

    for planned_entry in planned.entries {
        match planned_entry.action {
            ItemAction::Add => config.folders[folder_idx].entries.push(planned_entry.item),
            ItemAction::Replace => {
                if !apply_server_entry(config, &planned_entry.item) {
                    config.folders[folder_idx].entries.push(planned_entry.item);
                }
            }
            // `Skip`/`Rename` are not produced by the planner for folder
            // entries yet (Task 1 only adds them as available actions; the
            // per-row override that lets a caller choose one is a later
            // task). Match them exhaustively now rather than leaving a
            // wildcard arm that would silently swallow real handling once
            // that task lands.
            ItemAction::Skip => {}
            ItemAction::Rename(_) => {
                config.folders[folder_idx].entries.push(planned_entry.item);
            }
        }
    }
}

fn apply_server(config: &mut SshConfig, planned: PlannedItem<ServerEntry>) {
    match planned.action {
        ItemAction::Add => config.ungrouped.push(planned.item),
        ItemAction::Replace => {
            if !apply_server_entry(config, &planned.item) {
                config.ungrouped.push(planned.item);
            }
        }
        // See the comment in `apply_folder`: not reachable from the planner
        // yet, matched exhaustively in anticipation of the per-row override.
        ItemAction::Skip => {}
        ItemAction::Rename(_) => config.ungrouped.push(planned.item),
    }
}

/// Update the server entry with `item.id` wherever it currently lives in
/// `config` — ungrouped or nested inside any folder — and report whether it
/// was found. A bundle server planned as `Replace` (its id already exists
/// somewhere in `config`, per `import_planner::plan`'s use of
/// `config.find_server`) must be updated at that actual location: a
/// `Replace` that only ever looked in `ungrouped` (or only in one specific
/// folder) would leave the existing copy stale and, worse, let the caller's
/// own "not found, so push" fallback create a second, duplicate copy
/// elsewhere (2026-08-16 review finding I2).
fn apply_server_entry(config: &mut SshConfig, item: &ServerEntry) -> bool {
    if let Some(existing) = config.ungrouped.iter_mut().find(|e| e.id == item.id) {
        *existing = item.clone();
        return true;
    }
    for folder in &mut config.folders {
        if let Some(existing) = folder.entries.iter_mut().find(|e| e.id == item.id) {
            *existing = item.clone();
            return true;
        }
    }
    false
}

fn apply_tunnel(config: &mut SshConfig, planned: PlannedItem<SavedTunnel>) {
    match planned.action {
        ItemAction::Add => config.tunnels.push(planned.item),
        ItemAction::Replace => match config
            .tunnels
            .iter_mut()
            .find(|existing| existing.id == planned.item.id)
        {
            Some(existing) => *existing = planned.item,
            None => config.tunnels.push(planned.item),
        },
        // A `ReferenceBroken` tunnel (its host resolves nowhere) is planned
        // with action `Skip` by `import_planner::plan` precisely so it must
        // NOT be applied here — this is the load-bearing case, not a
        // leftover default. `Rename` is not reachable from the planner yet;
        // matched exhaustively in anticipation of the per-row override.
        ItemAction::Skip => {}
        ItemAction::Rename(_) => config.tunnels.push(planned.item),
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

    /// Run the real planner against `config`, so the resulting plan is
    /// exactly what `execute` would receive in production, not a
    /// hand-built stand-in for it. Callers that want to observe a second
    /// import correctly classify against what a first `execute()` actually
    /// wrote must pass that same (now-mutated) `config` back in here rather
    /// than a fresh default — see `importing_the_same_bundle_twice_...`.
    fn plan_from(bundle: &ShareBundle, config: &SshConfig) -> ImportPlan {
        plan(bundle, config, &[])
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
        plan_from(&bundle, &SshConfig::default())
    }

    /// Records every account handed to it. Good enough to prove
    /// `execute`'s vault-facing behaviour without opening a real vault.
    #[derive(Default)]
    struct FakeVaultSink {
        accounts: Vec<VaultAccount>,
    }

    impl VaultSink for FakeVaultSink {
        fn upsert_account(&mut self, account: VaultAccount) -> Result<(), String> {
            self.accounts.push(account);
            Ok(())
        }
    }

    #[test]
    fn materialises_bundled_keys_with_owner_only_permissions_when_vault_sink_present() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_with_one_bundled_key();
        let mut config = SshConfig::default();
        let mut sink = FakeVaultSink::default();
        let out = execute(plan, &mut config, dir.path(), Some(&mut sink)).unwrap();
        let written = std::fs::read(dir.path().join(format!("{KEY_ID}"))).unwrap();
        assert_eq!(written, b"PRIVATE-KEY-BYTES");
        assert!(
            !out.credentials_held_back,
            "a vault sink was supplied, credentials must not be held back"
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

    /// I6 regression: with no vault open on this machine, the frontend tells
    /// the user "credentials were not imported" — so the bundled private key
    /// must genuinely not be written to disk either, not just held back from
    /// the vault. Materialising it anyway (the pre-fix behaviour) meant the
    /// permissions were correct but the summary was lying about what
    /// happened (2026-08-16 review finding I6).
    #[test]
    fn no_vault_sink_holds_back_keys_and_does_not_write_them_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_with_one_bundled_key();
        let mut config = SshConfig::default();
        let out = execute(plan, &mut config, dir.path(), None).unwrap();

        assert!(
            !dir.path().join(format!("{KEY_ID}")).exists(),
            "a bundled key must not be written to disk when there is no vault sink to hold its account"
        );
        assert_eq!(out.credentials, 0);
        assert!(
            out.credentials_held_back,
            "the bundle did carry a key, so credentials_held_back must be true"
        );
    }

    /// M15 regression: `credentials_held_back` must reflect whether the
    /// *bundle* actually carried credentials, not just whether a vault sink
    /// was supplied. A bundle with no keys and no accounts imported with no
    /// vault open has nothing to hold back, so the frontend must not pop the
    /// "credentials were not imported" dialog for it.
    #[test]
    fn no_vault_sink_but_bundle_has_no_credentials_does_not_flag_held_back() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = sample_bundle(); // no keys, no accounts
        let mut config = SshConfig::default();
        let out = execute(plan_from(&bundle, &config), &mut config, dir.path(), None).unwrap();
        assert!(
            !out.credentials_held_back,
            "nothing to hold back: the bundle carried no keys or accounts"
        );
    }

    /// A file already sitting at the key's target path (e.g. left behind by
    /// an interrupted earlier import — the process died before completing
    /// the same key id) with loose permissions must not be trusted as-is:
    /// `materialise_keys` should tighten it to 0600 on the reuse path,
    /// without touching its bytes.
    #[test]
    #[cfg(unix)]
    fn reused_key_file_with_loose_permissions_is_tightened_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join(format!("{KEY_ID}"));
        std::fs::write(&key_path, b"PRE-EXISTING-BYTES").unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let plan = plan_with_one_bundled_key();
        let mut config = SshConfig::default();
        let mut sink = FakeVaultSink::default();
        execute(plan, &mut config, dir.path(), Some(&mut sink)).unwrap();

        let written = std::fs::read(&key_path).unwrap();
        assert_eq!(
            written, b"PRE-EXISTING-BYTES",
            "reuse must not rewrite the file's bytes"
        );
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a reused key file must be tightened to owner-only permissions"
        );
    }

    /// M14 regression: a `.tmp` file left behind at the key's target name by
    /// a previous run that died mid-write must not block (or be mistaken
    /// for) a correct materialisation on the next import. Before the fix, a
    /// truncated write could land directly at the final path and then be
    /// "reused" forever; the temp-then-rename write means the final path
    /// only ever exists complete, so a stale `.tmp` is simply overwritten.
    #[test]
    fn stale_tmp_file_from_an_interrupted_write_does_not_block_a_correct_import() {
        let dir = tempfile::tempdir().unwrap();
        let tmp_path = dir.path().join(format!("{KEY_ID}.tmp"));
        std::fs::write(&tmp_path, b"PARTIAL-GARBAGE-FROM-A-DEAD-PROCESS").unwrap();

        let plan = plan_with_one_bundled_key();
        let mut config = SshConfig::default();
        let mut sink = FakeVaultSink::default();
        execute(plan, &mut config, dir.path(), Some(&mut sink)).unwrap();

        let written = std::fs::read(dir.path().join(format!("{KEY_ID}"))).unwrap();
        assert_eq!(
            written, b"PRIVATE-KEY-BYTES",
            "the final file must contain the complete, correct key material"
        );
        assert!(
            !tmp_path.exists(),
            "the .tmp file must be consumed by the rename, not left behind"
        );
    }

    #[test]
    fn importing_the_same_bundle_twice_does_not_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = SshConfig::default();
        let bundle = sample_bundle();

        execute(plan_from(&bundle, &config), &mut config, dir.path(), None).unwrap();
        let after_first = config.ungrouped.len();
        assert_eq!(
            after_first, 1,
            "first import of one server must land exactly one entry, not zero or more"
        );

        // Plan against `config` as the first import actually left it, so
        // the second plan sees "s1" already present and classifies it as
        // Replace — the scenario `plan_from` was reworked to allow a test
        // to observe (see its doc comment).
        execute(plan_from(&bundle, &config), &mut config, dir.path(), None).unwrap();
        assert_eq!(
            config.ungrouped.len(),
            after_first,
            "second import must replace, not append"
        );
    }

    /// A server synthesised (during export) from a legacy `key_path` arrives
    /// with both `vault_account_id` set and a stale `key_path` still
    /// pointing at the sender's machine. Import must not "repair" that
    /// stale path — `vault_account_id` is authoritative, matching the
    /// existing convention in `SshConfig::has_legacy_entries`. No code path
    /// in the planner or executor touches `ServerEntry::key_path` at all,
    /// so this pins that by construction against a future "helpful" fix
    /// that starts rewriting it.
    #[test]
    fn server_with_both_vault_account_id_and_stale_key_path_is_left_untouched() {
        let account_id = Uuid::new_v4();
        let mut server = sample_server();
        server.vault_account_id = Some(account_id);
        server.key_path = Some("/home/sender/.ssh/id_ed25519".into());

        let mut bundle = sample_bundle();
        bundle.servers = vec![server];

        let dir = tempfile::tempdir().unwrap();
        let mut config = SshConfig::default();
        execute(plan_from(&bundle, &config), &mut config, dir.path(), None).unwrap();

        let imported = config.find_server("s1").expect("server must be imported");
        assert_eq!(imported.vault_account_id, Some(account_id));
        assert_eq!(
            imported.key_path.as_deref(),
            Some("/home/sender/.ssh/id_ed25519"),
            "stale sender-machine key_path must be left exactly as it arrived"
        );
    }

    fn make_server(id: &str, label: &str, host: &str, user: &str) -> ServerEntry {
        ServerEntry {
            id: id.into(),
            label: label.into(),
            host: host.into(),
            port: 22,
            user: Some(user.into()),
            auth_method: None,
            key_path: None,
            vault_account_id: None,
            proxy_command: None,
            proxy_jump: None,
        }
    }

    fn bundle_with_folder(folder: ServerFolder) -> ShareBundle {
        ShareBundle {
            schema_version: crate::SCHEMA_VERSION,
            metadata: sample_metadata(),
            folders: vec![folder],
            servers: Vec::new(),
            tunnels: Vec::new(),
            vault: BundledVault::default(),
        }
    }

    /// C1 regression: machine B already has folder "prod" holding two hosts
    /// — "a" (which the sender also has and is re-exporting) and
    /// "bobs-own-host" (local-only, never part of the sender's export). A
    /// bundle that re-exports "prod" with just "a" must update "a" in
    /// place and leave "bobs-own-host" exactly as it was — not delete it,
    /// as the pre-fix whole-folder `Replace` used to (2026-08-16 review
    /// finding C1). This is exactly the non-empty-machine-B fixture the
    /// original per-task tests never exercised, which is why the bug got
    /// through.
    #[test]
    fn importing_a_partial_folder_re_export_does_not_delete_local_only_entries() {
        let dir = tempfile::tempdir().unwrap();

        let host_a = make_server("a", "a", "a.example.com", "alice");
        let bobs_host = make_server("bobs-own-host", "bob's host", "bob.example.com", "bob");

        let mut config = SshConfig::default();
        config.folders.push(ServerFolder {
            id: "prod".into(),
            name: "Prod".into(),
            expanded: true,
            entries: vec![host_a.clone(), bobs_host.clone()],
        });

        let mut updated_a = host_a.clone();
        updated_a.label = "a (renamed by sender)".into();
        let bundle = bundle_with_folder(ServerFolder {
            id: "prod".into(),
            name: "Prod".into(),
            expanded: true,
            entries: vec![updated_a],
        });

        let outcome = execute(plan_from(&bundle, &config), &mut config, dir.path(), None).unwrap();
        assert!(outcome.skipped.is_empty(), "skipped: {:?}", outcome.skipped);

        let prod = config
            .folders
            .iter()
            .find(|f| f.id == "prod")
            .expect("folder must still exist");
        assert_eq!(
            prod.entries.len(),
            2,
            "bob's own host must survive the import: {:?}",
            prod.entries
        );
        assert!(
            prod.entries.iter().any(|e| e.id == "bobs-own-host"),
            "bobs-own-host must not have been deleted: {:?}",
            prod.entries
        );
        let a_after = prod
            .entries
            .iter()
            .find(|e| e.id == "a")
            .expect("host a must still be present");
        assert_eq!(
            a_after.label, "a (renamed by sender)",
            "the entry actually present in the bundle must still be updated in place"
        );
    }

    /// I2 regression: machine B already has host "a" nested inside folder
    /// "prod". A later bundle carries "a" as an *ungrouped* server (the
    /// shape a tunnel's auto-pulled dependency always takes on export,
    /// regardless of where the host lived on the sender's machine — see
    /// `export_planner.rs`'s `resolve_tunnel_host`). Importing it must
    /// update the existing folder-nested entry in place, not create a
    /// second, duplicate copy in `ungrouped` (2026-08-16 review finding
    /// I2).
    #[test]
    fn bundle_server_matching_an_existing_folder_nested_host_updates_it_in_place() {
        let dir = tempfile::tempdir().unwrap();

        let host_a = make_server("a", "a", "a.example.com", "alice");
        let mut config = SshConfig::default();
        config.folders.push(ServerFolder {
            id: "prod".into(),
            name: "Prod".into(),
            expanded: true,
            entries: vec![host_a.clone()],
        });

        let mut updated_a = host_a.clone();
        updated_a.label = "a (from bundle)".into();
        let bundle = ShareBundle {
            schema_version: crate::SCHEMA_VERSION,
            metadata: sample_metadata(),
            folders: Vec::new(),
            servers: vec![updated_a],
            tunnels: Vec::new(),
            vault: BundledVault::default(),
        };

        let outcome = execute(plan_from(&bundle, &config), &mut config, dir.path(), None).unwrap();
        assert!(outcome.skipped.is_empty(), "skipped: {:?}", outcome.skipped);

        assert_eq!(
            config.ungrouped.len(),
            0,
            "must not create a duplicate ungrouped copy: {:?}",
            config.ungrouped
        );
        assert_eq!(config.folders[0].entries.len(), 1);
        assert_eq!(
            config.folders[0].entries[0].label, "a (from bundle)",
            "the existing folder-nested host must be updated in place"
        );
    }

    /// Regression: `execute` used to compute `outcome.tunnels` as
    /// `tunnels.len()` before applying anything, which was accurate only
    /// because an unresolvable tunnel never reached `plan.tunnels` at all.
    /// Now that `import_planner::plan` keeps it as a `ReferenceBroken`/
    /// `Skip` row instead of dropping it, a raw `.len()` counts a tunnel
    /// that `apply_tunnel`'s `Skip => {}` writes nothing for — the frontend
    /// would report "1 tunnel imported" for an import that changed nothing.
    #[test]
    fn unresolvable_tunnel_does_not_inflate_the_reported_tunnel_count() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = ShareBundle {
            schema_version: crate::SCHEMA_VERSION,
            metadata: sample_metadata(),
            folders: Vec::new(),
            servers: Vec::new(),
            tunnels: vec![SavedTunnel {
                id: Uuid::new_v4(),
                label: "dangling".into(),
                session_key: "u@ghost.example.com:22".into(),
                server_entry_id: Some("ghost".into()),
                local_port: 5432,
                remote_host: "db.internal".into(),
                remote_port: 5432,
                auto_start: false,
            }],
            vault: BundledVault::default(),
        };

        let mut config = SshConfig::default();
        let outcome = execute(plan_from(&bundle, &config), &mut config, dir.path(), None).unwrap();

        assert_eq!(
            outcome.tunnels, 0,
            "a ReferenceBroken/Skip tunnel writes nothing, so it must not be counted as imported"
        );
        assert!(
            config.tunnels.is_empty(),
            "the tunnel must genuinely not have been written: {:?}",
            config.tunnels
        );
    }
}
