//! Pure import planning: decide, for each item in a decoded [`ShareBundle`],
//! whether it is new to this machine or replaces something that is already
//! here.
//!
//! This module never touches the filesystem, the vault, or `config` — it
//! only reads `config` to decide `Add` vs `Replace`. All mutation happens in
//! [`crate::import_executor`], the sole unit allowed to touch user state.
//! Keeping the decision pure here is what makes "import the same bundle
//! twice" a testable, deterministic property instead of something that only
//! shows up as a bug report after a duplicate host appears in someone's
//! sidebar.

use std::collections::HashSet;

use uuid::Uuid;

use termlab_remote::config::{SavedTunnel, ServerEntry, SshConfig};
use termlab_vault::model::VaultAccount;

use crate::bundle::{BundledKey, ShareBundle};

// No serde/ts-rs derives: termlab_share is a pure domain crate with neither
// dependency, and the Tauri layer maps this to a string for the frontend
// (ImportPreviewRow::status in Task 3). Keep it that way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStatus {
    New,
    SameId,
    LabelCollision,
    ReferenceBroken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemAction {
    Add,
    Replace,
    Skip,
    Rename(String),
}

#[derive(Debug, Clone)]
pub struct PlannedItem<T> {
    pub item: T,
    pub status: ConflictStatus,
    pub action: ItemAction,
}

/// A bundled folder, planned entry-by-entry rather than as one whole unit.
///
/// The export planner deliberately narrows a folder to only the *selected*
/// entries while keeping the folder's original id (`export_planner.rs`'s
/// stage 1), so a folder container in a bundle is never the recipient's
/// complete folder — it is whatever subset the sender chose to share. If
/// import replaced the local folder's `entries` wholesale with the bundle's,
/// any locally-owned entry that was never part of the export would be
/// deleted with no warning (2026-08-16 review finding C1). Planning entries
/// individually, and having the executor upsert them into the existing
/// folder rather than replace it, is what keeps a partial re-export from
/// being destructive.
#[derive(Debug, Clone)]
pub struct PlannedFolder {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub entries: Vec<PlannedItem<ServerEntry>>,
}

pub struct ImportPlan {
    pub folders: Vec<PlannedFolder>,
    pub servers: Vec<PlannedItem<ServerEntry>>,
    pub tunnels: Vec<PlannedItem<SavedTunnel>>,
    pub accounts: Vec<PlannedItem<VaultAccount>>,
    pub keys: Vec<BundledKey>,
    pub skipped: Vec<String>,
}

/// Resolve a decoded `bundle` against the local `config` into an
/// [`ImportPlan`].
///
/// An item whose id already exists locally is planned as `Replace` (so
/// re-importing the same bundle overwrites in place rather than
/// duplicating); everything else is `Add`. Vault account existence is
/// decided from `existing_account_ids` rather than a live vault lookup, so
/// this function stays pure — the caller (Task 6) supplies the id list from
/// whatever vault store it has open.
///
/// A tunnel whose `server_entry_id` names a host that is neither being
/// imported (as part of this same bundle) nor already known locally cannot
/// resolve on this machine — it is kept in `plan.tunnels` with status
/// `ConflictStatus::ReferenceBroken` and action `ItemAction::Skip`, rather
/// than being dropped, so the row still shows up for the user instead of
/// silently disappearing. A tunnel with no `server_entry_id` (only a legacy
/// `session_key`) has nothing to validate here and is always kept.
pub fn plan(bundle: &ShareBundle, config: &SshConfig, existing_account_ids: &[Uuid]) -> ImportPlan {
    let skipped = Vec::new();

    // Existence (and therefore Add-vs-Replace) for any server id — whether
    // the bundle carries it ungrouped or nested in a folder — is decided
    // against `config.find_server`, which scans *both* `ungrouped` and every
    // folder. Scanning only `ungrouped` here (as this used to) meant a
    // bundle server whose id already existed nested in a local folder was
    // always classified `Add`, so the executor pushed a second, duplicate
    // copy into `ungrouped` instead of updating the one already in the
    // folder (2026-08-16 review finding I2) — this is reachable in
    // practice because the export planner's tunnel auto-pull always deposits
    // a dependency host into `bundle.servers` even when it is folder-nested
    // on the sender.
    let folders: Vec<PlannedFolder> = bundle
        .folders
        .iter()
        .cloned()
        .map(|folder| {
            let entries = folder
                .entries
                .into_iter()
                .map(|item| {
                    let id_exists = config.find_server(&item.id).is_some();
                    let label_collides = config
                        .all_servers()
                        .any(|s| s.label == item.label && s.id != item.id);
                    let (status, action) = classify(id_exists, label_collides);
                    PlannedItem {
                        item,
                        status,
                        action,
                    }
                })
                .collect();
            PlannedFolder {
                id: folder.id,
                name: folder.name,
                expanded: folder.expanded,
                entries,
            }
        })
        .collect();

    let servers: Vec<PlannedItem<ServerEntry>> = bundle
        .servers
        .iter()
        .cloned()
        .map(|item| {
            let id_exists = config.find_server(&item.id).is_some();
            let label_collides = config
                .all_servers()
                .any(|s| s.label == item.label && s.id != item.id);
            let (status, action) = classify(id_exists, label_collides);
            PlannedItem {
                item,
                status,
                action,
            }
        })
        .collect();

    // Every host id this bundle carries with it, whether ungrouped or
    // nested in a folder — a tunnel referencing one of these will resolve
    // once import applies the folders/servers stages, even though the host
    // is not in `config` yet.
    let bundle_host_ids: HashSet<&str> = bundle
        .servers
        .iter()
        .map(|s| s.id.as_str())
        .chain(
            bundle
                .folders
                .iter()
                .flat_map(|f| f.entries.iter())
                .map(|s| s.id.as_str()),
        )
        .collect();

    let mut tunnels = Vec::new();
    for item in &bundle.tunnels {
        let unresolvable = matches!(&item.server_entry_id, Some(host_id)
            if !bundle_host_ids.contains(host_id.as_str())
                && config.find_server(host_id).is_none());
        let (status, action) = if unresolvable {
            (ConflictStatus::ReferenceBroken, ItemAction::Skip)
        } else {
            let label_collides = config
                .tunnels
                .iter()
                .any(|t| t.label == item.label && t.id != item.id);
            classify(
                config.tunnels.iter().any(|t| t.id == item.id),
                label_collides,
            )
        };
        tunnels.push(PlannedItem {
            item: item.clone(),
            status,
            action,
        });
    }

    let accounts: Vec<PlannedItem<VaultAccount>> = bundle
        .vault
        .accounts
        .iter()
        .cloned()
        .map(|item| {
            let (status, action) = classify(existing_account_ids.contains(&item.id), false);
            PlannedItem {
                item,
                status,
                action,
            }
        })
        .collect();

    ImportPlan {
        folders,
        servers,
        tunnels,
        accounts,
        keys: bundle.vault.keys.clone(),
        skipped,
    }
}

/// Status and default action for an item, given whether its id already exists
/// locally and whether its label collides with a *different* local item.
///
/// An id match outranks a label collision: the item IS the local one, whatever
/// it is currently called.
fn classify(id_exists: bool, label_collides: bool) -> (ConflictStatus, ItemAction) {
    if id_exists {
        (ConflictStatus::SameId, ItemAction::Replace)
    } else if label_collides {
        (ConflictStatus::LabelCollision, ItemAction::Add)
    } else {
        (ConflictStatus::New, ItemAction::Add)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{BundleMetadata, BundledVault};
    use termlab_remote::config::ServerFolder;
    use termlab_vault::model::AuthMethod;

    fn sample_metadata() -> BundleMetadata {
        BundleMetadata {
            created_at: chrono::Utc::now(),
            source_host: "test-host".into(),
            termlab_version: "0.0.0-test".into(),
            includes_credentials: false,
        }
    }

    fn make_server(id: &str, host: &str) -> ServerEntry {
        ServerEntry {
            id: id.into(),
            label: id.into(),
            host: host.into(),
            port: 22,
            user: None,
            auth_method: None,
            key_path: None,
            vault_account_id: None,
            proxy_command: None,
            proxy_jump: None,
        }
    }

    fn empty_bundle() -> ShareBundle {
        ShareBundle {
            schema_version: crate::SCHEMA_VERSION,
            metadata: sample_metadata(),
            folders: Vec::new(),
            servers: Vec::new(),
            tunnels: Vec::new(),
            vault: BundledVault::default(),
        }
    }

    /// One server id ("existing") already present locally, one ("new-host")
    /// that is not — the minimal shape needed to exercise both branches of
    /// `Add` vs `Replace` in one plan.
    fn fixture_with_one_overlapping_server() -> (ShareBundle, SshConfig) {
        let mut bundle = empty_bundle();
        bundle.servers = vec![
            make_server("existing", "overlap.example.com"),
            make_server("new-host", "fresh.example.com"),
        ];

        let mut config = SshConfig::default();
        config.add_server(make_server("existing", "overlap.example.com"));

        (bundle, config)
    }

    /// A tunnel whose `server_entry_id` ("ghost") names a host that is
    /// present neither in the bundle nor in the local config.
    fn fixture_tunnel_pointing_nowhere() -> (ShareBundle, SshConfig) {
        let mut bundle = empty_bundle();
        bundle.tunnels = vec![SavedTunnel {
            id: Uuid::new_v4(),
            label: "dangling".into(),
            session_key: "u@ghost.example.com:22".into(),
            server_entry_id: Some("ghost".into()),
            local_port: 5432,
            remote_host: "db.internal".into(),
            remote_port: 5432,
            auto_start: false,
        }];
        let config = SshConfig::default();
        (bundle, config)
    }

    /// A tunnel whose `server_entry_id` ("local-only") names a host that
    /// exists only in the local config, not in the bundle — the recipient's
    /// own pre-existing server, not something this import is bringing in.
    fn fixture_tunnel_pointing_at_existing_local_host() -> (ShareBundle, SshConfig) {
        let mut bundle = empty_bundle();
        bundle.tunnels = vec![SavedTunnel {
            id: Uuid::new_v4(),
            label: "to-local".into(),
            session_key: "u@local.example.com:22".into(),
            server_entry_id: Some("local-only".into()),
            local_port: 5432,
            remote_host: "db.internal".into(),
            remote_port: 5432,
            auto_start: false,
        }];

        let mut config = SshConfig::default();
        config.add_server(make_server("local-only", "local.example.com"));

        (bundle, config)
    }

    /// A server whose id is not present anywhere in `config` — the plain
    /// "New" case.
    fn fixture_new_server() -> (ShareBundle, SshConfig) {
        let mut bundle = empty_bundle();
        bundle.servers = vec![make_server("new-host", "fresh.example.com")];
        let config = SshConfig::default();
        (bundle, config)
    }

    /// A server whose id already exists in `config.ungrouped`.
    fn fixture_server_id_exists_ungrouped() -> (ShareBundle, SshConfig) {
        let mut bundle = empty_bundle();
        bundle.servers = vec![make_server("existing", "overlap.example.com")];

        let mut config = SshConfig::default();
        config.add_server(make_server("existing", "overlap.example.com"));

        (bundle, config)
    }

    /// A server whose id already exists, but nested inside a local folder
    /// rather than in `ungrouped` — existence must be decided with
    /// `config.find_server`, which scans folders too (the I2 trap).
    fn fixture_server_id_exists_in_folder() -> (ShareBundle, SshConfig) {
        let mut bundle = empty_bundle();
        bundle.servers = vec![make_server("existing", "overlap.example.com")];

        let mut config = SshConfig::default();
        config.folders.push(ServerFolder {
            id: "folder-1".into(),
            name: "Folder".into(),
            expanded: true,
            entries: vec![make_server("existing", "overlap.example.com")],
        });

        (bundle, config)
    }

    /// A bundle server with a fresh id whose label collides with a
    /// *different* local server's label.
    fn fixture_label_collides_different_id() -> (ShareBundle, SshConfig) {
        let mut bundle = empty_bundle();
        let mut incoming = make_server("bundle-id", "bundle.example.com");
        incoming.label = "prod".into();
        bundle.servers = vec![incoming];

        let mut config = SshConfig::default();
        let mut local = make_server("local-id", "local.example.com");
        local.label = "prod".into();
        config.add_server(local);

        (bundle, config)
    }

    /// A bundle server whose id matches a local server AND whose label
    /// matches a *different* local server's label. The id match must win.
    fn fixture_id_match_and_label_collision() -> (ShareBundle, SshConfig) {
        let mut bundle = empty_bundle();
        let mut incoming = make_server("shared-id", "bundle.example.com");
        incoming.label = "renamed-label".into();
        bundle.servers = vec![incoming];

        let mut config = SshConfig::default();
        let mut same_id = make_server("shared-id", "old.example.com");
        same_id.label = "old-name".into();
        config.add_server(same_id);
        let mut different_id_same_label = make_server("other-id", "other.example.com");
        different_id_same_label.label = "renamed-label".into();
        config.add_server(different_id_same_label);

        (bundle, config)
    }

    /// A tunnel whose `server_entry_id` names a host carried in this same
    /// bundle (not yet in local config) — it resolves once the host is
    /// imported alongside it.
    fn fixture_tunnel_host_in_bundle() -> (ShareBundle, SshConfig) {
        let mut bundle = empty_bundle();
        bundle.servers = vec![make_server("bundle-host", "bundlehost.example.com")];
        bundle.tunnels = vec![SavedTunnel {
            id: Uuid::new_v4(),
            label: "to-bundle-host".into(),
            session_key: "u@bundlehost.example.com:22".into(),
            server_entry_id: Some("bundle-host".into()),
            local_port: 5432,
            remote_host: "db.internal".into(),
            remote_port: 5432,
            auto_start: false,
        }];
        let config = SshConfig::default();
        (bundle, config)
    }

    /// A bundled vault account whose id already exists locally.
    fn fixture_account_exists() -> (ShareBundle, SshConfig, Uuid) {
        let account_id = Uuid::new_v4();
        let mut bundle = empty_bundle();
        bundle.vault.accounts = vec![VaultAccount {
            id: account_id,
            display_name: "Prod DB".into(),
            username: "svc".into(),
            auth: AuthMethod::Password("x".into()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }];
        let config = SshConfig::default();
        (bundle, config, account_id)
    }

    #[test]
    fn new_uuid_adds_and_existing_uuid_replaces() {
        let (bundle, config) = fixture_with_one_overlapping_server();
        let p = plan(&bundle, &config, &[]);
        assert!(matches!(p.servers[0].action, ItemAction::Replace));
        assert!(matches!(p.servers[1].action, ItemAction::Add));
    }

    // This module used to have `tunnel_with_unresolvable_host_is_skipped`
    // here, asserting the tunnel vanished into `p.tunnels.is_empty()` and a
    // message landed in `p.skipped`. That's the exact behaviour this task
    // retires: the row now survives in `p.tunnels` as `ReferenceBroken`/
    // `Skip` instead of being dropped. It has been removed rather than
    // merely updated because its replacement assertions would have been a
    // verbatim duplicate of `an_unresolvable_tunnel_is_kept_as_a_reference_broken_row`
    // below (also exercising `fixture_tunnel_pointing_nowhere`), which
    // already provides the coverage.

    #[test]
    fn tunnel_resolving_to_an_existing_local_host_is_kept() {
        let (bundle, config) = fixture_tunnel_pointing_at_existing_local_host();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.tunnels.len(), 1);
    }

    #[test]
    fn a_brand_new_server_is_new_and_defaults_to_add() {
        let (bundle, config) = fixture_new_server();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::New);
        assert_eq!(p.servers[0].action, ItemAction::Add);
    }

    #[test]
    fn an_existing_id_is_same_id_and_defaults_to_replace() {
        let (bundle, config) = fixture_server_id_exists_ungrouped();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::SameId);
        assert_eq!(p.servers[0].action, ItemAction::Replace);
    }

    #[test]
    fn an_existing_folder_nested_id_is_also_same_id() {
        // The pre-existing I2 trap: existence must be decided with
        // config.find_server, which scans folders, not just `ungrouped`.
        let (bundle, config) = fixture_server_id_exists_in_folder();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::SameId);
    }

    #[test]
    fn a_colliding_label_with_a_different_id_is_label_collision_and_defaults_to_add() {
        let (bundle, config) = fixture_label_collides_different_id();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::LabelCollision);
        assert_eq!(p.servers[0].action, ItemAction::Add);
    }

    #[test]
    fn an_id_match_outranks_a_label_collision() {
        // Same id as a local host AND the same label as a different local
        // host: it is the local item, so SameId wins.
        let (bundle, config) = fixture_id_match_and_label_collision();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::SameId);
        assert_eq!(p.servers[0].action, ItemAction::Replace);
    }

    #[test]
    fn an_unresolvable_tunnel_is_kept_as_a_reference_broken_row() {
        // It used to be dropped into ImportPlan::skipped. A dropped row is how
        // a user ends up wondering where a tunnel went, so it now survives with
        // status ReferenceBroken and action Skip.
        let (bundle, config) = fixture_tunnel_pointing_nowhere();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.tunnels.len(), 1, "the row must survive, not be dropped");
        assert_eq!(p.tunnels[0].status, ConflictStatus::ReferenceBroken);
        assert_eq!(p.tunnels[0].action, ItemAction::Skip);
        assert!(p.skipped.is_empty(), "no longer reported via `skipped`");
    }

    #[test]
    fn a_tunnel_whose_host_is_in_the_bundle_resolves() {
        let (bundle, config) = fixture_tunnel_host_in_bundle();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.tunnels[0].status, ConflictStatus::New);
        assert_eq!(p.tunnels[0].action, ItemAction::Add);
    }

    #[test]
    fn an_existing_account_id_is_same_id() {
        let (bundle, config, account_id) = fixture_account_exists();
        let p = plan(&bundle, &config, &[account_id]);
        assert_eq!(p.accounts[0].status, ConflictStatus::SameId);
        assert_eq!(p.accounts[0].action, ItemAction::Replace);
    }
}
