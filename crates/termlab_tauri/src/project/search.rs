//! Project-wide literal text search.
//!
//! Pure Rust — there is no dependence on `rg` existing on the host. The walk
//! is the `ignore` crate's (so `.gitignore`, `.ignore`, global excludes and
//! hidden VCS directories behave the way every other tool in the user's shell
//! behaves); the matching is a literal substring scan, case-sensitive or not.
//! Regex is deliberately future work.
//!
//! Results stream to the calling window in batches rather than accumulating,
//! so a first hit in a large tree is on screen long before the walk finishes,
//! and the walk stops at a hard cap. Cancellation is a flag the walker checks
//! per file: a superseding query sets the previous search's flag, so at most
//! one walk per window is ever producing results.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// The hard cap. A query like "e" over a monorepo is not a useful result set
/// at any size, and an uncapped stream is a way to wedge the webview.
pub(crate) const MAX_MATCHES: usize = 1000;
/// Files above this are not what a text search is for.
pub(crate) const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Matches per emitted event.
pub(crate) const BATCH_SIZE: usize = 100;
/// How much of a file is probed for NUL before deciding it is binary.
const BINARY_PROBE_BYTES: usize = 8192;
/// Previews longer than this are pointless in a one-line row.
const MAX_PREVIEW_CHARS: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchOptions {
    pub query: String,
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchMatch {
    pub path: String,
    pub relative_path: String,
    pub line: u32,
    pub column: u32,
    pub preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SearchOutcome {
    pub matched: usize,
    pub capped: bool,
    pub cancelled: bool,
}

/// A NUL in the first block is the same cheap test `editor_fs::looks_binary`
/// uses, and for the same reason: it is what actually distinguishes a source
/// file from an object file without decoding anything.
pub(crate) fn looks_binary(head: &[u8]) -> bool {
    head.iter().take(BINARY_PROBE_BYTES).any(|b| *b == 0)
}

fn truncate_preview(line: &str) -> String {
    if line.chars().count() <= MAX_PREVIEW_CHARS {
        return line.to_string();
    }
    line.chars().take(MAX_PREVIEW_CHARS).collect()
}

/// Every occurrence of `options.query` in `bytes`, as (1-based line, 1-based
/// byte column, trimmed preview). Lossy UTF-8 so a file with one bad byte
/// still searches instead of being silently skipped.
pub(crate) fn match_lines(bytes: &[u8], options: &SearchOptions) -> Vec<(u32, u32, String)> {
    if options.query.is_empty() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(bytes);
    let needle = if options.case_sensitive {
        options.query.clone()
    } else {
        options.query.to_lowercase()
    };

    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let haystack = if options.case_sensitive {
            line.to_string()
        } else {
            line.to_lowercase()
        };
        let mut from = 0usize;
        while let Some(at) = haystack[from..].find(&needle) {
            let column = from + at;
            out.push((
                (index + 1) as u32,
                (column + 1) as u32,
                truncate_preview(line.trim()),
            ));
            from = column + needle.len().max(1);
            if from >= haystack.len() {
                break;
            }
        }
    }
    out
}

/// Walk `root` and hand every match to `sink`, stopping at [`MAX_MATCHES`] or
/// the moment `cancel` is set.
pub(crate) fn run_search(
    root: &Path,
    options: &SearchOptions,
    cancel: &AtomicBool,
    mut sink: impl FnMut(SearchMatch),
) -> SearchOutcome {
    let mut outcome = SearchOutcome::default();
    if options.query.is_empty() {
        return outcome;
    }
    if cancel.load(Ordering::Relaxed) {
        outcome.cancelled = true;
        return outcome;
    }

    // Defaults are what we want: hidden files skipped, .gitignore/.ignore and
    // parent ignore files honoured, global excludes applied. `require_git`
    // must be turned off — the default only honours `.gitignore` when the
    // root sits inside an actual `.git` repository, but a project root here
    // is whatever directory the user opened, git-tracked or not.
    // Sorted so results land in the same order on every machine and every
    // filesystem — an unsorted walk order is directory-entry order, which is
    // often creation order and varies run to run.
    let walker = ignore::WalkBuilder::new(root)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();
    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            return outcome;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry
            .metadata()
            .map(|m| m.len() > MAX_FILE_BYTES)
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        if looks_binary(&bytes) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .display()
            .to_string();
        for (line, column, preview) in match_lines(&bytes, options) {
            if outcome.matched >= MAX_MATCHES {
                outcome.capped = true;
                return outcome;
            }
            outcome.matched += 1;
            sink(SearchMatch {
                path: entry.path().display().to_string(),
                relative_path: relative.clone(),
                line,
                column,
                preview,
            });
        }
    }
    outcome
}

// ---------------------------------------------------------------------------
// Per-window search state
// ---------------------------------------------------------------------------

/// window label → the live search's cancellation flag. Starting a search
/// cancels whatever that window was running, so a fast typist never has two
/// walks racing to publish into the same panel.
#[derive(Debug, Default)]
pub(crate) struct SearchRegistry {
    by_window: std::collections::HashMap<String, Arc<AtomicBool>>,
    next_id: u64,
}

impl SearchRegistry {
    pub(crate) fn start(&mut self, label: String) -> (String, Arc<AtomicBool>) {
        if let Some(previous) = self.by_window.remove(&label) {
            previous.store(true, Ordering::Relaxed);
        }
        self.next_id += 1;
        let id = format!("search-{}", self.next_id);
        let flag = Arc::new(AtomicBool::new(false));
        self.by_window.insert(label, Arc::clone(&flag));
        (id, flag)
    }

    pub(crate) fn cancel(&mut self, label: &str) {
        if let Some(previous) = self.by_window.remove(label) {
            previous.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResultsEvent {
    pub search_id: String,
    pub matches: Vec<SearchMatch>,
    pub done: bool,
    pub capped: bool,
}

pub(crate) const SEARCH_RESULTS_EVENT: &str = "project-search-results";

/// Start a search over the calling window's project. Returns the search id;
/// results arrive on `project-search-results`, and the emission carrying
/// `done: true` is the terminal one (it says whether the cap was hit).
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn project_search(
    window: tauri::WebviewWindow,
    query: String,
    case_sensitive: bool,
) -> Result<String, String> {
    use parking_lot::Mutex;
    use tauri::Manager;

    let label = window.label().to_string();
    let app = window.app_handle().clone();
    let root = app
        .state::<Mutex<super::ProjectRegistry>>()
        .lock()
        .root_for(&label)
        .ok_or_else(|| "This window has no project".to_string())?;

    let (search_id, cancel) = app
        .state::<Mutex<SearchRegistry>>()
        .lock()
        .start(label.clone());

    let options = SearchOptions {
        query,
        case_sensitive,
    };
    let thread_id = search_id.clone();
    std::thread::Builder::new()
        .name(format!("project-search-{label}"))
        .spawn(move || {
            use tauri::Emitter;
            let mut batch: Vec<SearchMatch> = Vec::with_capacity(BATCH_SIZE);
            let emit = |app: &tauri::AppHandle, batch: Vec<SearchMatch>, done, capped| {
                let _ = app.emit_to(
                    label.as_str(),
                    SEARCH_RESULTS_EVENT,
                    SearchResultsEvent {
                        search_id: thread_id.clone(),
                        matches: batch,
                        done,
                        capped,
                    },
                );
            };
            let outcome = run_search(Path::new(&root), &options, &cancel, |m| {
                batch.push(m);
                if batch.len() >= BATCH_SIZE {
                    emit(&app, std::mem::take(&mut batch), false, false);
                }
            });
            // A cancelled search is silent: it has been superseded, and its
            // terminal event would race the new query's first batch.
            if outcome.cancelled {
                return;
            }
            emit(&app, batch, true, outcome.capped);
        })
        .map_err(|e| format!("Could not start the search: {e}"))?;

    Ok(search_id)
}

/// Stop the calling window's search outright.
#[tauri::command]
pub(crate) fn project_search_cancel(window: tauri::WebviewWindow) {
    use parking_lot::Mutex;
    use tauri::Manager;
    window
        .app_handle()
        .state::<Mutex<SearchRegistry>>()
        .lock()
        .cancel(window.label());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn opts(query: &str, case_sensitive: bool) -> SearchOptions {
        SearchOptions {
            query: query.to_string(),
            case_sensitive,
        }
    }

    #[test]
    fn looks_binary_only_on_a_nul_byte() {
        assert!(!looks_binary(b"fn main() {}\n"));
        assert!(!looks_binary("héllo — em dash".as_bytes()));
        assert!(looks_binary(b"MZ\x00\x90"));
    }

    #[test]
    fn match_lines_reports_one_based_line_and_column_with_a_trimmed_preview() {
        let source = b"fn main() {\n    let needle = 1;\n}\n";
        let hits = match_lines(source, &opts("needle", true));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 2, "lines are 1-based");
        assert_eq!(
            hits[0].1, 9,
            "columns are 1-based and count bytes into the line"
        );
        assert_eq!(
            hits[0].2, "let needle = 1;",
            "the preview is the trimmed line"
        );
    }

    #[test]
    fn match_lines_reports_every_occurrence_on_a_line() {
        let hits = match_lines(b"aXbXc\n", &opts("X", true));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].1, 2);
        assert_eq!(hits[1].1, 4);
    }

    #[test]
    fn match_lines_is_case_insensitive_when_asked() {
        assert!(match_lines(b"Needle\n", &opts("needle", true)).is_empty());
        assert_eq!(match_lines(b"Needle\n", &opts("needle", false)).len(), 1);
    }

    #[test]
    fn match_lines_on_an_empty_query_finds_nothing() {
        assert!(
            match_lines(b"anything\n", &opts("", true)).is_empty(),
            "an empty query must not match every position in the project"
        );
    }

    #[test]
    fn run_search_respects_gitignore_and_reports_relative_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), b"ignored/\n").expect("write");
        std::fs::write(root.join("kept.rs"), b"needle here\n").expect("write");
        std::fs::create_dir(root.join("ignored")).expect("mkdir");
        std::fs::write(root.join("ignored/skip.rs"), b"needle here\n").expect("write");

        let cancel = AtomicBool::new(false);
        let mut found = Vec::new();
        let outcome = run_search(root, &opts("needle", true), &cancel, |m| found.push(m));
        assert_eq!(outcome.matched, 1, "the ignored directory is not searched");
        assert!(!outcome.capped);
        assert!(!outcome.cancelled);
        assert_eq!(
            found[0].relative_path, "kept.rs",
            "paths are relative to the root"
        );
        assert!(
            found[0].path.ends_with("kept.rs"),
            "the absolute path is carried too"
        );
    }

    #[test]
    fn run_search_visits_files_in_deterministic_path_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // Names picked so that a directory-entry-order walk (whatever the
        // filesystem happens to hand back — often creation order, which is
        // the reverse of this) would very likely disagree with alphabetical.
        std::fs::write(root.join("zeta.txt"), b"needle\n").expect("write");
        std::fs::write(root.join("mid.txt"), b"needle\n").expect("write");
        std::fs::write(root.join("alpha.txt"), b"needle\n").expect("write");

        let cancel = AtomicBool::new(false);
        let mut found = Vec::new();
        run_search(root, &opts("needle", true), &cancel, |m| {
            found.push(m.relative_path)
        });
        assert_eq!(
            found,
            vec![
                "alpha.txt".to_string(),
                "mid.txt".to_string(),
                "zeta.txt".to_string()
            ],
            "results must be in a stable, path-sorted order across machines/filesystems"
        );
    }

    #[test]
    fn run_search_skips_binary_files_and_files_over_the_size_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("bin.dat"), b"needle\x00needle").expect("write");
        let mut huge = vec![b' '; (MAX_FILE_BYTES + 1) as usize];
        huge.extend_from_slice(b"needle");
        std::fs::write(root.join("huge.txt"), &huge).expect("write");
        std::fs::write(root.join("ok.txt"), b"needle\n").expect("write");

        let cancel = AtomicBool::new(false);
        let mut found = Vec::new();
        let outcome = run_search(root, &opts("needle", true), &cancel, |m| found.push(m));
        assert_eq!(outcome.matched, 1, "only the plain, small file is searched");
        assert_eq!(found[0].relative_path, "ok.txt");
    }

    #[test]
    fn run_search_stops_at_the_cap_and_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let body = "needle\n".repeat(MAX_MATCHES + 50);
        std::fs::write(root.join("many.txt"), body.as_bytes()).expect("write");

        let cancel = AtomicBool::new(false);
        let mut count = 0usize;
        let outcome = run_search(root, &opts("needle", true), &cancel, |_| count += 1);
        assert_eq!(
            count, MAX_MATCHES,
            "never more than the cap reaches the sink"
        );
        assert_eq!(outcome.matched, MAX_MATCHES);
        assert!(outcome.capped, "the outcome flags that the cap was hit");
    }

    #[test]
    fn run_search_honours_a_cancellation_flag_that_is_already_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"needle\n").expect("write");
        let cancel = AtomicBool::new(true);
        let mut count = 0usize;
        let outcome = run_search(dir.path(), &opts("needle", true), &cancel, |_| count += 1);
        assert_eq!(count, 0, "a cancelled search emits nothing");
        assert!(outcome.cancelled);
    }

    #[test]
    fn registry_start_supersedes_the_previous_search_for_that_window() {
        let mut registry = SearchRegistry::default();
        let (first_id, first_flag) = registry.start("main".to_string());
        let (second_id, second_flag) = registry.start("main".to_string());
        assert_ne!(first_id, second_id, "each search gets its own id");
        assert!(
            first_flag.load(std::sync::atomic::Ordering::Relaxed),
            "starting a new search cancels the previous one for that window"
        );
        assert!(!second_flag.load(std::sync::atomic::Ordering::Relaxed));

        // A different window is untouched.
        let (_, other_flag) = registry.start("window-1".to_string());
        registry.cancel("main");
        assert!(second_flag.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!other_flag.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn search_match_serializes_as_camel_case() {
        let json = serde_json::to_string(&SearchMatch {
            path: "/repo/a.rs".into(),
            relative_path: "a.rs".into(),
            line: 3,
            column: 5,
            preview: "let a = 1;".into(),
        })
        .expect("serialize");
        assert!(json.contains("\"relativePath\":\"a.rs\""), "got {json}");
    }
}
