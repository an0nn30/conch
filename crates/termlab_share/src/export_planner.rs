//! Pure export planning: turn a user's selection into a resolved
//! [`ShareBundle`], pulling in dependent hosts, copying vault credentials,
//! and embedding private key material.
//!
//! This module never touches the filesystem directly. All key material is
//! obtained through the [`KeyReader`] trait so the planner stays pure and
//! testable without a real key on disk; the caller (Task 4) supplies a
//! filesystem-backed implementation.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use base64::Engine;
use uuid::Uuid;

use termlab_remote::config::{SavedTunnel, ServerEntry, ServerFolder, SshConfig};
use termlab_vault::model::{AuthMethod, VaultAccount};

use crate::SCHEMA_VERSION;
use crate::bundle::{BundleMetadata, BundledKey, BundledVault, ShareBundle};

/// Reads private (and optionally public) key bytes for a key file path.
///
/// Implemented against the real filesystem by the caller; tests supply a
/// fake so `export_planner` never performs I/O itself.
pub trait KeyReader {
    /// Returns (private bytes, optional public bytes) for a key path.
    fn read_key(&self, path: &str) -> Result<(Vec<u8>, Option<Vec<u8>>), String>;
}

pub struct ExportRequest<'a> {
    pub config: &'a SshConfig,
    pub ssh_config_entries: &'a [ServerEntry],
    pub server_ids: Vec<String>,
    pub tunnel_ids: Vec<String>,
    /// Server ids the user explicitly declined to auto-pull in as a tunnel
    /// dependency (the export dialog's dependency prompt, "Export Without").
    /// Without this, stage 2 has no way to distinguish "the frontend didn't
    /// think this host was relevant" from "the user was shown this host and
    /// said no" — both look like a plain absence from `server_ids`. A
    /// declined id is skipped by stage 2's auto-pull (see
    /// `resolve_tunnel_host`) and a warning is recorded instead; it has no
    /// effect on any id already present in `server_ids` or otherwise pulled
    /// in some other way.
    pub declined_server_ids: Vec<String>,
    pub include_credentials: bool,
    pub accounts: Vec<VaultAccount>,
    pub source_host: String,
    pub termlab_version: String,
}

pub struct ExportPlan {
    pub bundle: ShareBundle,
    pub warnings: Vec<String>,
    pub auto_pulled: Vec<String>,
}

/// Resolve an [`ExportRequest`] into an [`ExportPlan`].
///
/// Stages, in order:
/// 1. Filter `config` to the selected server/tunnel ids and fold in any
///    selected `ssh_config_entries` (mirrors the legacy plaintext JSON
///    export the task-4 `share_export` command replaced).
/// 2. Resolve each selected tunnel's host, pulling in servers that were not
///    directly selected but are required by a tunnel — unless the user
///    declined that specific host (`declined_server_ids`), in which case it
///    is left out and a warning takes its place.
/// 3. If credentials are excluded, strip vault references and stop.
/// 4. Otherwise copy referenced vault accounts into the bundle.
/// 5. Embed private key material for each copied account's key-based auth.
/// 6. Do the same for servers using the legacy `key_path` field directly.
pub fn plan(req: ExportRequest<'_>, keys: &dyn KeyReader) -> ExportPlan {
    let mut warnings = Vec::new();
    let mut auto_pulled = Vec::new();

    // Stage 1: filter to the selection. We deliberately do not strip
    // `vault_account_id` here the way the legacy plain-JSON export used to
    // (that format was local-machine-only, so the reference was meaningless
    // off-machine). This bundle format instead copies the referenced vault
    // account into the bundle itself, so the link must survive into
    // `bundle.servers` for stage 4 to find it.
    let folders: Vec<ServerFolder> = req
        .config
        .folders
        .iter()
        .filter_map(|f| {
            let entries: Vec<ServerEntry> = f
                .entries
                .iter()
                .filter(|s| req.server_ids.contains(&s.id))
                .cloned()
                .collect();
            if entries.is_empty() {
                None
            } else {
                Some(ServerFolder {
                    id: f.id.clone(),
                    name: f.name.clone(),
                    expanded: f.expanded,
                    entries,
                })
            }
        })
        .collect();

    let mut servers: Vec<ServerEntry> = req
        .config
        .ungrouped
        .iter()
        .filter(|s| req.server_ids.contains(&s.id))
        .cloned()
        .collect();

    // Fold in selected ~/.ssh/config entries — the behaviour the legacy
    // plaintext JSON export used to have (now `share_commands::share_export`).
    for entry in req.ssh_config_entries {
        if req.server_ids.contains(&entry.id) {
            servers.push(entry.clone());
        }
    }

    let mut tunnels: Vec<SavedTunnel> = req
        .config
        .tunnels
        .iter()
        .filter(|t| req.tunnel_ids.contains(&t.id.to_string()))
        .cloned()
        .collect();

    // Stage 2: resolve each tunnel's host, pulling in dependencies — except
    // any the user explicitly declined via the export dialog's dependency
    // prompt (2026-08-16 ruling: "Export Without" must genuinely exclude the
    // server, not just leave it out of `server_ids` while still letting a
    // selected tunnel drag it back in).
    let declined: HashSet<&str> = req.declined_server_ids.iter().map(String::as_str).collect();
    for tunnel in &mut tunnels {
        resolve_tunnel_host(
            tunnel,
            &mut servers,
            req.config,
            req.ssh_config_entries,
            &declined,
            &mut auto_pulled,
            &mut warnings,
        );
    }

    let mut bundle = ShareBundle {
        schema_version: SCHEMA_VERSION,
        metadata: BundleMetadata {
            created_at: chrono::Utc::now(),
            source_host: req.source_host,
            termlab_version: req.termlab_version,
            includes_credentials: false,
        },
        folders,
        servers,
        tunnels,
        vault: BundledVault::default(),
    };

    // Stage 3: credentials excluded — strip vault references and stop.
    if !req.include_credentials {
        for server in &mut bundle.servers {
            server.vault_account_id = None;
        }
        bundle.metadata.includes_credentials = false;
        return ExportPlan {
            bundle,
            warnings,
            auto_pulled,
        };
    }

    // Stage 4: copy referenced vault accounts, de-duplicated by id.
    let mut seen: HashSet<Uuid> = HashSet::new();
    for server in &bundle.servers {
        let Some(account_id) = server.vault_account_id else {
            continue;
        };
        if !seen.insert(account_id) {
            continue;
        }
        match req.accounts.iter().find(|a| a.id == account_id) {
            Some(account) => bundle.vault.accounts.push(account.clone()),
            None => warnings.push(format!(
                "Vault account {account_id} referenced by server \"{}\" was not found",
                server.label
            )),
        }
    }

    // Stage 5: embed private key material for each copied account.
    for account in &mut bundle.vault.accounts {
        embed_key_for_account(account, keys, &mut bundle.vault.keys, &mut warnings);
    }

    // Stage 6: servers still using the legacy `key_path` field (no vault
    // account) get a synthesised account so their key travels with them too.
    for server in &mut bundle.servers {
        if server.vault_account_id.is_some() {
            continue;
        }
        let Some(key_path) = server.key_path.clone() else {
            continue;
        };
        let mut account = VaultAccount {
            id: Uuid::new_v4(),
            display_name: server.label.clone(),
            username: server.user.clone().unwrap_or_default(),
            auth: AuthMethod::Key {
                path: PathBuf::from(key_path),
                passphrase: None,
            },
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        embed_key_for_account(&mut account, keys, &mut bundle.vault.keys, &mut warnings);
        // Link the server to its new account the same way a vault-backed
        // server already does, so import can treat both cases uniformly.
        server.vault_account_id = Some(account.id);
        bundle.vault.accounts.push(account);
    }

    bundle.metadata.includes_credentials = !bundle.vault.is_empty();

    ExportPlan {
        bundle,
        warnings,
        auto_pulled,
    }
}

/// Resolve a single tunnel's host and, if necessary, pull it into
/// `bundle_servers`. Mutates `tunnel.server_entry_id` only when the host was
/// resolved from `ssh_config_entries`, per the spec: import needs a real,
/// bundled server id to rewrite against, and an ssh_config alias never has
/// one until we mint it here.
///
/// `declined` holds ids the user was shown (via the export dialog's
/// dependency prompt) and explicitly chose to leave out. A host in
/// `declined` is never auto-pulled — it is skipped and a warning is
/// recorded instead — but `declined` has no effect on a host that is
/// already present in `bundle_servers` some other way (e.g. the user
/// selected it directly): declining a *dependency* does not retract an
/// explicit selection.
fn resolve_tunnel_host(
    tunnel: &mut SavedTunnel,
    bundle_servers: &mut Vec<ServerEntry>,
    known: &SshConfig,
    ssh_config_entries: &[ServerEntry],
    declined: &HashSet<&str>,
    auto_pulled: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    // 1. server_entry_id first.
    if let Some(id) = tunnel.server_entry_id.clone() {
        if bundle_servers.iter().any(|s| s.id == id) {
            return;
        }
        if let Some(entry) = known.find_server(&id) {
            if declined.contains(entry.id.as_str()) {
                warnings.push(declined_warning(entry, tunnel));
                return;
            }
            pull_in(bundle_servers, entry.clone(), tunnel, auto_pulled);
            return;
        }
    }

    // 2. session_key against known servers, then 3. against ssh_config_entries.
    let Some((_, host, port)) = SavedTunnel::parse_session_key(&tunnel.session_key) else {
        warnings.push(unresolvable_warning(tunnel));
        return;
    };

    if let Some(entry) = known
        .all_servers()
        .find(|s| s.host == host && s.port == port)
    {
        if !bundle_servers.iter().any(|s| s.id == entry.id) {
            if declined.contains(entry.id.as_str()) {
                warnings.push(declined_warning(entry, tunnel));
                return;
            }
            pull_in(bundle_servers, entry.clone(), tunnel, auto_pulled);
        }
        return;
    }

    if let Some(entry) = ssh_config_entries
        .iter()
        .find(|s| s.host == host && s.port == port)
    {
        if !bundle_servers.iter().any(|s| s.id == entry.id) {
            if declined.contains(entry.id.as_str()) {
                warnings.push(declined_warning(entry, tunnel));
                return;
            }
            auto_pulled.push(pull_in_message(entry, tunnel));
            bundle_servers.push(entry.clone());
        }
        tunnel.server_entry_id = Some(entry.id.clone());
        return;
    }

    warnings.push(unresolvable_warning(tunnel));
}

fn pull_in(
    bundle_servers: &mut Vec<ServerEntry>,
    entry: ServerEntry,
    tunnel: &SavedTunnel,
    auto_pulled: &mut Vec<String>,
) {
    auto_pulled.push(pull_in_message(&entry, tunnel));
    bundle_servers.push(entry);
}

fn pull_in_message(entry: &ServerEntry, tunnel: &SavedTunnel) -> String {
    format!(
        "Included host \"{}\" ({}) required by tunnel \"{}\"",
        entry.label, entry.id, tunnel.label
    )
}

fn declined_warning(entry: &ServerEntry, tunnel: &SavedTunnel) -> String {
    format!(
        "Host \"{}\" was excluded from the export; tunnel \"{}\" will not resolve it on import",
        entry.label, tunnel.label
    )
}

fn unresolvable_warning(tunnel: &SavedTunnel) -> String {
    format!(
        "Could not resolve host for tunnel \"{}\" (session_key \"{}\")",
        tunnel.label, tunnel.session_key
    )
}

/// Embed private key material for `account`'s key-based auth, if any.
///
/// On success, pushes a `BundledKey` and rewrites the account's auth path to
/// the `termlab-bundle:<key-id>` marker Task 5's import executor looks for.
/// On failure, records the reader's error (a path, never key material or a
/// passphrase) as a warning and leaves the account's path untouched.
fn embed_key_for_account(
    account: &mut VaultAccount,
    keys: &dyn KeyReader,
    out_keys: &mut Vec<BundledKey>,
    warnings: &mut Vec<String>,
) {
    let path = match &account.auth {
        AuthMethod::Key { path, .. } => path.clone(),
        AuthMethod::KeyAndPassword { key_path, .. } => key_path.clone(),
        AuthMethod::Password(_) => return,
    };
    let path_str = path.to_string_lossy().to_string();

    match keys.read_key(&path_str) {
        Ok((material_bytes, public_bytes)) => {
            let passphrase = match &account.auth {
                AuthMethod::Key { passphrase, .. } => passphrase.clone(),
                AuthMethod::KeyAndPassword { passphrase, .. } => passphrase.clone(),
                AuthMethod::Password(_) => None,
            };
            let key_id = Uuid::new_v4();
            let comment = Path::new(&path_str)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| path_str.clone());

            out_keys.push(BundledKey {
                id: key_id,
                original_path: path_str,
                material: base64::engine::general_purpose::STANDARD.encode(&material_bytes),
                public_material: public_bytes
                    .map(|b| base64::engine::general_purpose::STANDARD.encode(&b)),
                passphrase,
                comment,
            });

            let marker = PathBuf::from(format!("termlab-bundle:{key_id}"));
            match &mut account.auth {
                AuthMethod::Key { path, .. } => *path = marker,
                AuthMethod::KeyAndPassword { key_path, .. } => *key_path = marker,
                AuthMethod::Password(_) => {}
            }
        }
        Err(e) => warnings.push(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeKeys(HashMap<String, Vec<u8>>);
    impl KeyReader for FakeKeys {
        fn read_key(&self, path: &str) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
            self.0
                .get(path)
                .map(|b| (b.clone(), None))
                .ok_or_else(|| format!("Key file not found: {path}"))
        }
    }

    /// One server "s1" (prod.example.com), one tunnel "t1" referencing it by
    /// both `server_entry_id` and `session_key`, and a matching vault
    /// account with a key-based auth pointing at a path callers select via
    /// `FakeKeys`. `tunnel_ids` entries equal to the literal "t1" are mapped
    /// to the fixture tunnel's real UUID string before being passed to
    /// `plan`, since `SavedTunnel::id` is a `Uuid`, not a friendly string.
    /// `declined_server_ids` entries equal to the literal "s1" are similarly
    /// passed through as-is (the fixture server's id is already the plain
    /// string "s1", unlike the tunnel's UUID).
    fn plan_with(
        server_ids: Vec<String>,
        tunnel_ids: Vec<String>,
        include_credentials: bool,
        keys: FakeKeys,
        declined_server_ids: Vec<String>,
    ) -> ExportPlan {
        let account_id = Uuid::new_v4();
        let tunnel_id = Uuid::new_v4();

        let server = ServerEntry {
            id: "s1".into(),
            label: "prod".into(),
            host: "prod.example.com".into(),
            port: 22,
            user: Some("u".into()),
            auth_method: None,
            key_path: None,
            vault_account_id: Some(account_id),
            proxy_command: None,
            proxy_jump: None,
        };

        let tunnel = SavedTunnel {
            id: tunnel_id,
            label: "t".into(),
            session_key: "u@prod.example.com:22".into(),
            server_entry_id: Some("s1".into()),
            local_port: 5432,
            remote_host: "db.internal".into(),
            remote_port: 5432,
            auto_start: false,
        };

        let config = SshConfig {
            folders: Vec::new(),
            ungrouped: vec![server],
            tunnels: vec![tunnel],
        };

        let account = VaultAccount {
            id: account_id,
            display_name: "prod account".into(),
            username: "u".into(),
            auth: AuthMethod::Key {
                path: "/home/u/.ssh/id_ed25519".into(),
                passphrase: None,
            },
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let mapped_tunnel_ids: Vec<String> = tunnel_ids
            .into_iter()
            .map(|id| {
                if id == "t1" {
                    tunnel_id.to_string()
                } else {
                    id
                }
            })
            .collect();

        let req = ExportRequest {
            config: &config,
            ssh_config_entries: &[],
            server_ids,
            tunnel_ids: mapped_tunnel_ids,
            declined_server_ids,
            include_credentials,
            accounts: vec![account],
            source_host: "test-host".into(),
            termlab_version: "0.0.0-test".into(),
        };

        plan(req, &keys)
    }

    /// A tunnel whose host ("bastion") exists only as a `~/.ssh/config`
    /// alias in `ssh_config_entries` — never in `config.ungrouped` or
    /// `config.folders` — and is referenced only via `session_key`.
    fn plan_alias_case() -> ExportPlan {
        let tunnel_id = Uuid::new_v4();

        let alias = ServerEntry {
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

        let tunnel = SavedTunnel {
            id: tunnel_id,
            label: "jump".into(),
            session_key: "user@bastion.example.com:22".into(),
            server_entry_id: None,
            local_port: 8080,
            remote_host: "internal.local".into(),
            remote_port: 80,
            auto_start: false,
        };

        let config = SshConfig {
            folders: Vec::new(),
            ungrouped: Vec::new(),
            tunnels: vec![tunnel],
        };
        let ssh_config_entries = vec![alias];

        let req = ExportRequest {
            config: &config,
            ssh_config_entries: &ssh_config_entries,
            server_ids: vec![],
            tunnel_ids: vec![tunnel_id.to_string()],
            declined_server_ids: vec![],
            include_credentials: false,
            accounts: vec![],
            source_host: "test-host".into(),
            termlab_version: "0.0.0-test".into(),
        };

        plan(req, &FakeKeys(HashMap::new()))
    }

    /// A tunnel whose `session_key` matches no known server and no
    /// `ssh_config_entries` alias.
    fn plan_unresolvable_tunnel() -> ExportPlan {
        let tunnel_id = Uuid::new_v4();

        let tunnel = SavedTunnel {
            id: tunnel_id,
            label: "ghost-tunnel".into(),
            session_key: "ghost@nowhere:22".into(),
            server_entry_id: None,
            local_port: 1,
            remote_host: "x".into(),
            remote_port: 1,
            auto_start: false,
        };

        let config = SshConfig {
            folders: Vec::new(),
            ungrouped: Vec::new(),
            tunnels: vec![tunnel],
        };

        let req = ExportRequest {
            config: &config,
            ssh_config_entries: &[],
            server_ids: vec![],
            tunnel_ids: vec![tunnel_id.to_string()],
            declined_server_ids: vec![],
            include_credentials: false,
            accounts: vec![],
            source_host: "test-host".into(),
            termlab_version: "0.0.0-test".into(),
        };

        plan(req, &FakeKeys(HashMap::new()))
    }

    #[test]
    fn tunnel_pulls_in_its_host_even_when_unselected() {
        // server "s1" is NOT in server_ids; the tunnel referencing it is;
        // nothing was declined, so the default (pull it in) applies.
        let plan = plan_with(
            vec![],
            vec!["t1".into()],
            false,
            FakeKeys(HashMap::new()),
            vec![],
        );
        assert_eq!(plan.bundle.servers.len(), 1);
        assert_eq!(plan.bundle.servers[0].id, "s1");
        assert!(plan.auto_pulled.iter().any(|s| s.contains("s1")));
    }

    /// 2026-08-16 ruling: "Export Without" in the dependency prompt must
    /// genuinely exclude the server — no host entry, no vault account, no
    /// key material — even though the tunnel that needs it is selected and
    /// credentials are included (so the key *would* have been embeddable
    /// had the host been pulled in).
    #[test]
    fn declined_tunnel_host_is_not_pulled_in_and_warns() {
        let mut files = HashMap::new();
        files.insert(
            "/home/u/.ssh/id_ed25519".to_string(),
            b"PRIVATE-KEY-BYTES".to_vec(),
        );
        // server "s1" is NOT in server_ids; the tunnel referencing it IS
        // selected; "s1" is explicitly declined.
        let plan = plan_with(
            vec![],
            vec!["t1".into()],
            true,
            FakeKeys(files),
            vec!["s1".into()],
        );
        assert!(
            plan.bundle.servers.is_empty(),
            "declined host must not be pulled in: {:?}",
            plan.bundle.servers
        );
        assert!(
            plan.bundle.vault.accounts.is_empty(),
            "declined host's vault account must not be copied into the bundle"
        );
        assert!(
            plan.bundle.vault.keys.is_empty(),
            "declined host's private key must not be embedded into the bundle"
        );
        assert!(
            plan.auto_pulled.is_empty(),
            "a declined host must not be recorded as auto-pulled"
        );
        assert!(
            plan.warnings.iter().any(|w| w.contains("prod")),
            "warnings should name the declined host \"prod\": {:?}",
            plan.warnings
        );
    }

    #[test]
    fn credentials_off_strips_vault_account_id() {
        let plan = plan_with(
            vec!["s1".into()],
            vec![],
            false,
            FakeKeys(HashMap::new()),
            vec![],
        );
        assert!(plan.bundle.servers[0].vault_account_id.is_none());
        assert!(plan.bundle.vault.is_empty());
        assert!(!plan.bundle.metadata.includes_credentials);
    }

    #[test]
    fn credentials_on_copies_account_and_embeds_key_material() {
        let mut files = HashMap::new();
        files.insert(
            "/home/u/.ssh/id_ed25519".to_string(),
            b"PRIVATE-KEY-BYTES".to_vec(),
        );
        let plan = plan_with(vec!["s1".into()], vec![], true, FakeKeys(files), vec![]);
        assert_eq!(plan.bundle.vault.accounts.len(), 1);
        assert_eq!(plan.bundle.vault.keys.len(), 1);
        // Material is base64 of the file's bytes.
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&plan.bundle.vault.keys[0].material)
            .unwrap();
        assert_eq!(decoded, b"PRIVATE-KEY-BYTES");
        // The account now points at the bundled key by id, not the old path.
        assert!(
            plan.bundle.vault.keys[0]
                .original_path
                .contains("id_ed25519")
        );
        assert!(plan.bundle.metadata.includes_credentials);
    }

    #[test]
    fn missing_key_file_warns_but_still_exports_the_host() {
        let plan = plan_with(
            vec!["s1".into()],
            vec![],
            true,
            FakeKeys(HashMap::new()),
            vec![],
        );
        assert_eq!(plan.bundle.servers.len(), 1, "host must still export");
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("Key file not found")),
            "warnings were: {:?}",
            plan.warnings
        );
    }

    #[test]
    fn tunnel_referencing_an_ssh_config_alias_exports_it_as_a_real_server() {
        // The alias lives only in ssh_config_entries, never in config.ungrouped.
        let plan = plan_alias_case();
        assert!(plan.bundle.servers.iter().any(|s| s.label == "bastion"));
        let tunnel = &plan.bundle.tunnels[0];
        let alias_id = plan
            .bundle
            .servers
            .iter()
            .find(|s| s.label == "bastion")
            .unwrap()
            .id
            .clone();
        assert_eq!(tunnel.server_entry_id.as_deref(), Some(alias_id.as_str()));
    }

    #[test]
    fn unresolvable_tunnel_host_warns_and_keeps_session_key() {
        let plan = plan_unresolvable_tunnel();
        assert_eq!(plan.bundle.tunnels.len(), 1);
        assert_eq!(plan.bundle.tunnels[0].session_key, "ghost@nowhere:22");
        assert!(plan.warnings.iter().any(|w| w.contains("ghost@nowhere")));
    }
}
