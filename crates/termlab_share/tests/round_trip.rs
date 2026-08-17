//! End-to-end round trip for the share-bundle feature: a "machine A"
//! selection goes through the real `export_planner::plan`, gets encrypted
//! with `codec::encode`, decrypted with `codec::decode`, planned against an
//! empty "machine B" config with `import_planner::plan`, and finally
//! applied to disk with `import_executor::execute`. No unit under test is
//! reimplemented here — this file only supplies the two trait adapters
//! (`KeyReader`, `VaultSink`) the real units need to talk to a filesystem
//! and a vault, both backed by `tempfile::tempdir()`.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use termlab_remote::config::{SavedTunnel, ServerEntry, ServerFolder, SshConfig};
use termlab_share::codec::{decode, encode};
use termlab_share::export_planner::{ExportRequest, KeyReader, plan as plan_export};
use termlab_share::import_executor::{VaultSink, execute};
use termlab_share::import_planner::{ConflictStatus, ItemAction, plan as plan_import};
use termlab_vault::model::{AuthMethod, VaultAccount};

const PASSWORD: &[u8] = b"correct horse battery staple";

/// Reads key bytes straight off disk. This is the same shape as the real
/// `FsKeyReader` in `termlab_tauri::share_commands` (a plain `fs::read`),
/// minus its private-key-format sniffing, which isn't relevant here — the
/// test controls exactly what bytes are written and just needs them to
/// travel unchanged through the whole pipeline.
struct FileKeyReader;

impl KeyReader for FileKeyReader {
    fn read_key(&self, path: &str) -> Result<termlab_share::export_planner::KeyBytes, String> {
        std::fs::read(path)
            .map(|bytes| (zeroize::Zeroizing::new(bytes), None))
            .map_err(|e| format!("Key file not found: {path} ({e})"))
    }
}

/// Records every account handed to it, replacing on id collision the same
/// way a real vault upsert would. Good enough to prove the executor's
/// account-import behaviour (rewritten key paths, no duplication on a
/// second import) without opening a real vault.
#[derive(Default)]
struct RecordingVaultSink {
    accounts: Vec<VaultAccount>,
}

impl RecordingVaultSink {
    fn find(&self, id: Uuid) -> &VaultAccount {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .unwrap_or_else(|| panic!("account {id} was not recorded by the vault sink"))
    }
}

impl VaultSink for RecordingVaultSink {
    fn upsert_account(&mut self, account: VaultAccount) -> Result<(), String> {
        if let Some(existing) = self.accounts.iter_mut().find(|a| a.id == account.id) {
            *existing = account;
        } else {
            self.accounts.push(account);
        }
        Ok(())
    }
}

/// Machine A's fixture: two folders (one holding the password-account
/// server, one holding the key-account server), one ungrouped server using
/// a bare legacy `key_path`, and two tunnels — one resolving directly via
/// `server_entry_id` to an already-selected host, one resolving only
/// through a `~/.ssh/config` alias that must be auto-pulled in.
struct MachineA {
    config: SshConfig,
    ssh_config_entries: Vec<ServerEntry>,
    accounts: Vec<VaultAccount>,
    server_ids: Vec<String>,
    tunnel_ids: Vec<String>,

    // Expected identities, captured up front so assertions check against
    // what was actually constructed rather than re-deriving them.
    server_pw_id: String,
    server_key_id: String,
    server_barekey_id: String,
    tunnel_internal_id: Uuid,
    tunnel_alias_id: Uuid,
    account_pw_id: Uuid,
    account_key_id: Uuid,

    key_account_key_bytes: Vec<u8>,
    key_account_key_path: PathBuf,
    barekey_bytes: Vec<u8>,
    barekey_path: PathBuf,
}

fn build_machine_a(key_dir: &Path) -> MachineA {
    let account_pw_id = Uuid::new_v4();
    let account_key_id = Uuid::new_v4();

    let key_account_key_path = key_dir.join("id_ed25519_key_account");
    let key_account_key_bytes = b"FAKE-PRIVATE-KEY-FOR-KEY-ACCOUNT".to_vec();
    std::fs::write(&key_account_key_path, &key_account_key_bytes).unwrap();

    let barekey_path = key_dir.join("id_rsa_bare");
    let barekey_bytes = b"FAKE-PRIVATE-KEY-FOR-BARE-SERVER".to_vec();
    std::fs::write(&barekey_path, &barekey_bytes).unwrap();

    let server_pw = ServerEntry {
        id: "srv-pw".into(),
        label: "password box".into(),
        host: "pw.example.com".into(),
        port: 22,
        user: Some("alice".into()),
        auth_method: None,
        key_path: None,
        vault_account_id: Some(account_pw_id),
        proxy_command: None,
        proxy_jump: None,
    };

    let server_key = ServerEntry {
        id: "srv-key".into(),
        label: "key box".into(),
        host: "key.example.com".into(),
        port: 22,
        user: Some("bob".into()),
        auth_method: None,
        key_path: None,
        vault_account_id: Some(account_key_id),
        proxy_command: None,
        proxy_jump: None,
    };

    let server_barekey = ServerEntry {
        id: "srv-barekey".into(),
        label: "bare key box".into(),
        host: "bare.example.com".into(),
        port: 2222,
        user: Some("carol".into()),
        auth_method: Some("key".into()),
        key_path: Some(barekey_path.to_string_lossy().to_string()),
        vault_account_id: None,
        proxy_command: None,
        proxy_jump: None,
    };

    // Two folders — server_pw and server_key each live nested inside one,
    // not ungrouped, so the round trip actually exercises folder-nested
    // vault accounts (see export_planner's Stage 3/4/6 fix).
    let folder_ops = ServerFolder {
        id: "folder-ops".into(),
        name: "Ops".into(),
        expanded: true,
        entries: vec![server_pw.clone()],
    };
    let folder_db = ServerFolder {
        id: "folder-db".into(),
        name: "DB".into(),
        expanded: true,
        entries: vec![server_key.clone()],
    };

    // "Internal" tunnel: resolves via server_entry_id to a host that is
    // already directly selected, so resolve_tunnel_host's branch 1 returns
    // immediately without needing to auto-pull anything.
    let tunnel_internal = SavedTunnel {
        id: Uuid::new_v4(),
        label: "internal-tunnel".into(),
        session_key: "bob@key.example.com:22".into(),
        server_entry_id: Some("srv-key".into()),
        local_port: 5432,
        remote_host: "db.internal".into(),
        remote_port: 5432,
        auto_start: false,
    };

    // Alias tunnel: no server_entry_id, resolves only against a
    // ~/.ssh/config entry the user never selected directly — exercises
    // resolve_tunnel_host's branch 3 auto-pull.
    let tunnel_alias = SavedTunnel {
        id: Uuid::new_v4(),
        label: "alias-tunnel".into(),
        session_key: "user@bastion.example.com:22".into(),
        server_entry_id: None,
        local_port: 8080,
        remote_host: "internal.local".into(),
        remote_port: 80,
        auto_start: false,
    };

    let bastion_alias = ServerEntry {
        id: "sshconfig_bastion".into(),
        label: "bastion".into(),
        host: "bastion.example.com".into(),
        port: 22,
        user: Some("user".into()),
        auth_method: Some("key".into()),
        key_path: None,
        vault_account_id: None,
        proxy_command: None,
        proxy_jump: None,
    };

    let config = SshConfig {
        folders: vec![folder_ops, folder_db],
        ungrouped: vec![server_barekey],
        tunnels: vec![tunnel_internal.clone(), tunnel_alias.clone()],
    };

    let account_pw = VaultAccount {
        id: account_pw_id,
        display_name: "password account".into(),
        username: "alice".into(),
        auth: AuthMethod::Password("hunter2".into()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let account_key = VaultAccount {
        id: account_key_id,
        display_name: "key account".into(),
        username: "bob".into(),
        auth: AuthMethod::Key {
            path: key_account_key_path.clone(),
            passphrase: Some("passphrase123".into()),
        },
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    MachineA {
        config,
        ssh_config_entries: vec![bastion_alias],
        accounts: vec![account_pw, account_key],
        server_ids: vec!["srv-pw".into(), "srv-key".into(), "srv-barekey".into()],
        tunnel_ids: vec![tunnel_internal.id.to_string(), tunnel_alias.id.to_string()],
        server_pw_id: "srv-pw".into(),
        server_key_id: "srv-key".into(),
        server_barekey_id: "srv-barekey".into(),
        tunnel_internal_id: tunnel_internal.id,
        tunnel_alias_id: tunnel_alias.id,
        account_pw_id,
        account_key_id,
        key_account_key_bytes,
        key_account_key_path,
        barekey_bytes,
        barekey_path,
    }
}

fn export_request(a: &MachineA, include_credentials: bool) -> ExportRequest<'_> {
    ExportRequest {
        config: &a.config,
        ssh_config_entries: &a.ssh_config_entries,
        server_ids: a.server_ids.clone(),
        tunnel_ids: a.tunnel_ids.clone(),
        declined_server_ids: Vec::new(),
        include_credentials,
        accounts: a.accounts.clone(),
        source_host: "machine-a".into(),
        termlab_version: "0.0.0-test".into(),
    }
}

#[test]
fn full_round_trip_preserves_ids_and_materialises_keys() {
    let key_dir_a = tempfile::tempdir().unwrap();
    let key_dir_b = tempfile::tempdir().unwrap();
    let a = build_machine_a(key_dir_a.path());

    // --- Export on machine A -------------------------------------------
    let export_plan = plan_export(export_request(&a, true), &FileKeyReader);
    assert!(
        export_plan.warnings.is_empty(),
        "unexpected export warnings: {:?}",
        export_plan.warnings
    );
    assert_eq!(
        export_plan.auto_pulled.len(),
        1,
        "only the alias tunnel's host should be auto-pulled: {:?}",
        export_plan.auto_pulled
    );
    assert!(export_plan.auto_pulled[0].contains("bastion"));
    assert_eq!(
        export_plan.bundle.vault.accounts.len(),
        3,
        "password account + key account + synthesised bare-key account"
    );
    assert_eq!(
        export_plan.bundle.vault.keys.len(),
        2,
        "key account's key + bare key_path's key, not the password account"
    );

    // --- Encrypt, then decrypt -------------------------------------------
    let bytes = encode(&export_plan.bundle, PASSWORD).unwrap();
    let bundle = decode(&bytes, PASSWORD).unwrap();

    // --- Plan and execute against an empty machine B ---------------------
    let mut config_b = SshConfig::default();
    let import_plan_1 = plan_import(&bundle, &config_b, &[]);
    assert!(
        import_plan_1.skipped.is_empty(),
        "unexpected skipped items: {:?}",
        import_plan_1.skipped
    );

    let mut sink = RecordingVaultSink::default();
    let outcome = execute(import_plan_1, &mut config_b, key_dir_b.path(), Some(&mut sink)).unwrap();
    assert!(
        !outcome.credentials_held_back,
        "a vault sink was supplied, credentials must not be held back"
    );
    assert_eq!(outcome.credentials, 3);
    assert_eq!(outcome.servers, 4, "2 folder-nested + 2 ungrouped (bare key + bastion alias)");
    assert_eq!(outcome.tunnels, 2);

    // --- Every server and tunnel arrives with the same ids ---------------
    assert!(config_b.find_server(&a.server_pw_id).is_some());
    assert!(config_b.find_server(&a.server_key_id).is_some());
    assert!(config_b.find_server(&a.server_barekey_id).is_some());
    assert!(config_b.find_tunnel(&a.tunnel_internal_id).is_some());
    assert!(config_b.find_tunnel(&a.tunnel_alias_id).is_some());
    // The folder-nested servers actually stayed nested, not flattened.
    assert_eq!(config_b.folders.len(), 2);
    assert_eq!(config_b.find_server_folder(&a.server_pw_id), Some("folder-ops"));
    assert_eq!(config_b.find_server_folder(&a.server_key_id), Some("folder-db"));

    // --- Key file materialised in machine B's key dir, mode 0600, ---
    // --- byte-identical, and the account no longer points at A's path ---
    let key_account = sink.find(a.account_key_id);
    let AuthMethod::Key {
        path: key_account_path,
        ..
    } = &key_account.auth
    else {
        panic!("expected Key auth for the key account");
    };
    assert!(
        key_account_path.starts_with(key_dir_b.path()),
        "auth path must point at machine B's key dir: {key_account_path:?}"
    );
    assert_ne!(
        key_account_path, &a.key_account_key_path,
        "must not still point at machine A's original path"
    );
    assert!(
        !key_account_path.to_string_lossy().contains("termlab-bundle:"),
        "the termlab-bundle:<id> marker must have been rewritten to a real path"
    );
    let materialised = std::fs::read(key_account_path).unwrap();
    assert_eq!(materialised, a.key_account_key_bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(key_account_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "key must be owner-read/write only");
    }

    // Same checks for the bare `key_path` server's synthesised account.
    let barekey_server = config_b.find_server(&a.server_barekey_id).unwrap();
    let barekey_account_id = barekey_server
        .vault_account_id
        .expect("bare key_path server must have gained a synthesised vault account id");
    let barekey_account = sink.find(barekey_account_id);
    let AuthMethod::Key {
        path: barekey_account_path,
        ..
    } = &barekey_account.auth
    else {
        panic!("expected Key auth for the synthesised bare-key account");
    };
    assert!(barekey_account_path.starts_with(key_dir_b.path()));
    assert_ne!(barekey_account_path, &a.barekey_path);
    assert!(!barekey_account_path.to_string_lossy().contains("termlab-bundle:"));
    let materialised_bare = std::fs::read(barekey_account_path).unwrap();
    assert_eq!(materialised_bare, a.barekey_bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(barekey_account_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "key must be owner-read/write only");
    }
    // Its legacy key_path field itself is left exactly as it arrived —
    // vault_account_id is authoritative, per import_executor's convention.
    assert_eq!(
        barekey_server.key_path.as_deref(),
        Some(a.barekey_path.to_string_lossy().to_string()).as_deref()
    );

    // Password account passed through untouched.
    let pw_account = sink.find(a.account_pw_id);
    assert!(matches!(&pw_account.auth, AuthMethod::Password(p) if p == "hunter2"));

    // --- A second execute of the same bundle: counts unchanged -----------
    let existing_account_ids: Vec<Uuid> = sink.accounts.iter().map(|acc| acc.id).collect();
    let import_plan_2 = plan_import(&bundle, &config_b, &existing_account_ids);
    assert!(import_plan_2.skipped.is_empty());
    let outcome_2 = execute(import_plan_2, &mut config_b, key_dir_b.path(), Some(&mut sink)).unwrap();

    assert_eq!(outcome_2.servers, outcome.servers);
    assert_eq!(outcome_2.tunnels, outcome.tunnels);
    assert_eq!(outcome_2.credentials, outcome.credentials);
    assert_eq!(config_b.folders.len(), 2);
    assert_eq!(
        config_b.folders.iter().map(|f| f.entries.len()).sum::<usize>(),
        2
    );
    assert_eq!(config_b.ungrouped.len(), 2);
    assert_eq!(config_b.tunnels.len(), 2);
    assert_eq!(sink.accounts.len(), 3, "second import must replace, not duplicate");
    let key_files_in_b = std::fs::read_dir(key_dir_b.path()).unwrap().count();
    assert_eq!(
        key_files_in_b, 2,
        "re-materialising an already-imported key must reuse the file, not add another"
    );
}

#[test]
fn export_without_credentials_carries_no_vault_material() {
    let key_dir_a = tempfile::tempdir().unwrap();
    let a = build_machine_a(key_dir_a.path());

    let export_plan = plan_export(export_request(&a, false), &FileKeyReader);

    assert!(export_plan.bundle.vault.is_empty());
    assert!(!export_plan.bundle.metadata.includes_credentials);
    assert!(
        export_plan
            .bundle
            .servers
            .iter()
            .all(|s| s.vault_account_id.is_none()),
        "ungrouped servers must have vault_account_id stripped"
    );
    assert!(
        export_plan
            .bundle
            .folders
            .iter()
            .flat_map(|f| &f.entries)
            .all(|s| s.vault_account_id.is_none()),
        "folder-nested servers must have vault_account_id stripped too"
    );
}

/// The conflicting counterpart to
/// `full_round_trip_preserves_ids_and_materialises_keys`'s empty-machine-B
/// case. Machine B is seeded to hit every `ConflictStatus` at once:
///
/// - `srv-same-id` exists on B under the same id as a bundle server
///   (`SameId`);
/// - `srv-label-src` is a fresh id in the bundle whose label ("prod")
///   collides with a *different* local server, `srv-label-dst`
///   (`LabelCollision`);
/// - `srv-new` exists nowhere on B (`New`);
/// - `tunnel-broken` depends on `srv-ghost`, a host machine A has locally
///   but the export explicitly declines to pull in as a dependency, so it
///   never enters the bundle at all and resolves on neither the bundle nor
///   B (`ReferenceBroken`).
///
/// Runs the real `export_planner::plan` -> `codec::encode`/`decode` ->
/// `import_planner::plan` -> `import_executor::execute` pipeline twice
/// against the same machine-A bundle: once with the plan's defaults
/// unmodified (against a throwaway clone of B, so the second pass still
/// starts from B's true original state), and once re-planned with the
/// `SameId` row overridden to `Skip` and the `LabelCollision` row
/// overridden to `Rename`.
#[test]
fn conflicting_machine_b_produces_every_status_and_honours_overrides() {
    let account_id = Uuid::new_v4();
    let tunnel_ok_id = Uuid::new_v4();
    let tunnel_broken_id = Uuid::new_v4();

    // --- Machine A's selection -------------------------------------------
    let server_same_id = ServerEntry {
        id: "srv-same-id".into(),
        label: "same-id-host".into(),
        host: "same.example.com".into(),
        port: 22,
        user: Some("alice".into()),
        auth_method: None,
        key_path: None,
        vault_account_id: None,
        proxy_command: None,
        proxy_jump: None,
    };
    let server_label_src = ServerEntry {
        id: "srv-label-src".into(),
        label: "prod".into(),
        host: "labelsrc.example.com".into(),
        port: 22,
        user: Some("bob".into()),
        auth_method: None,
        key_path: None,
        vault_account_id: None,
        proxy_command: None,
        proxy_jump: None,
    };
    let server_new = ServerEntry {
        id: "srv-new".into(),
        label: "new-host".into(),
        host: "new.example.com".into(),
        port: 22,
        user: Some("carol".into()),
        auth_method: None,
        key_path: None,
        vault_account_id: Some(account_id),
        proxy_command: None,
        proxy_jump: None,
    };
    // Present on machine A but never directly selected for export; the
    // tunnel below depends on it and the export explicitly declines to pull
    // it in as a dependency, so it never enters the bundle at all — the
    // fixture for `ReferenceBroken`.
    let server_ghost = ServerEntry {
        id: "srv-ghost".into(),
        label: "ghost-host".into(),
        host: "ghost.example.com".into(),
        port: 22,
        user: Some("dave".into()),
        auth_method: None,
        key_path: None,
        vault_account_id: None,
        proxy_command: None,
        proxy_jump: None,
    };

    let tunnel_ok = SavedTunnel {
        id: tunnel_ok_id,
        label: "tunnel-ok".into(),
        session_key: "carol@new.example.com:22".into(),
        server_entry_id: Some("srv-new".into()),
        local_port: 5000,
        remote_host: "db.internal".into(),
        remote_port: 5432,
        auto_start: false,
    };
    let tunnel_broken = SavedTunnel {
        id: tunnel_broken_id,
        label: "tunnel-broken".into(),
        session_key: "dave@ghost.example.com:22".into(),
        server_entry_id: Some("srv-ghost".into()),
        local_port: 6000,
        remote_host: "cache.internal".into(),
        remote_port: 6379,
        auto_start: false,
    };

    let config_a = SshConfig {
        folders: Vec::new(),
        ungrouped: vec![server_same_id, server_label_src, server_new, server_ghost],
        tunnels: vec![tunnel_ok, tunnel_broken],
    };

    let account = VaultAccount {
        id: account_id,
        display_name: "new host account".into(),
        username: "carol".into(),
        auth: AuthMethod::Password("hunter3".into()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    // --- Export on machine A, with credentials ---------------------------
    let export_plan = plan_export(
        ExportRequest {
            config: &config_a,
            ssh_config_entries: &[],
            server_ids: vec![
                "srv-same-id".into(),
                "srv-label-src".into(),
                "srv-new".into(),
            ],
            tunnel_ids: vec![tunnel_ok_id.to_string(), tunnel_broken_id.to_string()],
            declined_server_ids: vec!["srv-ghost".into()],
            include_credentials: true,
            accounts: vec![account],
            source_host: "machine-a".into(),
            termlab_version: "0.0.0-test".into(),
        },
        &FileKeyReader,
    );
    assert!(
        export_plan.bundle.servers.iter().all(|s| s.id != "srv-ghost"),
        "the declined dependency must never enter the bundle: {:?}",
        export_plan.bundle.servers
    );
    assert!(
        export_plan.warnings.iter().any(|w| w.contains("ghost-host")),
        "the decline should be recorded as a warning: {:?}",
        export_plan.warnings
    );
    let broken_in_bundle = export_plan
        .bundle
        .tunnels
        .iter()
        .find(|t| t.id == tunnel_broken_id)
        .unwrap();
    assert_eq!(
        broken_in_bundle.server_entry_id.as_deref(),
        Some("srv-ghost"),
        "the broken tunnel must keep referencing the host that was never bundled"
    );

    // --- Encrypt, then decrypt --------------------------------------------
    let bytes = encode(&export_plan.bundle, PASSWORD).unwrap();
    let bundle = decode(&bytes, PASSWORD).unwrap();

    // --- Machine B: seeded to hit every status at once ---------------------
    let config_b = SshConfig {
        folders: Vec::new(),
        ungrouped: vec![
            ServerEntry {
                id: "srv-same-id".into(), // same id as the bundle's srv-same-id
                label: "old-label".into(),
                host: "old.example.com".into(),
                port: 22,
                user: Some("old-user".into()),
                auth_method: None,
                key_path: None,
                vault_account_id: None,
                proxy_command: None,
                proxy_jump: None,
            },
            ServerEntry {
                id: "srv-label-dst".into(), // different id, same label as srv-label-src
                label: "prod".into(),
                host: "labeldst.example.com".into(),
                port: 22,
                user: Some("eve".into()),
                auth_method: None,
                key_path: None,
                vault_account_id: None,
                proxy_command: None,
                proxy_jump: None,
            },
        ],
        tunnels: Vec::new(),
    };

    // --- Plan against machine B: all four statuses at once, on the rows
    // expected --------------------------------------------------------
    let plan_1 = plan_import(&bundle, &config_b, &[]);

    let same_id_row = plan_1
        .servers
        .iter()
        .find(|p| p.item.id == "srv-same-id")
        .unwrap();
    assert_eq!(same_id_row.status, ConflictStatus::SameId);
    assert_eq!(same_id_row.action, ItemAction::Replace);

    let label_collision_row = plan_1
        .servers
        .iter()
        .find(|p| p.item.id == "srv-label-src")
        .unwrap();
    assert_eq!(label_collision_row.status, ConflictStatus::LabelCollision);
    assert_eq!(label_collision_row.action, ItemAction::Add);

    let new_row = plan_1.servers.iter().find(|p| p.item.id == "srv-new").unwrap();
    assert_eq!(new_row.status, ConflictStatus::New);
    assert_eq!(new_row.action, ItemAction::Add);

    let broken_row = plan_1
        .tunnels
        .iter()
        .find(|p| p.item.id == tunnel_broken_id)
        .unwrap();
    assert_eq!(broken_row.status, ConflictStatus::ReferenceBroken);
    assert_eq!(broken_row.action, ItemAction::Skip);

    let ok_row = plan_1
        .tunnels
        .iter()
        .find(|p| p.item.id == tunnel_ok_id)
        .unwrap();
    assert_eq!(ok_row.status, ConflictStatus::New);
    assert_eq!(ok_row.action, ItemAction::Add);

    // --- Execute plan_1 unmodified, against a throwaway clone of B so the
    // overridden pass below still starts from B's true original state -----
    let key_dir_trial = tempfile::tempdir().unwrap();
    let mut config_b_trial = config_b.clone();
    let mut sink_trial = RecordingVaultSink::default();
    let outcome_trial = execute(
        plan_1,
        &mut config_b_trial,
        key_dir_trial.path(),
        Some(&mut sink_trial),
    )
    .unwrap();

    assert!(
        config_b_trial.find_tunnel(&tunnel_broken_id).is_none(),
        "the ReferenceBroken tunnel must not be imported"
    );
    assert!(config_b_trial.find_tunnel(&tunnel_ok_id).is_some());
    assert_eq!(
        outcome_trial.tunnels, 1,
        "only the resolvable tunnel counts as imported, not the skipped one"
    );

    let same_id_after_trial = config_b_trial.find_server("srv-same-id").unwrap();
    assert_eq!(
        same_id_after_trial.label, "same-id-host",
        "Replace must update the existing host in place with the bundle's values"
    );
    assert_eq!(same_id_after_trial.host, "same.example.com");
    assert_eq!(
        config_b_trial
            .ungrouped
            .iter()
            .filter(|s| s.id == "srv-same-id")
            .count(),
        1,
        "the SameId host must be updated in place, not duplicated"
    );
    assert_eq!(
        outcome_trial.servers, 3,
        "all three planned servers write on the unmodified pass (Replace + Add + Add)"
    );
    assert_eq!(outcome_trial.credentials, 1);

    // --- Re-plan against B's untouched original state, override the SameId
    // row to Skip and the LabelCollision row to Rename, then execute for
    // real -------------------------------------------------------------
    let mut plan_2 = plan_import(&bundle, &config_b, &[]);
    for row in &mut plan_2.servers {
        if row.item.id == "srv-same-id" {
            assert_eq!(row.status, ConflictStatus::SameId);
            row.action = ItemAction::Skip;
        }
        if row.item.id == "srv-label-src" {
            assert_eq!(row.status, ConflictStatus::LabelCollision);
            row.action = ItemAction::Rename("prod (2)".into());
        }
    }

    let key_dir_final = tempfile::tempdir().unwrap();
    let mut config_b_final = config_b.clone();
    let mut sink_final = RecordingVaultSink::default();
    let outcome_final = execute(
        plan_2,
        &mut config_b_final,
        key_dir_final.path(),
        Some(&mut sink_final),
    )
    .unwrap();

    // The skipped SameId row must leave B's local host exactly as it always
    // was — B's true original state, never mutated before this point since
    // the unmodified pass above ran against a separate clone.
    let same_id_final = config_b_final.find_server("srv-same-id").unwrap();
    assert_eq!(
        same_id_final.label, "old-label",
        "Skip must leave the local host's original values untouched"
    );
    assert_eq!(same_id_final.host, "old.example.com");
    assert_eq!(same_id_final.user.as_deref(), Some("old-user"));

    // The renamed LabelCollision row lands as a new entry carrying the
    // bundle's id and only the overridden label — everything else about it
    // is exactly what the bundle carried.
    let renamed = config_b_final
        .find_server("srv-label-src")
        .expect("the renamed host must be imported under the bundle's id");
    assert_eq!(renamed.label, "prod (2)");
    assert_eq!(
        renamed.host, "labelsrc.example.com",
        "a Rename changes only the label, not the rest of the item"
    );
    assert_eq!(renamed.user.as_deref(), Some("bob"));

    // Its label-collision partner, already local before the import, is left
    // completely alone.
    let untouched_label_dst = config_b_final.find_server("srv-label-dst").unwrap();
    assert_eq!(untouched_label_dst.label, "prod");
    assert_eq!(untouched_label_dst.host, "labeldst.example.com");

    // The New row and the resolvable tunnel still import normally; the
    // ReferenceBroken tunnel is still skipped on this pass too.
    assert!(config_b_final.find_server("srv-new").is_some());
    assert!(config_b_final.find_tunnel(&tunnel_ok_id).is_some());
    assert!(
        config_b_final.find_tunnel(&tunnel_broken_id).is_none(),
        "ReferenceBroken must still be a no-op even on the overridden pass"
    );

    assert_eq!(
        outcome_final.servers, 2,
        "Skip writes nothing; the Rename and the plain New Add each write one"
    );
    assert_eq!(outcome_final.tunnels, 1, "the ReferenceBroken tunnel is still excluded");
    assert_eq!(outcome_final.credentials, 1);
}
