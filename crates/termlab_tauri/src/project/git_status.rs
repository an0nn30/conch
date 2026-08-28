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

use std::collections::BTreeMap;
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
        Err(_) => return unavailable(),
    };

    let Some(stdout) = child.stdout.take() else {
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
        Ok(None) | Err(_) => {
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
        _ => unavailable(),
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
        Some(root) => read_status(&root),
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
}
