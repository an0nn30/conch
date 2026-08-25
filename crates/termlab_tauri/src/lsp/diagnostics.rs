#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{DiagnosticKey, DiagnosticStore};
    use crate::lsp::types::{
        Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, EditorLocation,
        EditorPosition, EditorRange,
    };

    // Catches a store mutation that appends a newer publication instead of replacing the URI.
    #[test]
    fn newer_publish_replaces_uri_diagnostics() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/a.ts"), Some(4), vec![diag("old")]);
        store.replace(key("ts", "/repo/a.ts"), Some(5), vec![diag("new")]);

        let snapshot = store.snapshot(None);
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].message, "new");
        assert_eq!(snapshot.revision, 2);
    }

    // Catches a key that omits the session identity and lets one server erase another's result.
    #[test]
    fn sessions_publish_to_independent_authoritative_entries() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/a.ts"), Some(4), vec![diag("typescript")]);
        store.replace(key("eslint", "/repo/a.ts"), Some(4), vec![diag("eslint")]);

        let messages: Vec<_> = store
            .snapshot(None)
            .items
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert_eq!(messages, vec!["eslint", "typescript"]);
    }

    // Catches an older document version clearing newer diagnostics after out-of-order delivery.
    #[test]
    fn stale_versioned_publish_is_rejected_without_changing_the_snapshot() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/a.ts"), Some(5), vec![diag("current")]);
        let changed = store.replace(key("ts", "/repo/a.ts"), Some(4), Vec::new());

        let snapshot = store.snapshot(None);
        assert!(!changed);
        assert_eq!(snapshot.items[0].message, "current");
        assert_eq!(snapshot.revision, 1);
    }

    // Catches equal-version re-publications being ignored even when the authoritative contents change.
    #[test]
    fn equal_versioned_publish_replaces_the_previous_contents() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/a.ts"), Some(5), vec![diag("old")]);
        let changed = store.replace(key("ts", "/repo/a.ts"), Some(5), vec![diag("corrected")]);

        assert!(changed);
        assert_eq!(store.snapshot(None).items[0].message, "corrected");
    }

    // Catches an empty diagnostic publish leaving stale rows in the Problems snapshot.
    #[test]
    fn empty_publish_clears_that_session_and_uri() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/a.ts"), Some(4), vec![diag("old")]);
        store.replace(key("ts", "/repo/a.ts"), Some(5), Vec::new());

        let snapshot = store.snapshot(None);
        assert!(snapshot.items.is_empty());
        assert_eq!(snapshot.counts.errors, 0);
    }

    // Catches a stopped session retaining its rows or deleting another session's rows.
    #[test]
    fn clearing_a_session_removes_only_that_sessions_diagnostics() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/a.ts"), Some(4), vec![diag("typescript")]);
        store.replace(key("rust", "/repo/a.rs"), Some(4), vec![diag("rust")]);
        let changed = store.clear_session("ts");

        let snapshot = store.snapshot(None);
        assert!(changed);
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].message, "rust");
    }

    // Catches aggregate counts derived from stale entries or an incomplete severity mapping.
    #[test]
    fn snapshot_reports_all_severity_totals() {
        let mut store = DiagnosticStore::default();
        store.replace(
            key("ts", "/repo/a.ts"),
            Some(1),
            vec![
                diag_with_severity("error", DiagnosticSeverity::Error),
                diag_with_severity("warning", DiagnosticSeverity::Warning),
                diag_with_severity("information", DiagnosticSeverity::Information),
                diag_with_severity("hint", DiagnosticSeverity::Hint),
            ],
        );

        let counts = store.snapshot(None).counts;
        assert_eq!(counts.errors, 1);
        assert_eq!(counts.warnings, 1);
        assert_eq!(counts.information, 1);
        assert_eq!(counts.hints, 1);
    }

    // Catches HashMap iteration leaking into the externally visible Problems order.
    #[test]
    fn snapshot_sorts_by_canonical_uri_then_range_and_stable_tie_breakers() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/z.ts"), Some(1), vec![diag_at("z", 0, 0)]);
        store.replace(
            key("ts", "/repo/a/../a.ts"),
            Some(1),
            vec![
                diag_at("later-column", 2, 3),
                diag_at("later-line", 3, 0),
                diag_at("first", 2, 1),
            ],
        );

        let snapshot = store.snapshot(None);
        let values: Vec<_> = snapshot
            .items
            .iter()
            .map(|diagnostic| (diagnostic.uri.as_str(), diagnostic.message.as_str()))
            .collect();
        assert_eq!(
            values,
            vec![
                ("file:///repo/a.ts", "first"),
                ("file:///repo/a.ts", "later-column"),
                ("file:///repo/a.ts", "later-line"),
                ("file:///repo/z.ts", "z"),
            ]
        );
    }

    // Catches string-prefix workspace filtering where /repo includes /repository.
    #[test]
    fn workspace_filter_uses_path_components_for_unavailable_historical_paths() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/src/a.ts"), Some(1), vec![diag("inside")]);
        store.replace(
            key("ts", "/repository/src/a.ts"),
            Some(1),
            vec![diag("prefix-only")],
        );
        store.replace(key("ts", "/repo-old/a.ts"), Some(1), vec![diag("outside")]);

        let messages: Vec<_> = store
            .snapshot(Some(Path::new("/repo")))
            .items
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert_eq!(messages, vec!["inside"]);
    }

    // Catches an invalid local workspace root broadening a scoped Problems snapshot to every project.
    #[test]
    fn invalid_workspace_root_returns_an_empty_scoped_snapshot() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/a.ts"), Some(1), vec![diag("inside")]);

        let snapshot = store.snapshot(Some(Path::new("relative-root")));
        assert!(snapshot.items.is_empty());
        assert_eq!(snapshot.counts.warnings, 0);
    }

    // Catches a missing descendant below a symlinked workspace alias retaining
    // the alias instead of its real canonical workspace identity.
    #[cfg(unix)]
    #[test]
    fn unavailable_descendant_through_workspace_symlink_has_one_key_and_is_scoped() {
        use std::fs;
        use std::os::unix::fs::symlink;

        use async_lsp::lsp_types::Url;
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let workspace = temp.path().join("real-workspace");
        fs::create_dir(&workspace).unwrap();
        let workspace_alias = temp.path().join("workspace-alias");
        symlink(&workspace, &workspace_alias).unwrap();
        let workspace_link = temp.path().join("repo-link");
        symlink(&workspace_alias, &workspace_link).unwrap();

        let unavailable_through_link = workspace_link.join("missing/../gone.ts");
        let unavailable_direct = workspace.join("gone.ts");
        let linked_key = DiagnosticKey::new(
            "ts",
            &Url::from_file_path(&unavailable_through_link)
                .unwrap()
                .to_string(),
        )
        .unwrap();
        let direct_key = DiagnosticKey::new(
            "ts",
            &Url::from_file_path(&unavailable_direct)
                .unwrap()
                .to_string(),
        )
        .unwrap();

        assert_eq!(linked_key, direct_key);

        let mut store = DiagnosticStore::default();
        store.replace(linked_key, Some(1), vec![diag("through-link")]);
        let messages: Vec<_> = store
            .snapshot(Some(&workspace))
            .items
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert_eq!(messages, vec!["through-link"]);
    }

    // Catches lexical fallback accepting paths below a regular file or a
    // broken symlink, where no real directory ancestor can contain the suffix.
    #[cfg(unix)]
    #[test]
    fn unavailable_descendants_without_a_resolvable_directory_fail_closed() {
        use std::fs;
        use std::os::unix::fs::symlink;

        use async_lsp::lsp_types::Url;
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let regular_file = temp.path().join("regular-file");
        fs::write(&regular_file, "not a directory").unwrap();
        let broken_link = temp.path().join("broken-link");
        symlink(temp.path().join("missing-target"), &broken_link).unwrap();

        for path in [regular_file.join("child.ts"), broken_link.join("child.ts")] {
            let uri = Url::from_file_path(path).unwrap().to_string();
            assert!(DiagnosticKey::new("ts", &uri).is_none());
        }
    }

    // Catches unversioned publications discarding known version authority and admitting later stale data.
    #[test]
    fn unversioned_publish_replaces_contents_but_preserves_stale_version_rejection() {
        let mut store = DiagnosticStore::default();
        store.replace(key("ts", "/repo/a.ts"), Some(5), vec![diag("versioned")]);
        store.replace(key("ts", "/repo/a.ts"), None, vec![diag("unversioned")]);
        let changed = store.replace(key("ts", "/repo/a.ts"), Some(4), vec![diag("stale")]);

        assert!(!changed);
        assert_eq!(store.snapshot(None).items[0].message, "unversioned");
    }

    // Catches remote or non-file URIs being admitted into the local-only authoritative store.
    #[test]
    fn non_file_uris_cannot_be_diagnostic_keys() {
        assert!(DiagnosticKey::new("ts", "https://example.test/a.ts").is_none());
    }

    fn key(session_id: &str, path: &str) -> DiagnosticKey {
        let uri = format!("file://{path}");
        DiagnosticKey::new(session_id, &uri).expect("absolute file URI")
    }

    fn diag(message: &str) -> Diagnostic {
        diag_at(message, 0, 0)
    }

    fn diag_at(message: &str, line: u32, character: u32) -> Diagnostic {
        Diagnostic {
            id: format!("id-{message}"),
            uri: "file:///untrusted-input.ts".into(),
            range: EditorRange {
                start: EditorPosition { line, character },
                end: EditorPosition {
                    line,
                    character: character + 1,
                },
            },
            severity: DiagnosticSeverity::Warning,
            code: Some("W1".into()),
            source: Some("test".into()),
            message: message.into(),
            related_information: vec![DiagnosticRelatedInformation {
                location: EditorLocation {
                    uri: "file:///untrusted-related.ts".into(),
                    range: EditorRange {
                        start: EditorPosition {
                            line: 9,
                            character: 1,
                        },
                        end: EditorPosition {
                            line: 9,
                            character: 2,
                        },
                    },
                },
                message: "related context".into(),
            }],
        }
    }

    fn diag_with_severity(message: &str, severity: DiagnosticSeverity) -> Diagnostic {
        let mut diagnostic = diag(message);
        diagnostic.severity = severity;
        diagnostic
    }
}
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use async_lsp::lsp_types::Url;

use super::types::{Diagnostic, DiagnosticCounts, DiagnosticSeverity, DiagnosticSnapshot};

/// Identifies one server's authoritative publication for a canonical local file URI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DiagnosticKey {
    session_id: String,
    canonical_uri: String,
    canonical_path: PathBuf,
}

impl DiagnosticKey {
    /// Returns no key for remote, relative, malformed, or otherwise non-local URIs.
    pub(crate) fn new(session_id: impl Into<String>, uri: &str) -> Option<Self> {
        let url = Url::parse(uri).ok()?;
        if url.scheme() != "file" {
            return None;
        }
        let path = url.to_file_path().ok()?;
        let canonical_path = canonical_path(&path)?;
        let canonical_uri = Url::from_file_path(&canonical_path).ok()?.to_string();
        Some(Self {
            session_id: session_id.into(),
            canonical_uri,
            canonical_path,
        })
    }
}

#[derive(Debug, Clone)]
struct DiagnosticEntry {
    /// The newest ordered document version accepted for this session/URI. An
    /// unversioned publish deliberately leaves this intact, so it cannot make
    /// an older later version authoritative again.
    document_version: Option<i32>,
    diagnostics: Vec<Diagnostic>,
}

/// The Rust-owned authoritative diagnostic state. Entries include empty
/// publications as version tombstones so an older update cannot resurrect a
/// cleared result.
#[derive(Debug, Default)]
pub(crate) struct DiagnosticStore {
    revision: u64,
    entries: HashMap<DiagnosticKey, DiagnosticEntry>,
}

impl DiagnosticStore {
    /// Replaces this session/URI publication when its document version is not
    /// older than the accepted one. Equal versions are intentionally accepted:
    /// the most recently received full publication remains authoritative.
    ///
    /// Returns `false` only for a stale versioned publication.
    pub(crate) fn replace(
        &mut self,
        key: DiagnosticKey,
        document_version: Option<i32>,
        mut diagnostics: Vec<Diagnostic>,
    ) -> bool {
        if self
            .entries
            .get(&key)
            .and_then(|entry| entry.document_version)
            .is_some_and(|accepted| document_version.is_some_and(|incoming| incoming < accepted))
        {
            return false;
        }

        for diagnostic in &mut diagnostics {
            diagnostic.uri.clone_from(&key.canonical_uri);
        }

        let next_version = document_version.or_else(|| {
            self.entries
                .get(&key)
                .and_then(|entry| entry.document_version)
        });
        let changed = self.entries.get(&key).is_none_or(|entry| {
            entry.document_version != next_version || entry.diagnostics != diagnostics
        });
        if changed {
            self.entries.insert(
                key,
                DiagnosticEntry {
                    document_version: next_version,
                    diagnostics,
                },
            );
            self.bump_revision();
        }
        true
    }

    /// Removes all publications and version tombstones belonging to a stopped
    /// session. Returns whether the visible/authority state changed.
    pub(crate) fn clear_session(&mut self, session_id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|key, _| key.session_id != session_id);
        let changed = self.entries.len() != before;
        if changed {
            self.bump_revision();
        }
        changed
    }

    /// Produces an order independent of HashMap layout. `workspace_root` is
    /// compared as path components, never by URI or string prefix.
    pub(crate) fn snapshot(&self, workspace_root: Option<&Path>) -> DiagnosticSnapshot {
        let workspace_root = match workspace_root {
            Some(root) => match canonical_path(root) {
                Some(root) => Some(root),
                None => return self.empty_snapshot(),
            },
            None => None,
        };
        let mut items: Vec<(&DiagnosticKey, &Diagnostic)> = self
            .entries
            .iter()
            .filter(|(key, _)| {
                workspace_root
                    .as_ref()
                    .is_none_or(|root| key.canonical_path.starts_with(root))
            })
            .flat_map(|(key, entry)| {
                entry
                    .diagnostics
                    .iter()
                    .map(move |diagnostic| (key, diagnostic))
            })
            .collect();
        items.sort_unstable_by(|(left_key, left), (right_key, right)| {
            compare_diagnostics(left, right)
                .then_with(|| left_key.session_id.cmp(&right_key.session_id))
        });

        let mut counts = DiagnosticCounts {
            errors: 0,
            warnings: 0,
            information: 0,
            hints: 0,
        };
        for (_, diagnostic) in &items {
            match diagnostic.severity {
                DiagnosticSeverity::Error => counts.errors += 1,
                DiagnosticSeverity::Warning => counts.warnings += 1,
                DiagnosticSeverity::Information => counts.information += 1,
                DiagnosticSeverity::Hint => counts.hints += 1,
            }
        }

        DiagnosticSnapshot {
            revision: self.revision,
            items: items
                .into_iter()
                .map(|(_, diagnostic)| diagnostic.clone())
                .collect(),
            counts,
        }
    }

    fn empty_snapshot(&self) -> DiagnosticSnapshot {
        DiagnosticSnapshot {
            revision: self.revision,
            items: Vec::new(),
            counts: DiagnosticCounts {
                errors: 0,
                warnings: 0,
                information: 0,
                hints: 0,
            },
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("diagnostic store revision exhausted");
    }
}

fn canonical_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let normalized = normalize_absolute_path(path);
    let mut candidate = normalized.as_path();
    let mut unresolved_suffix = Vec::new();

    loop {
        match fs::symlink_metadata(candidate) {
            Ok(_) => {
                let mut resolved = fs::canonicalize(candidate).ok()?;
                if !unresolved_suffix.is_empty() && !fs::metadata(&resolved).ok()?.is_dir() {
                    return None;
                }
                for component in unresolved_suffix.iter().rev() {
                    resolved.push(component);
                }
                return Some(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                unresolved_suffix.push(candidate.file_name()?.to_os_string());
                candidate = candidate.parent()?;
            }
            Err(_) => return None,
        }
    }
}

/// Normalizes `.` and `..` components without touching the filesystem. This
/// gives deleted historical paths a stable key and workspace-filter behavior.
fn normalize_absolute_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

fn compare_diagnostics(left: &Diagnostic, right: &Diagnostic) -> Ordering {
    left.uri
        .cmp(&right.uri)
        .then_with(|| left.range.start.line.cmp(&right.range.start.line))
        .then_with(|| left.range.start.character.cmp(&right.range.start.character))
        .then_with(|| left.range.end.line.cmp(&right.range.end.line))
        .then_with(|| left.range.end.character.cmp(&right.range.end.character))
        .then_with(|| severity_rank(left.severity).cmp(&severity_rank(right.severity)))
        .then_with(|| left.id.cmp(&right.id))
        .then_with(|| left.code.cmp(&right.code))
        .then_with(|| left.source.cmp(&right.source))
        .then_with(|| left.message.cmp(&right.message))
        .then_with(|| compare_related_information(left, right))
}

fn severity_rank(severity: DiagnosticSeverity) -> u8 {
    match severity {
        DiagnosticSeverity::Error => 0,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Information => 2,
        DiagnosticSeverity::Hint => 3,
    }
}

fn compare_related_information(left: &Diagnostic, right: &Diagnostic) -> Ordering {
    for (left, right) in left
        .related_information
        .iter()
        .zip(&right.related_information)
    {
        let ordering = left
            .location
            .uri
            .cmp(&right.location.uri)
            .then_with(|| {
                left.location
                    .range
                    .start
                    .line
                    .cmp(&right.location.range.start.line)
            })
            .then_with(|| {
                left.location
                    .range
                    .start
                    .character
                    .cmp(&right.location.range.start.character)
            })
            .then_with(|| {
                left.location
                    .range
                    .end
                    .line
                    .cmp(&right.location.range.end.line)
            })
            .then_with(|| {
                left.location
                    .range
                    .end
                    .character
                    .cmp(&right.location.range.end.character)
            })
            .then_with(|| left.message.cmp(&right.message));
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.related_information
        .len()
        .cmp(&right.related_information.len())
}
