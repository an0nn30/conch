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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemAction {
    Add,
    Replace,
}

#[derive(Debug, Clone)]
pub struct PlannedItem<T> {
    pub item: T,
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
/// resolve on this machine, so it is left out of `plan.tunnels` and a
/// message explaining why is appended to `plan.skipped` instead. A tunnel
/// with no `server_entry_id` (only a legacy `session_key`) has nothing to
/// validate here and is always kept.
pub fn plan(bundle: &ShareBundle, config: &SshConfig, existing_account_ids: &[Uuid]) -> ImportPlan {
    let mut skipped = Vec::new();

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
                    let action = action_for(config.find_server(&item.id).is_some());
                    PlannedItem { item, action }
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
            let action = action_for(config.find_server(&item.id).is_some());
            PlannedItem { item, action }
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
        if let Some(host_id) = &item.server_entry_id {
            let resolvable =
                bundle_host_ids.contains(host_id.as_str()) || config.find_server(host_id).is_some();
            if !resolvable {
                skipped.push(format!(
                    "Tunnel \"{}\" references a host that is not included in this bundle or your local configuration",
                    item.label
                ));
                continue;
            }
        }
        let action = action_for(config.tunnels.iter().any(|existing| existing.id == item.id));
        tunnels.push(PlannedItem {
            item: item.clone(),
            action,
        });
    }

    let accounts: Vec<PlannedItem<VaultAccount>> = bundle
        .vault
        .accounts
        .iter()
        .cloned()
        .map(|item| {
            let action = action_for(existing_account_ids.contains(&item.id));
            PlannedItem { item, action }
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

fn action_for(exists_locally: bool) -> ItemAction {
    if exists_locally {
        ItemAction::Replace
    } else {
        ItemAction::Add
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{BundleMetadata, BundledVault};

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

    #[test]
    fn new_uuid_adds_and_existing_uuid_replaces() {
        let (bundle, config) = fixture_with_one_overlapping_server();
        let p = plan(&bundle, &config, &[]);
        assert!(matches!(p.servers[0].action, ItemAction::Replace));
        assert!(matches!(p.servers[1].action, ItemAction::Add));
    }

    #[test]
    fn tunnel_with_unresolvable_host_is_skipped() {
        let (bundle, config) = fixture_tunnel_pointing_nowhere();
        let p = plan(&bundle, &config, &[]);
        assert!(p.tunnels.is_empty());
        assert!(p.skipped.iter().any(|s| s.contains("references a host")));
    }

    #[test]
    fn tunnel_resolving_to_an_existing_local_host_is_kept() {
        let (bundle, config) = fixture_tunnel_pointing_at_existing_local_host();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.tunnels.len(), 1);
    }
}
