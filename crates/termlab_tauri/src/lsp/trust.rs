//! Durable, fail-closed project policy for the future LSP manager actor.
//!
//! This module deliberately only remembers root choices and launch consent. It
//! does not discover projects, start adapters, or make any session-only choice
//! durable.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

const STORE_FILE: &str = "lsp-projects.toml";
const SCHEMA_VERSION: u32 = 1;

/// A remembered root choice for a workspace scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RootBinding {
    Root(PathBuf),
    Disabled,
}

/// Consent for launching an adapter in a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub(crate) enum TrustDecision {
    Trusted,
    Denied,
    Revoked,
}

/// A single persisted consent record. `workspace` is a scope, not a chosen root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustRecord {
    pub workspace: PathBuf,
    pub adapter_id: Option<String>,
    pub decision: TrustDecision,
    pub updated_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
}

/// Machine-readable category for a rejected store file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadWarningKind {
    ReadFailed,
    MalformedToml,
    UnsupportedSchema,
    InvalidRecord,
}

/// A non-fatal diagnostic emitted when loading falls back to an empty store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadWarning {
    pub kind: LoadWarningKind,
    pub message: String,
}

/// The outcome of loading the durable policy store.
#[derive(Debug)]
pub(crate) struct LoadResult {
    pub store: ProjectTrustStore,
    pub warning: Option<LoadWarning>,
}

/// Synchronous policy data owned by the future single LSP manager actor.
#[derive(Debug, Clone)]
pub(crate) struct ProjectTrustStore {
    config_dir: PathBuf,
    bindings: BTreeMap<PathBuf, RootBinding>,
    trust: Vec<TrustRecord>,
}

impl ProjectTrustStore {
    pub(crate) fn empty(config_dir: impl AsRef<Path>) -> Self {
        Self {
            config_dir: config_dir.as_ref().to_path_buf(),
            bindings: BTreeMap::new(),
            trust: Vec::new(),
        }
    }

    pub(crate) fn load(config_dir: impl AsRef<Path>) -> LoadResult {
        let empty = Self::empty(config_dir);
        let path = empty.store_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return LoadResult {
                    store: empty,
                    warning: None,
                };
            }
            Err(error) => {
                return LoadResult {
                    store: empty,
                    warning: Some(LoadWarning {
                        kind: LoadWarningKind::ReadFailed,
                        message: format!("Could not read {}: {error}", path.display()),
                    }),
                };
            }
        };

        let disk: DiskStore = match toml::from_str(&text) {
            Ok(disk) => disk,
            Err(error) => {
                return LoadResult {
                    store: empty,
                    warning: Some(LoadWarning {
                        kind: LoadWarningKind::MalformedToml,
                        message: format!("Could not parse {}: {error}", path.display()),
                    }),
                };
            }
        };
        if disk.schema_version != SCHEMA_VERSION {
            return LoadResult {
                store: empty,
                warning: Some(LoadWarning {
                    kind: LoadWarningKind::UnsupportedSchema,
                    message: format!(
                        "Unsupported LSP project store schema {} in {}",
                        disk.schema_version,
                        path.display()
                    ),
                }),
            };
        }

        match Self::from_disk(empty.config_dir.clone(), disk) {
            Ok(store) => LoadResult {
                store,
                warning: None,
            },
            Err(error) => LoadResult {
                store: empty,
                warning: Some(LoadWarning {
                    kind: LoadWarningKind::InvalidRecord,
                    message: format!(
                        "Invalid LSP project store record in {}: {error}",
                        path.display()
                    ),
                }),
            },
        }
    }

    /// Looks up consent for exactly the selected canonical root. An
    /// adapter-specific decision wins; otherwise an unscoped decision for that
    /// same root applies to every adapter.
    pub(crate) fn binding_for(&self, path: &Path) -> Option<RootBinding> {
        let path = canonical_path(path).ok()?;
        self.bindings
            .iter()
            .filter(|(scope, _)| path.starts_with(scope))
            .max_by_key(|(scope, _)| scope.components().count())
            .map(|(_, binding)| binding.clone())
    }

    pub(crate) fn trust_for(&self, path: &Path, adapter_id: Option<&str>) -> Option<&TrustRecord> {
        let path = canonical_path(path).ok()?;
        self.trust
            .iter()
            .find(|record| record.workspace == path && record.adapter_id.as_deref() == adapter_id)
            .or_else(|| {
                adapter_id.and_then(|_| {
                    self.trust
                        .iter()
                        .find(|record| record.workspace == path && record.adapter_id.is_none())
                })
            })
    }

    pub(crate) fn set_root_binding(
        &mut self,
        workspace: &Path,
        binding: RootBinding,
    ) -> io::Result<()> {
        let workspace = canonical_path(workspace)?;
        let binding = match binding {
            RootBinding::Root(root) => RootBinding::Root(canonical_path(&root)?),
            RootBinding::Disabled => RootBinding::Disabled,
        };
        self.bindings.insert(workspace, binding);
        Ok(())
    }

    pub(crate) fn set_trust(
        &mut self,
        workspace: &Path,
        adapter_id: Option<&str>,
        decision: TrustDecision,
        updated_at_ms: u64,
    ) -> io::Result<()> {
        let workspace = canonical_path(workspace)?;
        let adapter_id = adapter_id.map(str::to_owned);
        if let Some(record) = self
            .trust
            .iter_mut()
            .find(|record| record.workspace == workspace && record.adapter_id == adapter_id)
        {
            record.decision = decision;
            record.updated_at_ms = updated_at_ms;
            record.last_used_at_ms = None;
        } else {
            self.trust.push(TrustRecord {
                workspace,
                adapter_id,
                decision,
                updated_at_ms,
                last_used_at_ms: None,
            });
        }
        Ok(())
    }

    /// Records a root-wide revocation and removes every adapter-specific
    /// decision for that exact canonical root, so no more-specific stale
    /// `Trusted` record can outrank the revocation.
    pub(crate) fn revoke_all_at_root(
        &mut self,
        workspace: &Path,
        updated_at_ms: u64,
    ) -> io::Result<()> {
        let workspace = canonical_path(workspace)?;
        self.trust
            .retain(|record| record.workspace != workspace || record.adapter_id.is_none());
        self.set_trust(&workspace, None, TrustDecision::Revoked, updated_at_ms)
    }

    pub(crate) fn mark_trust_used(
        &mut self,
        workspace: &Path,
        adapter_id: Option<&str>,
        used_at_ms: u64,
    ) -> io::Result<()> {
        let workspace = canonical_path(workspace)?;
        let resolved = self
            .trust_for(&workspace, adapter_id)
            .map(|record| (record.workspace.clone(), record.adapter_id.clone()));
        if let Some((scope, adapter)) = resolved {
            if let Some(record) = self
                .trust
                .iter_mut()
                .find(|record| record.workspace == scope && record.adapter_id == adapter)
            {
                record.last_used_at_ms = Some(used_at_ms);
            }
        }
        Ok(())
    }

    pub(crate) fn records(&self) -> &[TrustRecord] {
        &self.trust
    }

    pub(crate) fn save(&self) -> io::Result<()> {
        fs::create_dir_all(&self.config_dir)?;
        let disk = DiskStore::from(self);
        let text = toml::to_string_pretty(&disk)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        termlab_core::config::atomic_write(&self.store_path(), text.as_bytes())
    }

    fn store_path(&self) -> PathBuf {
        self.config_dir.join(STORE_FILE)
    }

    fn from_disk(config_dir: PathBuf, disk: DiskStore) -> io::Result<Self> {
        let mut store = Self::empty(config_dir);
        for binding in disk.bindings {
            let workspace = canonical_stored_path(&binding.workspace)?;
            let binding = match (binding.kind, binding.root) {
                (DiskBindingKind::Root, Some(root)) => {
                    RootBinding::Root(canonical_stored_path(&root)?)
                }
                (DiskBindingKind::Disabled, None) => RootBinding::Disabled,
                (DiskBindingKind::Root, None) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "root binding is missing root",
                    ));
                }
                (DiskBindingKind::Disabled, Some(_)) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "disabled binding has a root",
                    ));
                }
            };
            if store.bindings.insert(workspace, binding).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate workspace binding",
                ));
            }
        }
        for trust in disk.trust {
            let workspace = canonical_stored_path(&trust.workspace)?;
            let record = TrustRecord {
                workspace,
                adapter_id: trust.adapter_id,
                decision: trust.decision,
                updated_at_ms: trust.updated_at_ms,
                last_used_at_ms: trust.last_used_at_ms,
            };
            if store.trust.iter().any(|existing| {
                existing.workspace == record.workspace && existing.adapter_id == record.adapter_id
            }) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate trust record",
                ));
            }
            store.trust.push(record);
        }
        Ok(store)
    }
}

fn canonical_path(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

fn canonical_stored_path(value: &str) -> io::Result<PathBuf> {
    let supplied = Path::new(value);
    if !supplied.is_absolute()
        || value.split(['/', '\\']).any(|component| component == ".")
        || supplied
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("path is not absolute and lexically normalized: {value}"),
        ));
    }
    Ok(supplied.to_path_buf())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskStore {
    schema_version: u32,
    #[serde(default)]
    bindings: Vec<DiskBinding>,
    #[serde(default)]
    trust: Vec<DiskTrust>,
}

impl From<&ProjectTrustStore> for DiskStore {
    fn from(store: &ProjectTrustStore) -> Self {
        let bindings = store
            .bindings
            .iter()
            .map(|(workspace, binding)| match binding {
                RootBinding::Root(root) => DiskBinding {
                    workspace: workspace.display().to_string(),
                    kind: DiskBindingKind::Root,
                    root: Some(root.display().to_string()),
                },
                RootBinding::Disabled => DiskBinding {
                    workspace: workspace.display().to_string(),
                    kind: DiskBindingKind::Disabled,
                    root: None,
                },
            })
            .collect();
        let trust = store
            .trust
            .iter()
            .map(|record| DiskTrust {
                workspace: record.workspace.display().to_string(),
                adapter_id: record.adapter_id.clone(),
                decision: record.decision,
                updated_at_ms: record.updated_at_ms,
                last_used_at_ms: record.last_used_at_ms,
            })
            .collect();
        Self {
            schema_version: SCHEMA_VERSION,
            bindings,
            trust,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskBinding {
    workspace: String,
    kind: DiskBindingKind,
    #[serde(default)]
    root: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DiskBindingKind {
    Root,
    Disabled,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskTrust {
    workspace: String,
    #[serde(default)]
    adapter_id: Option<String>,
    decision: TrustDecision,
    updated_at_ms: u64,
    #[serde(default)]
    last_used_at_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::{LoadWarningKind, ProjectTrustStore, RootBinding, TrustDecision};

    fn project(temp: &TempDir, relative: &str) -> PathBuf {
        let path = temp.path().join(relative);
        fs::create_dir_all(&path).expect("create project directory");
        fs::canonicalize(path).expect("canonical project directory")
    }

    fn store(temp: &TempDir) -> ProjectTrustStore {
        ProjectTrustStore::empty(temp.path())
    }

    #[test]
    fn nested_binding_wins_over_parent_workspace() {
        // Removing path-component precedence would make this select the parent.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let api = project(&temp, "repo/crates/api");
        let file = project(&temp, "repo/crates/api/src").join("lib.rs");
        fs::write(&file, "").expect("create source file");

        let mut store = store(&temp);
        store
            .set_root_binding(&repo, RootBinding::Root(repo.clone()))
            .unwrap();
        store
            .set_root_binding(&api, RootBinding::Root(api.clone()))
            .unwrap();

        assert_eq!(store.binding_for(&file), Some(RootBinding::Root(api)));
    }

    #[test]
    fn binding_prefixes_do_not_match_partial_path_components() {
        // Replacing component-aware matching with starts_with would select api here.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let api = project(&temp, "repo/api");
        let apix_file = project(&temp, "repo/apix/src").join("lib.rs");
        fs::write(&apix_file, "").expect("create source file");

        let mut store = store(&temp);
        store
            .set_root_binding(&repo, RootBinding::Root(repo.clone()))
            .unwrap();
        store
            .set_root_binding(&api, RootBinding::Root(api.clone()))
            .unwrap();

        assert_eq!(store.binding_for(&apix_file), Some(RootBinding::Root(repo)));
    }

    #[test]
    fn disabled_binding_round_trips_without_affecting_siblings() {
        // Collapsing disabled into an absent binding would lose the remembered choice.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let web = project(&temp, "repo/web");
        let api = project(&temp, "repo/api");
        let mut store = store(&temp);
        store.set_root_binding(&web, RootBinding::Disabled).unwrap();
        store
            .set_root_binding(&api, RootBinding::Root(repo.clone()))
            .unwrap();
        store.save().expect("save bindings");

        let loaded = ProjectTrustStore::load(temp.path()).store;
        assert_eq!(loaded.binding_for(&web), Some(RootBinding::Disabled));
        assert_eq!(loaded.binding_for(&api), Some(RootBinding::Root(repo)));
    }

    #[test]
    fn trust_is_separate_from_root_binding_and_revocation_is_remembered() {
        // Sharing trust with root selection would make revocation alter the root choice.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let mut store = store(&temp);
        store
            .set_root_binding(&repo, RootBinding::Root(repo.clone()))
            .unwrap();

        assert_eq!(store.trust_for(&repo, Some("rust-analyzer")), None);
        store
            .set_trust(&repo, Some("rust-analyzer"), TrustDecision::Denied, 10)
            .unwrap();
        store
            .set_trust(&repo, Some("rust-analyzer"), TrustDecision::Revoked, 20)
            .unwrap();

        assert_eq!(
            store.binding_for(&repo),
            Some(RootBinding::Root(repo.clone()))
        );
        assert_eq!(
            store
                .trust_for(&repo, Some("rust-analyzer"))
                .unwrap()
                .decision,
            TrustDecision::Revoked
        );
    }

    #[test]
    fn adapter_specific_trust_overrides_generic_and_other_adapters_fall_back() {
        // Ignoring adapter scope would return trusted for rust-analyzer too.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let mut store = store(&temp);
        store
            .set_trust(&repo, None, TrustDecision::Trusted, 10)
            .unwrap();
        store
            .set_trust(&repo, Some("rust-analyzer"), TrustDecision::Denied, 11)
            .unwrap();

        assert_eq!(
            store
                .trust_for(&repo, Some("rust-analyzer"))
                .unwrap()
                .decision,
            TrustDecision::Denied
        );
        assert_eq!(
            store.trust_for(&repo, Some("pyright")).unwrap().decision,
            TrustDecision::Trusted
        );
    }

    #[test]
    fn parent_trust_does_not_authorize_or_record_use_for_a_nested_selected_root() {
        // Prefix lookup would incorrectly reuse the parent consent for api.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let api = project(&temp, "repo/crates/api");
        let mut store = store(&temp);
        store
            .set_root_binding(&repo, RootBinding::Root(repo.clone()))
            .unwrap();
        store
            .set_root_binding(&api, RootBinding::Root(api.clone()))
            .unwrap();
        store
            .set_trust(&repo, None, TrustDecision::Trusted, 10)
            .unwrap();

        assert_eq!(
            store.binding_for(&api),
            Some(RootBinding::Root(api.clone()))
        );
        assert_eq!(store.trust_for(&api, Some("rust-analyzer")), None);
        store
            .mark_trust_used(&api, Some("rust-analyzer"), 20)
            .unwrap();
        assert_eq!(store.trust_for(&repo, None).unwrap().last_used_at_ms, None);
    }

    #[test]
    fn trust_usage_updates_last_used_without_changing_decision_timestamp() {
        // Updating the decision timestamp during use would blur two distinct events.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let mut store = store(&temp);
        store
            .set_trust(&repo, None, TrustDecision::Trusted, 10)
            .unwrap();
        store.mark_trust_used(&repo, None, 20).unwrap();

        let record = store.trust_for(&repo, None).unwrap();
        assert_eq!(record.decision, TrustDecision::Trusted);
        assert_eq!(record.updated_at_ms, 10);
        assert_eq!(record.last_used_at_ms, Some(20));
    }

    #[test]
    fn missing_store_is_empty_and_untrusted() {
        // Treating a missing file as trusted would start servers without consent.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let loaded = ProjectTrustStore::load(temp.path());

        assert_eq!(loaded.warning, None);
        assert_eq!(loaded.store.binding_for(&repo), None);
        assert_eq!(loaded.store.trust_for(&repo, None), None);
    }

    #[test]
    fn hand_authored_store_keeps_bindings_and_trust_as_separate_records() {
        // Merging the two tables would make a revoked adapter change root selection.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let api = project(&temp, "repo/api");
        let disabled = project(&temp, "repo/disabled");
        let api_file = project(&temp, "repo/api/src").join("lib.rs");
        fs::write(&api_file, "").expect("create source file");
        let disabled_file = project(&temp, "repo/disabled/src").join("lib.rs");
        fs::write(&disabled_file, "").expect("create source file");

        let store_toml = format!(
            r#"schema_version = 1

[[bindings]]
workspace = "{}"
kind = "root"
root = "{}"

[[bindings]]
workspace = "{}"
kind = "disabled"

[[trust]]
workspace = "{}"
adapter_id = "rust-analyzer"
decision = "revoked"
updated_at_ms = 7
last_used_at_ms = 9
"#,
            api.display(),
            repo.display(),
            disabled.display(),
            api.display(),
        );
        fs::write(temp.path().join("lsp-projects.toml"), store_toml)
            .expect("write hand-authored store");

        let loaded = ProjectTrustStore::load(temp.path());
        assert_eq!(loaded.warning, None);
        assert_eq!(
            loaded.store.binding_for(&api_file),
            Some(RootBinding::Root(repo))
        );
        assert_eq!(
            loaded.store.binding_for(&disabled_file),
            Some(RootBinding::Disabled)
        );
        let trust = loaded.store.trust_for(&api, Some("rust-analyzer")).unwrap();
        assert_eq!(trust.decision, TrustDecision::Revoked);
        assert_eq!(trust.updated_at_ms, 7);
        assert_eq!(trust.last_used_at_ms, Some(9));
    }

    #[test]
    fn reload_keeps_unmounted_project_records_without_authorizing_a_different_root() {
        // Canonicalizing disk records would discard every decision with one missing project.
        let temp = TempDir::new().expect("temp directory");
        let mounted = project(&temp, "mounted");
        let unmounted = project(&temp, "unmounted");
        let mut store = store(&temp);
        store
            .set_trust(&mounted, None, TrustDecision::Trusted, 10)
            .unwrap();
        store
            .set_trust(&unmounted, None, TrustDecision::Denied, 11)
            .unwrap();
        store.save().expect("persist two project decisions");
        fs::remove_dir_all(&unmounted).expect("unmount one project");

        let loaded = ProjectTrustStore::load(temp.path());
        assert_eq!(loaded.warning, None);
        assert_eq!(loaded.store.trust.len(), 2);
        assert!(
            loaded
                .store
                .trust
                .iter()
                .any(|record| record.workspace == unmounted)
        );
        assert_eq!(
            loaded
                .store
                .trust_for(&mounted, Some("rust-analyzer"))
                .unwrap()
                .decision,
            TrustDecision::Trusted
        );
    }

    #[test]
    fn relative_or_parent_traversal_paths_fail_closed_on_load() {
        // Accepting lexical aliases would bypass the canonical-path persistence invariant.
        let temp = TempDir::new().expect("temp directory");
        let project = project(&temp, "repo");
        for workspace in [
            "repo".to_owned(),
            format!("{}/repo/../repo", temp.path().display()),
        ] {
            fs::write(
                temp.path().join("lsp-projects.toml"),
                format!(
                    "schema_version = 1\n[[trust]]\nworkspace = \"{workspace}\"\ndecision = \"trusted\"\nupdated_at_ms = 1\n"
                ),
            )
            .expect("write invalid path record");

            let loaded = ProjectTrustStore::load(temp.path());
            assert_eq!(loaded.warning.unwrap().kind, LoadWarningKind::InvalidRecord);
            assert_eq!(loaded.store.trust_for(&project, None), None);
        }
    }

    #[test]
    fn malformed_store_fails_closed_with_a_structured_warning() {
        // Reusing records after a parse error could silently trust stale data.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        fs::write(temp.path().join("lsp-projects.toml"), "schema_version = [")
            .expect("write malformed TOML");

        let loaded = ProjectTrustStore::load(temp.path());
        assert_eq!(loaded.warning.unwrap().kind, LoadWarningKind::MalformedToml);
        assert_eq!(loaded.store.binding_for(&repo), None);
        assert_eq!(loaded.store.trust_for(&repo, None), None);
    }

    #[test]
    fn unsupported_schema_fails_closed_with_a_structured_warning() {
        // Accepting an unknown schema risks misreading its trust semantics.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        fs::write(
            temp.path().join("lsp-projects.toml"),
            "schema_version = 999\n[[trust]]\nworkspace = \"/unsafe\"\ndecision = \"trusted\"\nupdated_at_ms = 1\n",
        )
        .expect("write unsupported store");

        let loaded = ProjectTrustStore::load(temp.path());
        assert_eq!(
            loaded.warning.unwrap().kind,
            LoadWarningKind::UnsupportedSchema
        );
        assert_eq!(loaded.store.binding_for(&repo), None);
        assert_eq!(loaded.store.trust_for(&repo, None), None);
    }

    #[test]
    fn persisted_store_reloads_from_an_atomic_write() {
        // Replacing atomic persistence with an incomplete write would fail a fresh reload.
        let temp = TempDir::new().expect("temp directory");
        let repo = project(&temp, "repo");
        let mut store = store(&temp);
        store
            .set_root_binding(&repo, RootBinding::Root(repo.clone()))
            .unwrap();
        store
            .set_trust(&repo, None, TrustDecision::Trusted, 10)
            .unwrap();
        store.mark_trust_used(&repo, None, 20).unwrap();
        store.save().expect("save store atomically");

        let loaded = ProjectTrustStore::load(temp.path());
        assert_eq!(loaded.warning, None);
        assert_eq!(
            loaded.store.binding_for(&repo),
            Some(RootBinding::Root(repo.clone()))
        );
        let trust = loaded.store.trust_for(&repo, None).unwrap();
        assert_eq!(trust.decision, TrustDecision::Trusted);
        assert_eq!(trust.updated_at_ms, 10);
        assert_eq!(trust.last_used_at_ms, Some(20));
        assert!(Path::new(&temp.path().join("lsp-projects.toml")).is_file());
    }
}
