//! Git status for the project tree.
//!
//! `git status --porcelain=v2 -z` is run against the project root with a
//! timeout, and the parsing is a pure function over the raw bytes so the
//! record framing (including a rename's second NUL-terminated field, which is
//! the part every naive parser gets wrong) is unit-tested against fixtures.
//!
//! No git on PATH, not a repository, or a timeout all produce
//! [`unavailable`], and the feature is silently off. Never a toast: a project
//! that is not a git repository is completely ordinary, and telling the user
//! about it on every refresh would be noise.
//!
//! `git status` emits paths relative to the repository's TOPLEVEL regardless
//! of the `-C` flag used to invoke it — confirmed empirically, not merely
//! assumed — so a project opened on a subdirectory of a larger repo (a
//! monorepo's `frontend/`, say) would otherwise see every OTHER package's
//! changes, addressed by paths that do not even resolve under its own root.
//! [`project_git_status_for_root`] resolves the toplevel with a second,
//! equally bounded `git rev-parse --show-toplevel` call and rewrites the
//! snapshot onto root-relative keys, dropping anything outside the project
//! root entirely — the frontend's contract (see `git-tints.js`) is that
//! every key it receives is already relative to the root it asked about.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum GitFileState {
    Modified,
    Added,
    Untracked,
    Deleted,
    Renamed,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GitStatusSnapshot {
    pub available: bool,
    /// Repo-relative path → state. `BTreeMap` so the serialized snapshot is
    /// deterministic, which makes a diff of two snapshots readable in a log.
    pub files: BTreeMap<String, GitFileState>,
}

pub(crate) fn unavailable() -> GitStatusSnapshot {
    GitStatusSnapshot {
        available: false,
        files: BTreeMap::new(),
    }
}

/// Map an ordinary entry's two-character `XY` field to one state. Staged and
/// unstaged are collapsed deliberately: the tree shows THAT a file changed,
/// not the index/worktree split, which belongs in a diff view.
fn ordinary_state(xy: &str) -> Option<GitFileState> {
    let mut chars = xy.chars();
    let x = chars.next()?;
    let y = chars.next()?;
    for code in [x, y] {
        match code {
            'A' => return Some(GitFileState::Added),
            'D' => return Some(GitFileState::Deleted),
            'R' | 'C' => return Some(GitFileState::Renamed),
            _ => {}
        }
    }
    if x == 'M' || y == 'M' || x == 'T' || y == 'T' {
        return Some(GitFileState::Modified);
    }
    None
}

/// Parse `git status --porcelain=v2 -z` output.
///
/// Records are NUL-terminated. `1 ` is an ordinary change, `2 ` a rename or
/// copy (whose ORIGINAL path follows as its own NUL-terminated field — it is
/// a field of this record, not a record of its own), `u ` an unmerged entry,
/// `? ` untracked and `! ` ignored. `# ` lines are headers. Anything that does
/// not parse is skipped rather than aborting: a partial snapshot is a tint or
/// two short, an aborted one is the whole feature off.
pub(crate) fn parse_porcelain_v2(bytes: &[u8]) -> BTreeMap<String, GitFileState> {
    let mut out = BTreeMap::new();
    let mut records = bytes
        .split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .map(|r| String::from_utf8_lossy(r).into_owned());

    while let Some(record) = records.next() {
        let Some((marker, rest)) = record.split_once(' ') else {
            continue;
        };
        match marker {
            "#" | "!" => {}
            "?" => {
                if !rest.is_empty() {
                    out.insert(rest.to_string(), GitFileState::Untracked);
                }
            }
            "1" => {
                // <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
                let fields: Vec<&str> = rest.splitn(8, ' ').collect();
                if fields.len() < 8 {
                    continue;
                }
                if let Some(state) = ordinary_state(fields[0])
                    && !fields[7].is_empty()
                {
                    out.insert(fields[7].to_string(), state);
                }
            }
            "2" => {
                // <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>,
                // then the original path as the NEXT NUL-terminated field.
                let fields: Vec<&str> = rest.splitn(9, ' ').collect();
                // The original-path field belongs to this record whether or
                // not the record itself parsed, so it is consumed first.
                let _original = records.next();
                if fields.len() < 9 || fields[8].is_empty() {
                    continue;
                }
                out.insert(fields[8].to_string(), GitFileState::Renamed);
            }
            "u" => {
                // <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
                let fields: Vec<&str> = rest.splitn(10, ' ').collect();
                if fields.len() < 10 || fields[9].is_empty() {
                    continue;
                }
                out.insert(fields[9].to_string(), GitFileState::Conflicted);
            }
            _ => {}
        }
    }
    out
}

/// Run `git status` against `root`, bounded by [`GIT_TIMEOUT`].
///
/// The wait happens on a worker thread so a git that hangs (a stale lock, a
/// network filesystem) cannot hold the command's thread forever; on a timeout
/// the child is killed and the snapshot comes back unavailable.
fn read_status(root: &str) -> GitStatusSnapshot {
    let mut child = match std::process::Command::new("git")
        .args(["-C", root, "status", "--porcelain=v2", "-z"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        // No git on PATH is not an error worth reporting: the feature is off.
        Err(e) => {
            log::debug!("project git status: could not spawn git for {root}: {e}");
            return unavailable();
        }
    };

    let Some(stdout) = child.stdout.take() else {
        log::debug!("project git status: git spawned for {root} with no stdout pipe");
        let _ = child.kill();
        return unavailable();
    };

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("project-git-status".into())
        .spawn(move || {
            use std::io::Read;
            let mut buffer = Vec::new();
            let mut reader = stdout;
            let read = reader.read_to_end(&mut buffer).is_ok();
            let _ = tx.send(if read { Some(buffer) } else { None });
        })
        .ok();

    let bytes = match rx.recv_timeout(GIT_TIMEOUT) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            log::debug!("project git status: could not read git output for {root}");
            let _ = child.kill();
            let _ = child.wait();
            return unavailable();
        }
        Err(_) => {
            log::debug!("project git status: timed out waiting on git for {root}");
            let _ = child.kill();
            let _ = child.wait();
            return unavailable();
        }
    };

    match child.wait() {
        Ok(status) if status.success() => GitStatusSnapshot {
            available: true,
            files: parse_porcelain_v2(&bytes),
        },
        // A non-zero exit is "not a repository" in practice. Silently off.
        Ok(_) => unavailable(),
        Err(e) => {
            log::debug!("project git status: could not wait on git for {root}: {e}");
            unavailable()
        }
    }
}

/// Resolve `root`'s git repository toplevel via `git rev-parse
/// --show-toplevel`, canonicalized so it can be compared against a project
/// root with plain path operations. `None` covers exactly the same ground as
/// [`unavailable`] — no git on PATH, not a repository, a timeout, or output
/// that fails to canonicalize — so callers fold it straight into the same
/// silent-off behaviour. Same subprocess discipline as [`read_status`]: the
/// wait happens on a worker thread, bounded by [`GIT_TIMEOUT`], with the
/// child killed and reaped on a timeout so a hung git cannot block the
/// command's thread.
fn resolve_toplevel(root: &str) -> Option<PathBuf> {
    let mut child = match std::process::Command::new("git")
        .args(["-C", root, "rev-parse", "--show-toplevel"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log::debug!("project git toplevel: could not spawn git for {root}: {e}");
            return None;
        }
    };

    let Some(stdout) = child.stdout.take() else {
        log::debug!("project git toplevel: git spawned for {root} with no stdout pipe");
        let _ = child.kill();
        return None;
    };

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("project-git-toplevel".into())
        .spawn(move || {
            use std::io::Read;
            let mut buffer = Vec::new();
            let mut reader = stdout;
            let read = reader.read_to_end(&mut buffer).is_ok();
            let _ = tx.send(if read { Some(buffer) } else { None });
        })
        .ok();

    let bytes = match rx.recv_timeout(GIT_TIMEOUT) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            log::debug!("project git toplevel: could not read git output for {root}");
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        Err(_) => {
            log::debug!("project git toplevel: timed out waiting on git for {root}");
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };

    match child.wait() {
        Ok(status) if status.success() => {
            let text = String::from_utf8_lossy(&bytes);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                log::debug!("project git toplevel: empty output for {root}");
                return None;
            }
            match std::fs::canonicalize(trimmed) {
                Ok(path) => Some(path),
                Err(e) => {
                    log::debug!(
                        "project git toplevel: could not canonicalize {trimmed} for {root}: {e}"
                    );
                    None
                }
            }
        }
        // A non-zero exit is "not a repository" in practice. Silently off.
        Ok(_) => None,
        Err(e) => {
            log::debug!("project git toplevel: could not wait on git for {root}: {e}");
            None
        }
    }
}

/// Rewrite a toplevel-relative snapshot (what `git status` actually emits,
/// regardless of `-C`) into one keyed by paths relative to the PROJECT root,
/// dropping every entry outside it. `toplevel` and `root` must both already
/// be canonical absolute paths. Matches on path SEGMENTS via the trailing
/// `/` appended to the boundary below, never a bare string prefix — a
/// project root named `frontend` must not swallow a sibling directory named
/// `frontend-tools`.
fn rebase_to_root(
    files: BTreeMap<String, GitFileState>,
    toplevel: &Path,
    root: &Path,
) -> BTreeMap<String, GitFileState> {
    let suffix = match root.strip_prefix(toplevel) {
        Ok(suffix) => suffix,
        // The project root is not under its own reported toplevel — should
        // not happen in practice, but nothing here can be trusted as in
        // scope if it does.
        Err(_) => return BTreeMap::new(),
    };
    let prefix = git_style_relative_path(suffix);
    if prefix.is_empty() {
        // The project root IS the repo toplevel: every key git emitted is
        // already root-relative.
        return files;
    }
    let boundary = format!("{prefix}/");
    files
        .into_iter()
        .filter_map(|(key, state)| {
            key.strip_prefix(boundary.as_str())
                .map(|rest| (rest.to_string(), state))
        })
        .collect()
}

/// `/`-joined path segments regardless of the host OS's own separator —
/// git's snapshot keys are always `/`-separated, so the boundary this feeds
/// [`rebase_to_root`] must be too, even on a platform whose paths are not.
fn git_style_relative_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The full pipeline for one root: run `git status`, resolve the repo
/// toplevel, and rewrite the snapshot onto root-relative keys. Split out
/// from the [`project_git_status`] command so it can be exercised directly
/// against a real temporary repository in tests, with no Tauri window
/// needed.
pub(crate) fn project_git_status_for_root(root: &str) -> GitStatusSnapshot {
    let snapshot = read_status(root);
    if !snapshot.available {
        return snapshot;
    }
    let Some(toplevel) = resolve_toplevel(root) else {
        log::debug!(
            "project git status: could not resolve a toplevel for {root}; treating as unavailable"
        );
        return unavailable();
    };
    let root_path = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(e) => {
            log::debug!("project git status: could not canonicalize root {root}: {e}");
            return unavailable();
        }
    };
    GitStatusSnapshot {
        available: true,
        files: rebase_to_root(snapshot.files, &toplevel, &root_path),
    }
}

/// The calling window's project git status. Replace-only: the frontend never
/// merges two snapshots, so a file that stopped being modified simply stops
/// appearing.
#[tauri::command]
pub(crate) fn project_git_status(window: tauri::WebviewWindow) -> GitStatusSnapshot {
    use parking_lot::Mutex;
    use tauri::Manager;

    let root = window
        .app_handle()
        .state::<Mutex<super::ProjectRegistry>>()
        .lock()
        .root_for(window.label());
    match root {
        Some(root) => project_git_status_for_root(&root),
        None => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `git status --porcelain=v2 -z` framing: every record ends with a NUL,
    // and a rename record (`2 `) is followed by a SECOND NUL-terminated field
    // holding the original path. Getting that wrong is how a rename swallows
    // the record after it, so the fixtures below are byte-exact.
    fn fixture(records: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for record in records {
            out.extend_from_slice(record.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn parses_ordinary_changed_entries() {
        let bytes = fixture(&[
            "1 .M N... 100644 100644 100644 aaaa bbbb src/main.rs",
            "1 M. N... 100644 100644 100644 cccc dddd src/lib.rs",
        ]);
        let files = parse_porcelain_v2(&bytes);
        assert_eq!(files.get("src/main.rs"), Some(&GitFileState::Modified));
        assert_eq!(files.get("src/lib.rs"), Some(&GitFileState::Modified));
    }

    #[test]
    fn an_added_path_is_added_and_a_deleted_path_is_deleted() {
        let bytes = fixture(&[
            "1 A. N... 000000 100644 100644 0000 eeee new.rs",
            "1 .D N... 100644 100644 000000 ffff 0000 gone.rs",
            "1 D. N... 100644 000000 000000 aaaa 0000 staged-gone.rs",
        ]);
        let files = parse_porcelain_v2(&bytes);
        assert_eq!(files.get("new.rs"), Some(&GitFileState::Added));
        assert_eq!(files.get("gone.rs"), Some(&GitFileState::Deleted));
        assert_eq!(files.get("staged-gone.rs"), Some(&GitFileState::Deleted));
    }

    #[test]
    fn a_rename_record_consumes_its_original_path_field() {
        // The record AFTER the rename must still parse: a parser that forgets
        // the second NUL-terminated field reads "old.rs" as the next record.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"2 R. N... 100644 100644 100644 aaaa bbbb R100 new.rs");
        bytes.push(0);
        bytes.extend_from_slice(b"old.rs");
        bytes.push(0);
        bytes.extend_from_slice(b"? untracked.rs");
        bytes.push(0);
        let files = parse_porcelain_v2(&bytes);
        assert_eq!(files.get("new.rs"), Some(&GitFileState::Renamed));
        assert_eq!(
            files.get("old.rs"),
            None,
            "the original path is a field, not a record"
        );
        assert_eq!(
            files.get("untracked.rs"),
            Some(&GitFileState::Untracked),
            "the record after a rename must still be read"
        );
    }

    #[test]
    fn untracked_ignored_and_headers() {
        let bytes = fixture(&[
            "# branch.oid aaaaaaa",
            "# branch.head main",
            "? notes.txt",
            "! target/debug/thing",
        ]);
        let files = parse_porcelain_v2(&bytes);
        assert_eq!(files.get("notes.txt"), Some(&GitFileState::Untracked));
        assert_eq!(
            files.get("target/debug/thing"),
            None,
            "ignored files carry no tint"
        );
        assert_eq!(files.len(), 1, "headers contribute nothing");
    }

    #[test]
    fn unmerged_entries_are_conflicted() {
        let bytes = fixture(&["u UU N... 100644 100644 100644 100644 aaaa bbbb cccc both.rs"]);
        let files = parse_porcelain_v2(&bytes);
        assert_eq!(files.get("both.rs"), Some(&GitFileState::Conflicted));
    }

    #[test]
    fn a_path_with_spaces_keeps_its_whole_name() {
        // The path is the LAST field and may contain spaces; -z is precisely
        // why it does not need quoting.
        let bytes = fixture(&["1 .M N... 100644 100644 100644 aaaa bbbb my notes/a b.txt"]);
        let files = parse_porcelain_v2(&bytes);
        assert_eq!(files.get("my notes/a b.txt"), Some(&GitFileState::Modified));
    }

    #[test]
    fn malformed_input_yields_an_empty_map_rather_than_a_panic() {
        assert!(parse_porcelain_v2(b"").is_empty());
        assert!(parse_porcelain_v2(&fixture(&["1 short"])).is_empty());
        assert!(
            parse_porcelain_v2(&fixture(&["?"])).is_empty(),
            "a bare marker names no path"
        );
        assert!(parse_porcelain_v2(&fixture(&["x nonsense"])).is_empty());
        // A rename record truncated before its original-path field.
        let mut truncated = Vec::new();
        truncated.extend_from_slice(b"2 R. N... 100644 100644 100644 aaaa bbbb R100 new.rs");
        truncated.push(0);
        let files = parse_porcelain_v2(&truncated);
        assert_eq!(files.get("new.rs"), Some(&GitFileState::Renamed));
    }

    #[test]
    fn unavailable_is_empty_and_flagged() {
        let snapshot = unavailable();
        assert!(!snapshot.available);
        assert!(snapshot.files.is_empty());
    }

    #[test]
    fn snapshot_serializes_states_in_lowercase() {
        let mut files = BTreeMap::new();
        files.insert("a.rs".to_string(), GitFileState::Untracked);
        let json = serde_json::to_string(&GitStatusSnapshot {
            available: true,
            files,
        })
        .expect("serialize");
        assert!(json.contains("\"available\":true"), "got {json}");
        assert!(json.contains("\"a.rs\":\"untracked\""), "got {json}");
    }

    // Deferred from task 10's own review: `ordinary_state` checks X, then Y,
    // returning on the FIRST match — a both-sides-set entry exercises that
    // precedence for real instead of only ever presenting one side set.
    #[test]
    fn an_entry_with_both_sides_set_resolves_by_marker_precedence() {
        let bytes = fixture(&[
            // X='A' matches before Y='M' is ever checked.
            "1 AM N... 100644 100644 100644 aaaa bbbb added-and-modified.rs",
            // X='M' matches nothing in the first pass; Y='D' matches on the
            // second, so Deleted wins over the Modified fallback below it.
            "1 MD N... 100644 100644 100644 cccc dddd modified-and-deleted.rs",
        ]);
        let files = parse_porcelain_v2(&bytes);
        assert_eq!(
            files.get("added-and-modified.rs"),
            Some(&GitFileState::Added),
            "X is checked before Y"
        );
        assert_eq!(
            files.get("modified-and-deleted.rs"),
            Some(&GitFileState::Deleted),
            "D on either side outranks a plain M"
        );
    }

    // Also deferred from task 10: a copy (score '2', X='C') takes the same
    // marker-"2" path as a rename and was never exercised on its own.
    #[test]
    fn a_copy_score_entry_parses_as_renamed() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"2 C. N... 100644 100644 100644 aaaa bbbb C100 copy.rs");
        bytes.push(0);
        bytes.extend_from_slice(b"original.rs");
        bytes.push(0);
        let files = parse_porcelain_v2(&bytes);
        assert_eq!(
            files.get("copy.rs"),
            Some(&GitFileState::Renamed),
            "a copy collapses onto the same tint as a rename"
        );
        assert_eq!(files.get("original.rs"), None);
    }

    // `-z` framing means a literal newline byte inside a path is just
    // ordinary path content, not a record separator.
    #[test]
    fn a_path_with_an_embedded_newline_keeps_its_whole_name() {
        let bytes = fixture(&["1 .M N... 100644 100644 100644 aaaa bbbb weird\nname.rs"]);
        let files = parse_porcelain_v2(&bytes);
        assert_eq!(
            files.get("weird\nname.rs"),
            Some(&GitFileState::Modified),
            "the embedded newline is part of the path, not a framing byte"
        );
        assert_eq!(files.len(), 1);
    }

    // --- rebase_to_root: the controller-ruling extension ---------------------
    //
    // git emits toplevel-relative paths regardless of `-C`, so a project
    // opened on a subdirectory of a larger repo needs its snapshot rewritten
    // onto root-relative keys with everything outside the root dropped.
    // These first four are pure — no subprocess — exercising rebase_to_root
    // directly against synthetic paths; the fixture below them drives the
    // real `git` binary end to end.

    #[test]
    fn rebase_to_root_passes_through_unchanged_when_root_is_the_toplevel() {
        let mut files = BTreeMap::new();
        files.insert("src/main.rs".to_string(), GitFileState::Modified);
        files.insert("README.md".to_string(), GitFileState::Untracked);
        let toplevel = Path::new("/repo");
        let root = Path::new("/repo");
        let rebased = rebase_to_root(files.clone(), toplevel, root);
        assert_eq!(rebased, files, "no subdirectory to strip means no rewrite");
    }

    #[test]
    fn rebase_to_root_strips_the_prefix_and_drops_entries_outside_it() {
        let mut files = BTreeMap::new();
        files.insert("frontend/app.js".to_string(), GitFileState::Modified);
        files.insert("frontend/nested/x.js".to_string(), GitFileState::Added);
        files.insert("backend/server.rs".to_string(), GitFileState::Modified);
        let toplevel = Path::new("/repo");
        let root = Path::new("/repo/frontend");
        let rebased = rebase_to_root(files, toplevel, root);
        assert_eq!(rebased.get("app.js"), Some(&GitFileState::Modified));
        assert_eq!(rebased.get("nested/x.js"), Some(&GitFileState::Added));
        assert_eq!(
            rebased.get("backend/server.rs"),
            None,
            "a sibling package is outside this project root"
        );
        assert_eq!(
            rebased.len(),
            2,
            "the sibling entry must not survive under any key"
        );
    }

    #[test]
    fn rebase_to_root_matches_on_path_segments_not_string_prefixes() {
        let mut files = BTreeMap::new();
        files.insert("frontend/app.js".to_string(), GitFileState::Modified);
        files.insert(
            "frontend-tools/note.txt".to_string(),
            GitFileState::Untracked,
        );
        let toplevel = Path::new("/repo");
        let root = Path::new("/repo/frontend");
        let rebased = rebase_to_root(files, toplevel, root);
        assert_eq!(rebased.get("app.js"), Some(&GitFileState::Modified));
        assert_eq!(
            rebased.get("tools/note.txt"),
            None,
            "\"frontend-tools\" is not inside \"frontend\""
        );
        assert_eq!(
            rebased.len(),
            1,
            "the sibling directory must not leak in under any key"
        );
    }

    #[test]
    fn rebase_to_root_returns_empty_when_root_is_not_under_toplevel() {
        let mut files = BTreeMap::new();
        files.insert("a.rs".to_string(), GitFileState::Modified);
        let toplevel = Path::new("/repo");
        let root = Path::new("/elsewhere");
        let rebased = rebase_to_root(files, toplevel, root);
        assert!(rebased.is_empty());
    }

    // --- project_git_status_for_root: end to end against a real repo --------
    //
    // Red-first for the controller ruling: builds an actual git repository
    // with a project root nested one directory below the toplevel (the
    // monorepo shape) and confirms the returned snapshot is keyed relative
    // to that SUBDIRECTORY, not the repo root, and that files belonging to a
    // sibling package — including one whose name merely shares a string
    // prefix with the project directory — are absent entirely.

    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git must be on PATH to run this test");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    #[test]
    fn project_git_status_for_root_rewrites_keys_onto_a_subdirectory_project_root() {
        let repo = tempfile::tempdir().expect("tempdir");
        let repo_path = repo.path();
        run_git(repo_path, &["init", "-q"]);

        // A sibling package under the toplevel, outside the project root.
        std::fs::create_dir_all(repo_path.join("backend")).unwrap();
        std::fs::write(repo_path.join("backend/server.rs"), "fn main() {}\n").unwrap();

        // A sibling directory that merely SHARES a string prefix with the
        // project root's own name — the path-boundary case.
        std::fs::create_dir_all(repo_path.join("frontend-tools")).unwrap();
        std::fs::write(repo_path.join("frontend-tools/note.txt"), "note\n").unwrap();

        // The project root itself: a subdirectory of the repo toplevel.
        let project_dir = repo_path.join("frontend");
        std::fs::create_dir_all(project_dir.join("nested")).unwrap();
        std::fs::write(project_dir.join("app.js"), "console.log(1);\n").unwrap();
        std::fs::write(project_dir.join("nested/x.js"), "console.log(2);\n").unwrap();

        // `git status` collapses a WHOLLY untracked directory into one
        // "dir/" entry rather than listing each file inside it (that is a
        // pre-existing, unrelated characteristic of the default
        // `--untracked-files=normal` mode this feature already used before
        // this task). Staging the files sidesteps it so this fixture
        // exercises the toplevel-rewrite logic under test, not that
        // collapsing behaviour.
        run_git(repo_path, &["add", "-A"]);

        let project_dir_str = project_dir.display().to_string();
        let snapshot = project_git_status_for_root(&project_dir_str);

        assert!(snapshot.available, "a real repository must be available");
        assert_eq!(
            snapshot.files.get("app.js"),
            Some(&GitFileState::Added),
            "keyed relative to the PROJECT root, not the repo toplevel"
        );
        assert_eq!(
            snapshot.files.get("nested/x.js"),
            Some(&GitFileState::Added)
        );
        assert_eq!(
            snapshot.files.get("backend/server.rs"),
            None,
            "a sibling package outside the project root must not appear under any key (it is added, so unrewritten it would show as backend/server.rs)"
        );
        assert!(
            !snapshot.files.keys().any(|k| k.contains("frontend-tools")),
            "a same-prefix sibling directory must not leak in: {:?}",
            snapshot.files.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            snapshot.files.len(),
            2,
            "only the two files under the project root: {:?}",
            snapshot.files
        );
    }

    #[test]
    fn project_git_status_for_root_is_unaffected_when_root_is_the_toplevel_itself() {
        let repo = tempfile::tempdir().expect("tempdir");
        let repo_path = repo.path();
        run_git(repo_path, &["init", "-q"]);
        std::fs::write(repo_path.join("top.rs"), "fn main() {}\n").unwrap();

        let root_str = repo_path.display().to_string();
        let snapshot = project_git_status_for_root(&root_str);

        assert!(snapshot.available);
        assert_eq!(snapshot.files.get("top.rs"), Some(&GitFileState::Untracked));
    }

    #[test]
    fn project_git_status_for_root_is_unavailable_outside_any_repository() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_str = dir.path().display().to_string();
        let snapshot = project_git_status_for_root(&dir_str);
        assert!(!snapshot.available);
        assert!(snapshot.files.is_empty());
    }
}
