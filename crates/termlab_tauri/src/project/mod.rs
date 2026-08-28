//! Project mode: one absolute directory bound to one window.
//!
//! A project is opened explicitly — opening a file never creates one. The
//! registry maps a window label to its canonical root, so every project
//! command resolves through the CALLING window exactly the way `panel_host`
//! does, and window destruction drops the entry through the same
//! `WindowEvent::Destroyed` hook the other secondary-window registries use.

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

    // Not called from Task 1's own commands — the project-mode plan's later
    // tasks (search, git status, recents) resolve their window's root through
    // this method instead of duplicating `get(...).root.display()`.
    #[allow(dead_code)]
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
#[tauri::command]
pub(crate) async fn project_open(
    app: tauri::AppHandle,
    path: String,
) -> Result<ProjectOpenResult, String> {
    let root = canonical_root(&path)?;
    let name = project_name(&root);
    let registry = app.state::<Mutex<ProjectRegistry>>();

    if let Some(existing) = registry.lock().window_for_root(&root) {
        if let Some(win) = app.get_webview_window(&existing) {
            let _ = win.show();
            let _ = win.set_focus();
            return Ok(ProjectOpenResult {
                root: root.display().to_string(),
                name,
                window_label: existing,
                focused_existing: true,
            });
        }
        // The entry outlived its window (an OS kill that skipped Destroyed).
        registry.lock().remove(&existing);
    }

    let label = crate::windows::allocate_window_label(&app);
    registry.lock().bind(label.clone(), root.clone(), now_ms());

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
    let Some(registry) = app.try_state::<Mutex<ProjectRegistry>>() else {
        return;
    };
    registry.lock().remove(window.label());
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
