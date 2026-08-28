//! Project mode: one absolute directory bound to one window.
//!
//! A project is opened explicitly — opening a file never creates one. The
//! registry maps a window label to its canonical root, so every project
//! command resolves through the CALLING window exactly the way `panel_host`
//! does, and window destruction drops the entry through the same
//! `WindowEvent::Destroyed` hook the other secondary-window registries use.

pub(crate) mod git_status;
pub(crate) mod recents;
pub(crate) mod search;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Manager;

/// One window's project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectState {
    pub root: PathBuf,
    pub opened_at_ms: u64,
}

/// window label → project. One project per window; one window per project
/// (the second half is enforced by `window_for_root` at open time, not by the
/// map, so a stale entry can never make a root permanently unopenable).
#[derive(Debug, Default)]
pub(crate) struct ProjectRegistry {
    by_window: HashMap<String, ProjectState>,
}

impl ProjectRegistry {
    pub(crate) fn bind(&mut self, label: String, root: PathBuf, now_ms: u64) {
        self.by_window.insert(
            label,
            ProjectState {
                root,
                opened_at_ms: now_ms,
            },
        );
    }

    pub(crate) fn get(&self, label: &str) -> Option<&ProjectState> {
        self.by_window.get(label)
    }

    // Not called from Task 1's own commands — later project-mode tasks
    // (search, git status, recents) resolve their window's root through this
    // method instead of duplicating `get(...).root.display()`.
    pub(crate) fn root_for(&self, label: &str) -> Option<String> {
        self.by_window
            .get(label)
            .map(|state| state.root.display().to_string())
    }

    pub(crate) fn remove(&mut self, label: &str) -> Option<ProjectState> {
        self.by_window.remove(label)
    }

    pub(crate) fn window_for_root(&self, root: &Path) -> Option<String> {
        self.by_window
            .iter()
            .find(|(_, state)| state.root == root)
            .map(|(label, _)| label.clone())
    }

    /// Atomically check-and-bind: the single `&mut self` call this method
    /// makes is what closes the TOCTOU window between "is this root already
    /// claimed" and "claim it" — `project_open` must call this exactly once
    /// per attempt, under one continuous `registry.lock()`, rather than a
    /// separate `window_for_root` check followed by a separate `bind` (two
    /// independent lock acquisitions let two concurrent callers for the same
    /// root both observe "unclaimed" and both reserve, double-opening the
    /// project). `label` is assumed already allocated by the caller —
    /// allocating a *label* is not racy (it is a monotonic counter); binding
    /// a *root* to one is.
    pub(crate) fn reserve(&mut self, label: String, root: PathBuf, now_ms: u64) -> ReserveOutcome {
        if let Some(existing) = self.window_for_root(&root) {
            return ReserveOutcome::Existing(existing);
        }
        self.bind(label, root, now_ms);
        ReserveOutcome::Reserved
    }
}

/// Result of [`ProjectRegistry::reserve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReserveOutcome {
    /// No window held this root; it is now bound to the label that was
    /// passed in.
    Reserved,
    /// Some window already holds this root — attach to it instead of
    /// building a second window for the same project.
    Existing(String),
}

/// Milliseconds since the epoch. A clock that has gone backwards yields 0
/// rather than panicking — a wrong timestamp on a recents entry is a cosmetic
/// ordering bug; a panic here would take the window open with it.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Canonicalize and verify. Canonicalization is what makes two spellings of
/// one directory (a symlink and its target) collapse onto ONE project rather
/// than opening two windows that fight over the same files.
pub(crate) fn canonical_root(path: &str) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(path).map_err(|e| format!("Cannot open {path}: {e}"))?;
    if !canonical.is_dir() {
        return Err(format!("Cannot open {path}: it is not a folder"));
    }
    std::fs::read_dir(&canonical).map_err(|e| format!("Cannot read {path}: {e}"))?;
    Ok(canonical)
}

/// The directory basename — the project's name, and the window title.
pub(crate) fn project_name(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectInfo {
    pub root: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectOpenResult {
    pub root: String,
    pub name: String,
    pub window_label: String,
    pub focused_existing: bool,
}

/// The program and arguments that reveal `path` in the platform's file
/// manager. Pure, so the per-platform argument shapes are unit-testable
/// without spawning anything. Only macOS and Windows can select the item
/// itself; elsewhere the containing folder is opened.
pub(crate) fn reveal_command(path: &str) -> (&'static str, Vec<String>) {
    if cfg!(target_os = "macos") {
        ("open", vec!["-R".to_string(), path.to_string()])
    } else if cfg!(target_os = "windows") {
        ("explorer", vec![format!("/select,{path}")])
    } else {
        let parent = Path::new(path)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| path.to_string());
        ("xdg-open", vec![parent])
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Open `path` as a project. If some window already holds the same canonical
/// root, that window is focused and nothing is created — one project per
/// window, one window per project. Otherwise the label is allocated and the
/// registry entry written BEFORE the window is built (the same seed-then-build
/// discipline `open_path::seed_then_build` documents), so the new window's
/// very first `get_saved_layout` already sees its project; a failed build
/// drains the entry back out.
///
/// `project_open` is an async command dispatched on Tauri's worker thread
/// pool, so two calls for the same root can genuinely run concurrently (a
/// double-invoked "Open Folder", the CLI queue listing one directory twice).
/// The check-existing / stale-eviction / reserve sequence below runs under
/// ONE continuous `registry.lock()` — never two separate lock acquisitions —
/// specifically so two such calls cannot both observe the root as unclaimed
/// and both go on to build a window for it. The window itself is still built
/// with the lock released (see the `run_on_main_thread` call below): only the
/// registry bookkeeping needs to be atomic, not the (slow, main-thread-only)
/// window construction.
#[tauri::command]
pub(crate) async fn project_open(
    app: tauri::AppHandle,
    path: String,
) -> Result<ProjectOpenResult, String> {
    let root = canonical_root(&path)?;
    let name = project_name(&root);
    let registry = app.state::<Mutex<ProjectRegistry>>();
    let label = crate::windows::allocate_window_label(&app);

    let outcome = {
        let mut guard = registry.lock();
        // A stale entry (its window died without `Destroyed` firing, e.g. an
        // OS kill) must not block a fresh reservation. Evict it here, inside
        // the same lock acquisition `reserve` runs under below, so no other
        // caller can slip a reservation in between the eviction and the
        // retry.
        if let Some(existing) = guard.window_for_root(&root)
            && app.get_webview_window(&existing).is_none()
        {
            guard.remove(&existing);
        }
        guard.reserve(label.clone(), root.clone(), now_ms())
    };

    let existing = match outcome {
        ReserveOutcome::Existing(existing) => existing,
        ReserveOutcome::Reserved => {
            return project_open_build(app, root, name, label).await;
        }
    };

    if let Some(win) = app.get_webview_window(&existing) {
        let _ = win.show();
        let _ = win.set_focus();
    }
    recents::remember(&root.display().to_string(), now_ms());
    Ok(ProjectOpenResult {
        root: root.display().to_string(),
        name,
        window_label: existing,
        focused_existing: true,
    })
}

/// The window-building half of [`project_open`], split out so the reserve
/// step above stays a small, easily-audited critical section. Only reached
/// once this call has already won the reservation for `root`.
async fn project_open_build(
    app: tauri::AppHandle,
    root: PathBuf,
    name: String,
    label: String,
) -> Result<ProjectOpenResult, String> {
    let handle = app.clone();
    let build_label = label.clone();
    let build_title = name.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let result =
            crate::windows::create_window_with_label_titled(&handle, &build_label, &build_title)
                .map_err(|e| e.to_string());
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;

    if let Err(e) = rx.recv().map_err(|e| e.to_string())? {
        app.state::<Mutex<ProjectRegistry>>().lock().remove(&label);
        return Err(e);
    }

    recents::remember(&root.display().to_string(), now_ms());
    Ok(ProjectOpenResult {
        root: root.display().to_string(),
        name,
        window_label: label,
        focused_existing: false,
    })
}

/// This window's project, resolved by the caller's own label — never by
/// parsing anything out of it.
#[tauri::command]
pub(crate) fn project_info(
    window: tauri::WebviewWindow,
    registry: tauri::State<'_, Mutex<ProjectRegistry>>,
) -> Option<ProjectInfo> {
    registry
        .lock()
        .get(window.label())
        .map(|state| ProjectInfo {
            root: state.root.display().to_string(),
            name: project_name(&state.root),
        })
}

/// The native directory picker. Blocking, and therefore never called on the
/// main thread: Tauri commands run on a thread pool, the same footing
/// `share_commands`' blocking file dialogs already rely on.
#[tauri::command]
pub(crate) fn project_pick_folder(app: tauri::AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|p| p.as_path().map(|p| p.display().to_string()))
}

/// Show `path` in the platform's file manager.
#[tauri::command]
pub(crate) fn project_reveal_path(path: String) -> Result<(), String> {
    let (program, args) = reveal_command(&path);
    std::process::Command::new(program)
        .args(&args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not reveal {path}: {e}"))
}

/// `WindowEvent::Destroyed`: drop this window's project. Idempotent — a
/// window that never had one removes nothing.
pub(crate) fn on_window_destroyed<R: tauri::Runtime>(window: &tauri::Window<R>) {
    let app = window.app_handle();
    if let Some(searches) = app.try_state::<Mutex<search::SearchRegistry>>() {
        searches.lock().cancel(window.label());
    }
    let Some(registry) = app.try_state::<Mutex<ProjectRegistry>>() else {
        return;
    };
    registry.lock().remove(window.label());
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectAdoptResult {
    pub adopted: Option<ProjectInfo>,
    pub focused_existing: bool,
}

/// Bind THIS window to a directory that the CLI/IPC queued into it.
///
/// Called from `startup-runtime.js` during `applyAppConfig` — before the
/// layout read and before any tool window registers — so the per-project
/// layout is already in effect on the window's first paint and the Search
/// tool window knows it has a project at registration time.
///
/// If another window already holds the root, that window is focused and this
/// one (a blank window the IPC layer created moments ago) destroys itself
/// rather than lingering empty. The check-existing / stale-eviction / reserve
/// sequence mirrors `project_open`'s and runs under the same ONE continuous
/// `registry.lock()` for the same reason: two windows racing to adopt the
/// same queued root (e.g. `termlab open /repo` invoked twice back to back,
/// each spawning its own window) must not both observe the root as
/// unclaimed and both bind — see `ProjectRegistry::reserve`'s doc comment.
#[tauri::command]
pub(crate) fn project_adopt_pending(
    window: tauri::WebviewWindow,
    pending: tauri::State<'_, crate::open_path::PendingOpens>,
    registry: tauri::State<'_, Mutex<ProjectRegistry>>,
) -> ProjectAdoptResult {
    let label = window.label().to_string();
    let dirs = pending.take_directories(&label, |p| Path::new(p).is_dir());
    let none = ProjectAdoptResult {
        adopted: None,
        focused_existing: false,
    };
    let Some(first) = dirs.first() else {
        return none;
    };
    if dirs.len() > 1 {
        log::warn!(
            "project: {} directories queued for {label}; opening only {first}",
            dirs.len()
        );
    }
    let Ok(root) = canonical_root(first) else {
        log::warn!("project: cannot adopt {first}");
        return none;
    };

    let app = window.app_handle().clone();
    let outcome = {
        let mut guard = registry.lock();
        // A stale entry (its window died without `Destroyed` firing) must
        // not block this window from adopting the root. Evict it here,
        // inside the same lock acquisition `reserve` runs under below.
        if let Some(existing) = guard.window_for_root(&root)
            && app.get_webview_window(&existing).is_none()
        {
            guard.remove(&existing);
        }
        guard.reserve(label, root.clone(), now_ms())
    };

    match outcome {
        ReserveOutcome::Reserved => {
            recents::remember(&root.display().to_string(), now_ms());
            ProjectAdoptResult {
                adopted: Some(ProjectInfo {
                    root: root.display().to_string(),
                    name: project_name(&root),
                }),
                focused_existing: false,
            }
        }
        ReserveOutcome::Existing(existing) => {
            if let Some(win) = app.get_webview_window(&existing) {
                let _ = win.show();
                let _ = win.set_focus();
            }
            let _ = window.destroy();
            ProjectAdoptResult {
                adopted: None,
                focused_existing: true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn reserve_lets_exactly_one_racing_caller_win_and_the_loser_gets_the_winners_label() {
        // Two threads race to reserve the SAME root, synchronized with a
        // barrier so both cross the check-then-bind boundary as close
        // together as the scheduler allows. `reserve` must hold ONE lock
        // across the whole check-and-bind (see project_open), so exactly one
        // of them can observe the root unclaimed; the other must attach to
        // the winner instead of both reserving separately.
        let registry = Arc::new(Mutex::new(ProjectRegistry::default()));
        let root = PathBuf::from("/repo");
        let barrier = Arc::new(Barrier::new(2));
        let labels = ["window-a", "window-b"];

        let handles: Vec<_> = labels
            .iter()
            .map(|label| {
                let registry = Arc::clone(&registry);
                let barrier = Arc::clone(&barrier);
                let root = root.clone();
                let label = label.to_string();
                thread::spawn(move || {
                    barrier.wait();
                    (label.clone(), registry.lock().reserve(label, root, 1))
                })
            })
            .collect();

        let results: Vec<(String, ReserveOutcome)> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        let winners: Vec<&str> = results
            .iter()
            .filter(|(_, outcome)| matches!(outcome, ReserveOutcome::Reserved))
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "exactly one racing caller reserves the root"
        );
        let winner_label = winners[0];

        let losers: Vec<&ReserveOutcome> = results
            .iter()
            .filter(|(_, outcome)| matches!(outcome, ReserveOutcome::Existing(_)))
            .map(|(_, outcome)| outcome)
            .collect();
        assert_eq!(losers.len(), 1, "the other caller must lose, not also win");
        match losers[0] {
            ReserveOutcome::Existing(label) => assert_eq!(
                label, winner_label,
                "the loser attaches to the winner's window, not a third one"
            ),
            ReserveOutcome::Reserved => unreachable!("filtered to Existing above"),
        }

        assert_eq!(
            registry.lock().window_for_root(&root).as_deref(),
            Some(winner_label),
            "only the winner's label ends up bound to the root"
        );
    }

    #[test]
    fn reserve_on_an_unclaimed_root_binds_it_and_reports_reserved() {
        let mut reg = ProjectRegistry::default();
        let outcome = reg.reserve("window-1".into(), PathBuf::from("/repo"), 5);
        assert_eq!(outcome, ReserveOutcome::Reserved);
        assert_eq!(reg.root_for("window-1").as_deref(), Some("/repo"));
    }

    #[test]
    fn reserve_on_an_already_claimed_root_reports_the_existing_window_and_does_not_rebind() {
        let mut reg = ProjectRegistry::default();
        assert_eq!(
            reg.reserve("window-1".into(), PathBuf::from("/repo"), 5),
            ReserveOutcome::Reserved
        );
        assert_eq!(
            reg.reserve("window-2".into(), PathBuf::from("/repo"), 6),
            ReserveOutcome::Existing("window-1".to_string()),
            "a second reservation for the same root attaches to the first window"
        );
        assert!(
            reg.get("window-2").is_none(),
            "the losing label must not end up bound to anything"
        );
    }

    #[test]
    fn a_failed_build_after_reservation_leaves_no_stuck_entry() {
        // Mirrors project_open's error path: reserve, then simulate the
        // window build failing by draining the entry the same way
        // `registry.lock().remove(&label)` does on that path. The root must
        // become reservable again immediately — never permanently stuck
        // pointing at a window that was never actually built.
        let mut reg = ProjectRegistry::default();
        let outcome = reg.reserve("window-3".into(), PathBuf::from("/repo"), 5);
        assert_eq!(outcome, ReserveOutcome::Reserved);

        reg.remove("window-3");
        assert!(
            reg.get("window-3").is_none(),
            "a failed build must not leave a phantom reservation"
        );

        let retry = reg.reserve("window-4".into(), PathBuf::from("/repo"), 6);
        assert_eq!(
            retry,
            ReserveOutcome::Reserved,
            "the root is reservable again once the failed build's entry is drained"
        );
    }

    #[test]
    fn bind_get_and_remove_are_per_window() {
        let mut reg = ProjectRegistry::default();
        reg.bind("main".into(), PathBuf::from("/repo"), 100);
        reg.bind("window-1".into(), PathBuf::from("/other"), 200);
        assert_eq!(
            reg.get("main").map(|s| s.root.clone()),
            Some(PathBuf::from("/repo"))
        );
        assert_eq!(reg.root_for("window-1").as_deref(), Some("/other"));
        assert_eq!(reg.remove("main").map(|s| s.opened_at_ms), Some(100));
        assert!(reg.get("main").is_none(), "remove drops the entry");
        assert!(
            reg.root_for("window-9").is_none(),
            "unknown label has no project"
        );
    }

    #[test]
    fn window_for_root_finds_the_window_already_holding_it() {
        let mut reg = ProjectRegistry::default();
        reg.bind("window-2".into(), PathBuf::from("/repo"), 1);
        assert_eq!(
            reg.window_for_root(Path::new("/repo")).as_deref(),
            Some("window-2")
        );
        assert!(
            reg.window_for_root(Path::new("/repo/src")).is_none(),
            "a subdirectory is a different project"
        );
    }

    #[test]
    fn rebinding_a_label_replaces_rather_than_duplicates() {
        let mut reg = ProjectRegistry::default();
        reg.bind("main".into(), PathBuf::from("/a"), 1);
        reg.bind("main".into(), PathBuf::from("/b"), 2);
        assert_eq!(reg.root_for("main").as_deref(), Some("/b"));
        assert!(
            reg.window_for_root(Path::new("/a")).is_none(),
            "the old root is no longer held"
        );
    }

    #[test]
    fn canonical_root_collapses_a_symlinked_root_to_one_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("mkdir");
        let link = dir.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        #[cfg(not(unix))]
        std::os::windows::fs::symlink_dir(&real, &link).expect("symlink");
        let a = canonical_root(real.to_str().unwrap()).expect("real resolves");
        let b = canonical_root(link.to_str().unwrap()).expect("link resolves");
        assert_eq!(a, b, "a symlinked root must collapse onto the same project");
    }

    #[test]
    fn canonical_root_rejects_a_missing_path_and_a_plain_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope");
        assert!(
            canonical_root(missing.to_str().unwrap()).is_err(),
            "missing path is an error, not a window"
        );
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").expect("write");
        let err = canonical_root(file.to_str().unwrap()).expect_err("a file is not a project");
        assert!(
            err.contains("not a folder"),
            "the message names the real problem: {err}"
        );
    }

    #[test]
    fn project_name_is_the_basename() {
        assert_eq!(project_name(Path::new("/home/dustin/conch")), "conch");
        assert_eq!(
            project_name(Path::new("/")),
            "/",
            "a rootless path falls back to its display form"
        );
    }

    #[test]
    fn adopt_result_serializes_as_camel_case() {
        let json = serde_json::to_string(&ProjectAdoptResult {
            adopted: Some(ProjectInfo {
                root: "/repo".into(),
                name: "repo".into(),
            }),
            focused_existing: false,
        })
        .expect("serialize");
        assert!(json.contains("\"focusedExisting\":false"), "got {json}");
        assert!(json.contains("\"root\":\"/repo\""), "got {json}");
    }

    #[test]
    fn reveal_command_selects_the_path_where_the_platform_can() {
        let (program, args) = reveal_command("/repo/src/main.rs");
        if cfg!(target_os = "macos") {
            assert_eq!(program, "open");
            assert_eq!(
                args,
                vec!["-R".to_string(), "/repo/src/main.rs".to_string()]
            );
        } else if cfg!(target_os = "windows") {
            assert_eq!(program, "explorer");
            assert_eq!(args, vec!["/select,/repo/src/main.rs".to_string()]);
        } else {
            assert_eq!(program, "xdg-open");
            assert_eq!(
                args,
                vec!["/repo/src".to_string()],
                "no select on xdg-open: open the parent"
            );
        }
    }

    // --- F9: remember() actually fires on every successful bind path ------
    //
    // `project_open`/`project_open_build`/`project_adopt_pending` are async
    // Tauri commands that build a real window — not unit-testable in
    // isolation without a live app handle. `remember()` itself writes
    // straight to `~/.config/termlab/state.toml` (no path-injection seam
    // exists in termlab_core to redirect it to a tempdir), so exercising it
    // end to end from here would either pollute a real user's state file or
    // require a much larger seam than this fix round's scope. Following the
    // same precedent `termlab_remote::ssh::key_auth_log_messages_never_include_private_key_paths`
    // sets for exactly this situation (a production behavior that can only
    // be pinned by reading the source, not by executing it in a test), this
    // asserts the wiring is present at all three bind sites by source
    // inspection instead.
    #[test]
    fn recents_are_remembered_on_every_project_open_and_adopt_path() {
        let full_source = include_str!("mod.rs");
        // Non-test source only — the test module below quotes this exact
        // string as a literal, which would otherwise self-count.
        let source = &full_source[..full_source
            .find("#[cfg(test)]")
            .expect("this file has a test module")];
        let call = "recents::remember(&root.display().to_string(), now_ms());";
        let occurrences = source.matches(call).count();
        assert_eq!(
            occurrences, 3,
            "one remember() call in project_open's focused-existing branch, \
             one in project_open_build's success path, and one in \
             project_adopt_pending's Reserved arm — found {occurrences}"
        );

        let adopt_fn_start = source
            .find("pub(crate) fn project_adopt_pending")
            .expect("project_adopt_pending must exist");
        assert!(
            source[adopt_fn_start..].contains(call),
            "project_adopt_pending must remember the project it just bound"
        );

        let open_build_fn_start = source
            .find("async fn project_open_build")
            .expect("project_open_build must exist");
        let open_build_fn_end = source[open_build_fn_start..]
            .find("\n#[tauri::command]")
            .map(|rel| open_build_fn_start + rel)
            .unwrap_or(source.len());
        assert!(
            source[open_build_fn_start..open_build_fn_end].contains(call),
            "project_open_build must remember a freshly opened project"
        );

        let open_fn_start = source
            .find("pub(crate) async fn project_open(")
            .expect("project_open must exist");
        assert!(
            source[open_fn_start..open_build_fn_start].contains(call),
            "project_open's focused-existing branch must remember the project too"
        );
    }
}
