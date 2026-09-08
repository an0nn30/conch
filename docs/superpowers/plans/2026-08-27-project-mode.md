# Project Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Opening a directory turns a TermLab window into a lightweight IDE for that directory — project tree in the Files panel, one trust decision, project-wide text search, git tints, and a recent-projects list.

**Architecture:** A new Rust `project/` module owns a per-window registry (window label → canonical root) plus the walker, git parser and recents helpers around it; every project command resolves its root through the calling window the same way `panel_host` does. The frontend gains a small `features/project/` family (mode resolver, trust banner, git rollup) and two panels (`project-tree.js`, `project-search-panel.js`), all IIFE globals wired in through the existing files panel, tool-window runtime, command palette and keyboard tables.

**Tech Stack:** Rust / Tauri v2 (`#[tauri::command]`, `Emitter::emit_to`, `parking_lot::Mutex` registries, `tempfile` dev-dep), the `ignore` crate for the search walker, vanilla-JS IIFE frontend modules with `window.*` globals, design-system CSS custom properties, node VM suites under `scripts/tests/`.

**Spec:** `docs/superpowers/specs/2026-08-27-project-mode-design.md`

## Global Constraints

- **Never commit or push to `main`.** All work happens on `codex/lsp-completion` in the worktree `/Users/dustin/projects/conch/.worktrees/lsp-completion`. Verify with `git branch --show-current` before every commit.
- TDD, red first: every task writes the failing test, **runs it and sees it fail**, then implements, then re-runs green, then commits. No step may be skipped.
- No `Co-Authored-By` lines. Commit messages are imperative mood (`feat: …`, `fix: …`, `chore: …`).
- Frontend modules are self-contained IIFEs exposing one `window.*` global; never define local copies of `window.utils.esc` / `attr`.
- CSS uses design-system custom properties only — **no hex literals** in `styles/design-system/components/**` (the boundary script fails on them). Semantic aliases may be added to `styles/design-system/base.css`, which is outside that scan.
- All user-facing messages go through `window.toast` (`error` / `success` / `info` / `warn`). Never `alert()` or `confirm()`.
- Keyboard handling goes through the capture-phase router (`global.termlabKeyboardRouter.register`) or the shortcut-runtime fallback tables. **No `document.addEventListener('keydown', …)` outside `app/core/keyboard-router.js`** — the boundary script fails on it.
- **NO regex lookbehind (`(?<=` / `(?<!`) and NO raw control bytes** anywhere in frontend source. A lookbehind is a parse-time `SyntaxError` on the older WKWebView (it costs the *whole file*, not just the regex), and a control byte makes git treat the file as binary. Every new module must be added to the lookbehind and control-byte source-scan guards in its suite.
- Keys that vim consumes go through the Vim API (`app/features/editor/vim-mode.js`), never a DOM handler.
- Rust: `pub(crate)` visibility (not `pub`) for everything internal; `#[serde(default)]` on persisted structs; `if let` / `match` over `.unwrap()`; `log::warn!` / `log::error!` for recoverable failures.
- New dependencies: **the `ignore` crate only** (added and justified in Task 8). Nothing else.
- `bash scripts/check_frontend_boundaries.sh`: the only allowed failure is the pre-existing `tl-dialog.js:334` keydown finding.
- Rust warnings: 24 rustc warnings is the accepted baseline — do not increase it. `cargo clippy --all-targets` must introduce no new warnings.
- Node suites run as `node scripts/tests/<file>.mjs`. VM realm rules: JSON-round-trip cross-realm values before `deepStrictEqual` (`const plain = (v) => JSON.parse(JSON.stringify(v === undefined ? null : v))`); **do not define `sandbox.global`** — `tool-window-manager.js` binds `exports` and a helpfully-supplied `global` would hide a bare-`global.` regression.
- After every task: the task's own suite, **plus** the full sweep `for f in scripts/tests/*.mjs; do node "$f" >/dev/null 2>&1 || echo "FAIL: $f"; done` (must print nothing). Rust tasks also run `cargo test -p termlab_tauri --quiet`.

## Resolved Spec Ambiguities

Recorded here so an executor does not re-litigate them:

1. **Two entry points, not one.** `project_adopt_pending` (boot path, called by `startup-runtime.js`) binds a window that already exists because the CLI/IPC queued a directory into it; `project_open` (menu, palette, recents, and `routeOne`'s replaced directory branch) creates or focuses a project window. Adopting during `applyAppConfig` — before the layout read and before the tool-window runtime registers anything — is what lets `get_saved_layout` return the per-project layout and lets the Search tool window know it has a project at registration time.
2. **Window title.** The spec says the basename "becomes the window title", but `tab-manager.js:updateWindowTitle` rewrites the title on every tab activation. Resolution: `project_open` builds the window with `.title(name)` for pre-boot identity, and `updateWindowTitle` prefixes `"<name> — "` whenever `window.__termlabProjectName` is set, so the name survives tab switching.
3. **"Reveal in Finder"** has no existing command in this repo. Implemented as `project_reveal_path` with a pure platform-argument function (`reveal_command`), labelled "Reveal in File Manager" so the copy is honest on Linux/Windows.
4. **"New file"** has no existing files-panel local op either; added as `doNewFile` next to `doNewFolder`, over the existing `editor_write_file` command.
5. **A fourth `project/` file.** The spec lists `mod.rs`, `git_status.rs`, `search.rs`. Recents/layout helpers go in `project/recents.rs` rather than swelling `mod.rs`, per CLAUDE.md's no-monolith rule.
6. **Per-project layout save cadence.** The spec says "saved on project-window close"; `save_window_layout` is already debounced and called on close, resize and visibility change, so the project entry is written there instead of only at close. A project window writes **only** its project entry, so a project layout can never leak into ordinary windows.
7. **Search terminal event.** One event name, `project-search-results`, carrying `done` and `capped` flags on the terminal emission — the spec names only that event, so a second name would be an invention.

---

### Task 1: Rust — the project registry, `project_open`, `project_info`

**Files:**
- Create: `crates/termlab_tauri/src/project/mod.rs`
- Modify: `crates/termlab_tauri/src/windows.rs` (add `create_window_with_label_titled`, delegate `create_window_with_label:120`)
- Modify: `crates/termlab_tauri/src/lib.rs` (`mod` list ~line 22, `.manage(` chain ~line 363, `Destroyed` hook ~line 836, `generate_handler!` ~line 899)
- Test: `#[cfg(test)] mod tests` at the bottom of `crates/termlab_tauri/src/project/mod.rs`

**Interfaces:**
- Consumes: `crate::windows::allocate_window_label(&AppHandle) -> String`; `crate::windows::create_window_with_label(&AppHandle, &str) -> tauri::Result<()>`.
- Produces:
  - `pub(crate) struct ProjectState { pub root: PathBuf, pub opened_at_ms: u64 }`
  - `pub(crate) struct ProjectRegistry` with `bind(&mut self, label: String, root: PathBuf, now_ms: u64)`, `get(&self, label: &str) -> Option<&ProjectState>`, `root_for(&self, label: &str) -> Option<String>`, `remove(&mut self, label: &str) -> Option<ProjectState>`, `window_for_root(&self, root: &Path) -> Option<String>`
  - `pub(crate) fn canonical_root(path: &str) -> Result<PathBuf, String>`
  - `pub(crate) fn project_name(root: &Path) -> String`
  - `pub(crate) fn now_ms() -> u64`
  - commands `project_open(app, path: String) -> Result<ProjectOpenResult, String>`, `project_info(window, registry) -> Option<ProjectInfo>`, `project_pick_folder(app) -> Option<String>`, `project_reveal_path(path: String) -> Result<(), String>`
  - `pub(crate) struct ProjectInfo { root: String, name: String }` (serde camelCase)
  - `pub(crate) struct ProjectOpenResult { root: String, name: String, window_label: String, focused_existing: bool }` (serde camelCase)
  - `pub(crate) fn on_window_destroyed<R: tauri::Runtime>(window: &tauri::Window<R>)`
  - `pub(crate) fn reveal_command(path: &str) -> (&'static str, Vec<String>)`

- [ ] **Step 1: Write the failing tests.** Create `crates/termlab_tauri/src/project/mod.rs` containing ONLY the test module below plus the `use` line, so the file exists and every referenced item is missing:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_get_and_remove_are_per_window() {
        let mut reg = ProjectRegistry::default();
        reg.bind("main".into(), PathBuf::from("/repo"), 100);
        reg.bind("window-1".into(), PathBuf::from("/other"), 200);
        assert_eq!(reg.get("main").map(|s| s.root.clone()), Some(PathBuf::from("/repo")));
        assert_eq!(reg.root_for("window-1").as_deref(), Some("/other"));
        assert_eq!(reg.remove("main").map(|s| s.opened_at_ms), Some(100));
        assert!(reg.get("main").is_none(), "remove drops the entry");
        assert!(reg.root_for("window-9").is_none(), "unknown label has no project");
    }

    #[test]
    fn window_for_root_finds_the_window_already_holding_it() {
        let mut reg = ProjectRegistry::default();
        reg.bind("window-2".into(), PathBuf::from("/repo"), 1);
        assert_eq!(reg.window_for_root(Path::new("/repo")).as_deref(), Some("window-2"));
        assert!(reg.window_for_root(Path::new("/repo/src")).is_none(), "a subdirectory is a different project");
    }

    #[test]
    fn rebinding_a_label_replaces_rather_than_duplicates() {
        let mut reg = ProjectRegistry::default();
        reg.bind("main".into(), PathBuf::from("/a"), 1);
        reg.bind("main".into(), PathBuf::from("/b"), 2);
        assert_eq!(reg.root_for("main").as_deref(), Some("/b"));
        assert!(reg.window_for_root(Path::new("/a")).is_none(), "the old root is no longer held");
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
        assert!(canonical_root(missing.to_str().unwrap()).is_err(), "missing path is an error, not a window");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").expect("write");
        let err = canonical_root(file.to_str().unwrap()).expect_err("a file is not a project");
        assert!(err.contains("not a folder"), "the message names the real problem: {err}");
    }

    #[test]
    fn project_name_is_the_basename() {
        assert_eq!(project_name(Path::new("/home/dustin/conch")), "conch");
        assert_eq!(project_name(Path::new("/")), "/", "a rootless path falls back to its display form");
    }

    #[test]
    fn reveal_command_selects_the_path_where_the_platform_can() {
        let (program, args) = reveal_command("/repo/src/main.rs");
        if cfg!(target_os = "macos") {
            assert_eq!(program, "open");
            assert_eq!(args, vec!["-R".to_string(), "/repo/src/main.rs".to_string()]);
        } else if cfg!(target_os = "windows") {
            assert_eq!(program, "explorer");
            assert_eq!(args, vec!["/select,/repo/src/main.rs".to_string()]);
        } else {
            assert_eq!(program, "xdg-open");
            assert_eq!(args, vec!["/repo/src".to_string()], "no select on xdg-open: open the parent");
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_tauri --quiet project::`
Expected: compile FAIL — `cannot find type ProjectRegistry`, `cannot find function canonical_root`, and `file not found for module project` until the `mod` line is added.

- [ ] **Step 3: Write the implementation.** Prepend to `crates/termlab_tauri/src/project/mod.rs` (above the test module):

```rust
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
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| format!("Cannot open {path}: {e}"))?;
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
    registry
        .lock()
        .bind(label.clone(), root.clone(), now_ms());

    let handle = app.clone();
    let build_label = label.clone();
    let build_title = name.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let result = crate::windows::create_window_with_label_titled(
            &handle,
            &build_label,
            &build_title,
        )
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
    registry.lock().get(window.label()).map(|state| ProjectInfo {
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
```

- [ ] **Step 4: Wire it into the app.** Four edits:

`crates/termlab_tauri/src/lib.rs`, in the `mod` list (keep alphabetical, between `plugins` and `pty`):

```rust
pub(crate) mod project;
```

`crates/termlab_tauri/src/lib.rs`, after `.manage(open_path::PendingOpens::default())`:

```rust
        .manage(Mutex::new(project::ProjectRegistry::default()))
```

`crates/termlab_tauri/src/lib.rs`, in the `Destroyed` arm, immediately after `panel_host::on_window_destroyed(window);`:

```rust
                // A project belongs to the window that opened it and to
                // nothing else: dropping the entry here is what lets the same
                // root be opened again in a fresh window.
                project::on_window_destroyed(window);
```

`crates/termlab_tauri/src/lib.rs`, in `generate_handler![`, after the `panel_host::` block:

```rust
                project::project_open,
                project::project_info,
                project::project_pick_folder,
                project::project_reveal_path,
```

- [ ] **Step 5: Add the titled window builder.** In `crates/termlab_tauri/src/windows.rs`, replace the `create_window_with_label` body's `WebviewWindowBuilder` title and add the delegating pair:

```rust
/// Build a window under an already-allocated `label`, carrying `title` as its
/// OS window title. Project windows open named after the project so the
/// window is identifiable in the OS window list before its frontend boots.
///
/// Must run on the main thread (see [`open_new_window`]'s note): the
/// builder's `build()` posts to the main thread and waits.
pub(crate) fn create_window_with_label_titled<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    label: &str,
    title: &str,
) -> tauri::Result<()> {
```

…then inside that function change the builder line from `.title("TermLab")` to `.title(title)`, and add below it:

```rust
/// The untitled form every non-project caller wants.
pub(crate) fn create_window_with_label<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    label: &str,
) -> tauri::Result<()> {
    create_window_with_label_titled(app, label, "TermLab")
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p termlab_tauri --quiet project::`
Expected: PASS (7 tests).
Run: `cargo test --workspace --quiet` — PASS.
Run: `cargo clippy --all-targets 2>&1 | grep -c '^warning'` — no increase over the 24-warning baseline.

- [ ] **Step 7: Commit**

```bash
git add crates/termlab_tauri/src/project/mod.rs crates/termlab_tauri/src/windows.rs crates/termlab_tauri/src/lib.rs
git commit -m "feat: add the project registry and project_open/project_info commands"
```

---

### Task 2: Rust — a starting directory for terminal tabs, and pending-open classification

**Files:**
- Modify: `crates/termlab_tauri/src/pty_backend.rs` (`PtyBackend::new:53`)
- Modify: `crates/termlab_tauri/src/pty.rs` (`spawn_shell_for_pane:52`, `spawn_shell:114`, `spawn_default_shell:142`)
- Modify: `crates/termlab_tauri/src/open_path.rs` (`peek:30`, add `classify` + `pending_open_paths_kind`, remove `has_pending_open_paths`)
- Modify: `crates/termlab_tauri/src/project/mod.rs` (add `project_adopt_pending`)
- Modify: `crates/termlab_tauri/src/lib.rs` (`generate_handler!`)
- Test: `#[cfg(test)] mod tests` in `open_path.rs` and in `project/mod.rs`

**Interfaces:**
- Consumes: `ProjectRegistry::{bind, window_for_root, remove}`, `canonical_root`, `project_name`, `now_ms`, `ProjectInfo` (Task 1).
- Produces:
  - `spawn_shell(window, app, state, pane_id, cols, rows, cwd: Option<String>)` — the frontend sends `cwd`; an absent/unreadable directory falls back to the process default rather than failing the spawn.
  - `pub(crate) enum PendingKind { None, Files, Project }` with `as_str(&self) -> &'static str`
  - `pub(crate) fn classify(paths: &[String], is_dir: impl Fn(&str) -> bool) -> PendingKind`
  - command `pending_open_paths_kind(window, state) -> String` (`"none" | "files" | "project"`), replacing `has_pending_open_paths`
  - command `project_adopt_pending(window, pending, registry, app) -> ProjectAdoptResult { adopted: Option<ProjectInfo>, focused_existing: bool }` (serde camelCase)
  - `PendingOpens::peek(&self, label: &str) -> Vec<String>` is now `pub(crate)`
  - `PendingOpens::take_directories(&self, label: &str, is_dir: impl Fn(&str) -> bool) -> Vec<String>` — drains only the directory entries, leaving files queued for the editor

- [ ] **Step 1: Write the failing tests.** Append to `open_path.rs`'s existing `mod tests`:

```rust
    #[test]
    fn classify_reports_none_files_or_project() {
        let dirs = |p: &str| p.ends_with('/');
        assert_eq!(classify(&[], dirs), PendingKind::None);
        assert_eq!(classify(&["/a.txt".to_string()], dirs), PendingKind::Files);
        assert_eq!(classify(&["/repo/".to_string()], dirs), PendingKind::Project);
        // A mixed queue is a FILE queue: the window becomes a zen editor
        // window for the file, and the directory is handled separately.
        assert_eq!(
            classify(&["/repo/".to_string(), "/a.txt".to_string()], dirs),
            PendingKind::Files
        );
    }

    #[test]
    fn take_directories_drains_only_the_directories() {
        let dirs = |p: &str| p.ends_with('/');
        let pending = PendingOpens::default();
        seed_for_label(
            &pending,
            "main",
            vec!["/repo/".into(), "/a.txt".into(), "/other/".into()],
        );
        assert_eq!(
            pending.take_directories("main", dirs),
            vec!["/repo/".to_string(), "/other/".to_string()]
        );
        assert_eq!(
            pending.peek("main"),
            vec!["/a.txt".to_string()],
            "files stay queued for the editor"
        );
        assert!(
            pending.take_directories("main", dirs).is_empty(),
            "a second drain finds no directories"
        );
    }
```

…and append to `project/mod.rs`'s `mod tests`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_tauri --quiet open_path:: project::`
Expected: compile FAIL — `cannot find function classify`, `no method named take_directories`, `cannot find type ProjectAdoptResult`.

- [ ] **Step 3: Implement the pending-open classification.** In `open_path.rs`, change `peek`'s visibility and add the new items above the `#[tauri::command]` block:

```rust
    /// Non-destructive read of a label's whole queue.
    pub(crate) fn peek(&self, label: &str) -> Vec<String> {
        self.0.lock().unwrap().get(label).cloned().unwrap_or_default()
    }

    /// Drain ONLY the directory entries, leaving files queued. The boot path
    /// adopts a queued directory as this window's project (project mode) while
    /// the editor still gets any files that arrived alongside it.
    pub(crate) fn take_directories(
        &self,
        label: &str,
        is_dir: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let mut guard = self.0.lock().unwrap();
        let Some(queue) = guard.get_mut(label) else {
            return Vec::new();
        };
        let mut dirs = Vec::new();
        queue.retain(|path| {
            if is_dir(path) {
                dirs.push(path.clone());
                false
            } else {
                true
            }
        });
        dirs
    }
```

(Delete the old `#[cfg(test)] fn peek`.) Then, still in `open_path.rs`:

```rust
/// What a window's queued open-paths amount to, as far as the boot layout is
/// concerned. A window opening a FILE becomes a zen, editor-only window; a
/// window opening a PROJECT keeps its panels and gets a terminal tab.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PendingKind {
    None,
    Files,
    Project,
}

impl PendingKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            PendingKind::None => "none",
            PendingKind::Files => "files",
            PendingKind::Project => "project",
        }
    }
}

/// Pure classifier — `is_dir` is injected so the rule is testable without
/// touching the filesystem. A queue holding even one non-directory is a FILE
/// queue: the file's editor window is the stronger claim on the layout.
pub(crate) fn classify(paths: &[String], is_dir: impl Fn(&str) -> bool) -> PendingKind {
    if paths.is_empty() {
        return PendingKind::None;
    }
    if paths.iter().all(|p| is_dir(p)) {
        PendingKind::Project
    } else {
        PendingKind::Files
    }
}
```

Replace the `has_pending_open_paths` command with:

```rust
/// Non-destructive peek used by the boot layout decision. Replaces the older
/// boolean `has_pending_open_paths`: a directory and a file want opposite
/// window shapes, so a yes/no answer is no longer enough.
#[tauri::command]
pub(crate) fn pending_open_paths_kind(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, PendingOpens>,
) -> String {
    let paths = state.peek(window.label());
    classify(&paths, |p| std::path::Path::new(p).is_dir())
        .as_str()
        .to_string()
}
```

In `lib.rs`'s `generate_handler!`, replace `open_path::has_pending_open_paths,` with `open_path::pending_open_paths_kind,`.

Delete the now-stale `has_reports_without_draining` test's second half only if it fails to compile — `has()` itself stays (it is still used by nothing else, so also delete `PendingOpens::has` and that test, and remove the `has()` mention from the struct doc comment).

- [ ] **Step 4: Implement `project_adopt_pending`.** Append to `crates/termlab_tauri/src/project/mod.rs` (above the test module):

```rust
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
/// rather than lingering empty.
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

    let existing = registry.lock().window_for_root(&root);
    if let Some(existing) = existing
        && existing != label
    {
        let app = window.app_handle().clone();
        if let Some(win) = app.get_webview_window(&existing) {
            let _ = win.show();
            let _ = win.set_focus();
            let _ = window.destroy();
            return ProjectAdoptResult {
                adopted: None,
                focused_existing: true,
            };
        }
        registry.lock().remove(&existing);
    }

    registry.lock().bind(label, root.clone(), now_ms());
    ProjectAdoptResult {
        adopted: Some(ProjectInfo {
            root: root.display().to_string(),
            name: project_name(&root),
        }),
        focused_existing: false,
    }
}
```

Register it in `lib.rs`'s `generate_handler!` beside the other project commands:

```rust
                project::project_adopt_pending,
```

- [ ] **Step 5: Thread `cwd` into the PTY.** In `pty_backend.rs`, add the parameter and use it:

```rust
    pub fn new(
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        shell_args: &[String],
        extra_env: &HashMap<String, String>,
        clear_tmux_env: bool,
        cwd: Option<&str>,
    ) -> Result<Self> {
```

…and immediately after the `for arg in shell_args { cmd.arg(arg); }` loop:

```rust
        // A project window's first terminal starts at the project root. An
        // unreadable directory is dropped rather than failing the spawn: a
        // shell in the wrong directory is a nuisance, a window with no shell
        // at all is a broken window.
        if let Some(dir) = cwd
            && Path::new(dir).is_dir()
        {
            cmd.cwd(dir);
        }
```

Add `use std::path::Path;` to the file's imports.

In `pty.rs`, add `cwd: Option<String>` as the last parameter of `spawn_shell_for_pane`, pass `cwd.as_deref()` into `PtyBackend::new`, add `cwd: Option<String>` as the last parameter of the `spawn_shell` command and forward it, and pass `None` from `spawn_default_shell`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p termlab_tauri --quiet` — PASS.
Run: `cargo test --workspace --quiet` — PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/termlab_tauri/src/pty_backend.rs crates/termlab_tauri/src/pty.rs crates/termlab_tauri/src/open_path.rs crates/termlab_tauri/src/project/mod.rs crates/termlab_tauri/src/lib.rs
git commit -m "feat: adopt a queued directory as the window project and start terminals at a cwd"
```

---

### Task 3: Frontend — boot a project window (routing seam, zen gate, terminal at root, title)

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/project/project-mode.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/open-path-routing.js` (the `DIRECTORY_COMING_SOON` branch, lines 23 and 42-45)
- Modify: `crates/termlab_tauri/frontend/app/event-wiring-runtime.js` (`wirePendingOpenDrain:19-30`)
- Modify: `crates/termlab_tauri/frontend/app/startup-runtime.js` (the zen block, lines 191-209)
- Modify: `crates/termlab_tauri/frontend/app/main-runtime.js` (the first-tab block, lines 456-474)
- Modify: `crates/termlab_tauri/frontend/app/tab-manager.js` (`createTab:550`, `updateWindowTitle:154`)
- Modify: `crates/termlab_tauri/frontend/app/manager-compose-runtime.js` (both `spawnShell` deps, lines 136 and 273)
- Modify: `crates/termlab_tauri/frontend/index.html` (script tag)
- Modify: `scripts/tests/test_open_path_routing.mjs`, `scripts/tests/test_open_path_wiring.mjs` (the coming-soon assertions)
- Test: `scripts/tests/test_project_mode.mjs` (new)

**Interfaces:**
- Consumes: `project_adopt_pending -> { adopted: { root, name } | null, focusedExisting }`, `pending_open_paths_kind -> "none"|"files"|"project"`, `project_open(path) -> { root, name, windowLabel, focusedExisting }`, `spawn_shell(paneId, cols, rows, cwd)` (Task 2).
- Produces:
  - `window.termlabProjectMode` with `adopt(invoke) -> Promise<{root,name}|null>`, `set(info)`, `root() -> string|null`, `name() -> string|null`, `isActive() -> boolean`, `isUnderRoot(path) -> boolean`, `reset()`
  - `window.__termlabProject = { root, name }` and `window.__termlabProjectName` (read by `tab-manager.js`)
  - `termlabOpenPathRouting.create(deps)` gains a required `deps.openProject(path) -> Promise` and no longer exports `DIRECTORY_COMING_SOON`
  - `createTab({ cwd })` starts its shell in `cwd`

- [ ] **Step 1: Write the failing test.** Create `scripts/tests/test_project_mode.mjs`:

```javascript
// Run: node scripts/tests/test_project_mode.mjs
//
// Project mode's boot half: the mode resolver every other project surface
// reads, the routing seam that used to answer a directory with a
// "coming soon" toast, and the source-level ordering that keeps a project
// window out of zen and gives it a terminal tab at the project root.
//
// No jsdom (see test_tl_dialog.mjs for the precedent). Deliberately does NOT
// define `sandbox.global` — see test_problems_panel.mjs's note.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const MODE = path.join(APP, 'features/project/project-mode.js');
const ROUTING = path.join(APP, 'features/editor/open-path-routing.js');
const INDEX_HTML = path.join(ROOT, 'index.html');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

const plain = (value) => JSON.parse(JSON.stringify(value === undefined ? null : value));
function deepEq(actual, expected, message) {
  assert.deepStrictEqual(plain(actual), plain(expected), message);
}

function load(files) {
  const sandbox = { console, Promise, JSON, String, Object, Array, Error };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return sandbox;
}

check('adopt binds the window and publishes the globals', async () => {
  const sandbox = load([MODE]);
  const mode = sandbox.termlabProjectMode;
  assert.strictEqual(mode.isActive(), false, 'a fresh window has no project');
  const invoked = [];
  const invoke = async (cmd) => {
    invoked.push(cmd);
    return { adopted: { root: '/repo', name: 'repo' }, focusedExisting: false };
  };
  const info = await mode.adopt(invoke);
  deepEq(info, { root: '/repo', name: 'repo' });
  deepEq(invoked, ['project_adopt_pending']);
  assert.strictEqual(mode.isActive(), true);
  assert.strictEqual(mode.root(), '/repo');
  assert.strictEqual(mode.name(), 'repo');
  deepEq(sandbox.__termlabProject, { root: '/repo', name: 'repo' });
  assert.strictEqual(sandbox.__termlabProjectName, 'repo');
});

check('adopt with nothing to adopt leaves the window project-less', async () => {
  const sandbox = load([MODE]);
  const mode = sandbox.termlabProjectMode;
  const info = await mode.adopt(async () => ({ adopted: null, focusedExisting: false }));
  assert.strictEqual(info, null);
  assert.strictEqual(mode.isActive(), false);
  assert.strictEqual(sandbox.__termlabProjectName, undefined);
});

check('a failing adopt is not fatal', async () => {
  const sandbox = load([MODE]);
  const info = await sandbox.termlabProjectMode.adopt(async () => { throw new Error('no command'); });
  assert.strictEqual(info, null, 'a missing backend must not break boot');
});

check('isUnderRoot answers on path boundaries, not string prefixes', () => {
  const sandbox = load([MODE]);
  const mode = sandbox.termlabProjectMode;
  mode.set({ root: '/repo', name: 'repo' });
  assert.strictEqual(mode.isUnderRoot('/repo/src/main.rs'), true);
  assert.strictEqual(mode.isUnderRoot('/repo'), true, 'the root itself is under the root');
  assert.strictEqual(mode.isUnderRoot('/repository/src/main.rs'), false, 'a sibling that shares a prefix is not inside');
  assert.strictEqual(mode.isUnderRoot('/elsewhere/lib.rs'), false);
  mode.reset();
  assert.strictEqual(mode.isUnderRoot('/repo/src/main.rs'), false, 'no project means nothing is under it');
});

check('a directory routes to project_open instead of a toast', async () => {
  const sandbox = load([ROUTING]);
  const calls = { opened: [], projects: [], infos: [], errors: [] };
  const routing = sandbox.termlabOpenPathRouting.create({
    invoke: async (cmd, args) => {
      if (cmd === 'local_stat') {
        return args.path === '/tmp/dir'
          ? { name: 'dir', is_dir: true, size: 0, modified: null, permissions: null }
          : { name: 'a.txt', is_dir: false, size: 1, modified: null, permissions: null };
      }
      throw new Error('unexpected command ' + cmd);
    },
    openLocalFile: (p) => { calls.opened.push(p); },
    openProject: async (p) => { calls.projects.push(p); },
    toastError: (title, body) => { calls.errors.push(body); },
    toastInfo: (title, body) => { calls.infos.push(body); },
  });
  const opened = await routing.routePaths(['/tmp/a.txt', '/tmp/dir']);
  assert.strictEqual(opened, 1, 'only the file counts as an opened editor');
  deepEq(calls.opened, ['/tmp/a.txt']);
  deepEq(calls.projects, ['/tmp/dir']);
  assert.strictEqual(calls.infos.length, 0, 'a directory is opened, not explained away');
  assert.strictEqual(
    sandbox.termlabOpenPathRouting.DIRECTORY_COMING_SOON,
    undefined,
    'the coming-soon seam is gone, not merely unused',
  );
});

check('a failing project_open reports the path rather than dropping it', async () => {
  const sandbox = load([ROUTING]);
  const errors = [];
  const routing = sandbox.termlabOpenPathRouting.create({
    invoke: async () => ({ name: 'dir', is_dir: true, size: 0, modified: null, permissions: null }),
    openLocalFile: () => {},
    openProject: async () => { throw new Error('not a folder'); },
    toastError: (title, body) => { errors.push(body); },
    toastInfo: () => {},
  });
  const opened = await routing.routePaths(['/tmp/dir']);
  assert.strictEqual(opened, 0);
  assert.strictEqual(errors.length, 1);
  assert.ok(errors[0].includes('/tmp/dir'), 'the error names the path');
});

check('startup-runtime keeps a project window out of zen', () => {
  const src = fs.readFileSync(path.join(APP, 'startup-runtime.js'), 'utf8');
  assert.ok(src.includes("invoke('pending_open_paths_kind')"), 'the boot layout asks what kind of paths are queued');
  assert.ok(!src.includes("has_pending_open_paths"), 'the boolean peek is gone');
  assert.ok(src.includes('termlabProjectMode'), 'the adoption happens before the layout read');
  const adopt = src.indexOf('termlabProjectMode');
  const layout = src.indexOf("invoke('get_saved_layout')");
  assert.ok(adopt < layout, 'adopt must precede the layout read so the per-project layout applies');
});

check('main-runtime gives a project window a terminal tab at the project root', () => {
  const src = fs.readFileSync(path.join(APP, 'main-runtime.js'), 'utf8');
  const pull = src.indexOf('take_pending_open_paths');
  const projectTab = src.indexOf('createTab({ cwd: projectRoot })');
  const firstTab = src.indexOf('createTab().catch');
  assert.ok(projectTab !== -1, 'a project window opens its first terminal at the root');
  assert.ok(firstTab !== -1, 'the plain terminal tab still exists for ordinary windows');
  assert.ok(pull < projectTab && pull < firstTab, 'the queue pull still precedes any tab creation');
});

check('the window title carries the project name across tab switches', () => {
  const src = fs.readFileSync(path.join(APP, 'tab-manager.js'), 'utf8');
  assert.ok(src.includes('__termlabProjectName'), 'updateWindowTitle reads the project name');
  const read = src.indexOf('__termlabProjectName');
  const set = src.indexOf('setWindowTitle(title)');
  assert.ok(read < set, 'the prefix is applied before the title is pushed to the OS');
});

check('index.html loads project-mode.js before the modules that read it', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(name);
  assert.ok(at('app/features/project/project-mode.js') > 0, 'project-mode.js is not loaded');
  assert.ok(at('app/features/project/project-mode.js') < at('app/features/editor/open-path-routing.js'));
  assert.ok(at('app/features/project/project-mode.js') < at('app/startup-runtime.js'));
});

check('the project modules use no regex lookbehind', () => {
  for (const file of [MODE, ROUTING]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(
      !/\(\?<[=!]/.test(source),
      `${file} uses a lookbehind — it costs the whole file on an older WKWebView`,
    );
  }
});

check('the project modules contain no control bytes', () => {
  for (const file of [MODE, ROUTING]) {
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      const byte = bytes[i];
      assert.ok(
        byte >= 0x20 || byte === 0x0a || byte === 0x09,
        `${file}: control byte 0x${byte.toString(16)} at offset ${i} — git treats the file as binary`,
      );
    }
  }
});

for (const { name, fn } of queued) {
  ran += 1;
  try {
    await fn();
    console.log(`  ok   ${name}`);
  } catch (error) {
    failures += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${(error && error.stack) || error}`);
  }
}
if (failures) {
  console.log(`project mode: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`project mode: all ${ran} checks passed`);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `node scripts/tests/test_project_mode.mjs`
Expected: FAIL — `Cannot find module .../features/project/project-mode.js` on the first checks, and the routing/source checks fail on the still-present `DIRECTORY_COMING_SOON`.

- [ ] **Step 3: Write the mode resolver.** Create `crates/termlab_tauri/frontend/app/features/project/project-mode.js`:

```javascript
// This window's project, resolved once and read by every project surface.
//
// The adoption happens in startup-runtime.js during applyAppConfig — BEFORE
// the layout read and before the tool-window runtime registers anything — so
// the per-project layout is in effect on first paint and the Search tool
// window knows at registration time whether it belongs in this window.
//
// `isUnderRoot` compares on path boundaries rather than string prefixes: a
// sibling directory whose name merely starts with the project's ("/repository"
// beside "/repo") is a different project, and treating it as inside would
// silently attach the wrong LSP root to its files.
(function initTermLabProjectMode(global) {
  'use strict';

  let project = null;

  function set(info) {
    if (!info || !info.root) {
      project = null;
      return null;
    }
    project = { root: String(info.root), name: String(info.name || '') };
    global.__termlabProject = { root: project.root, name: project.name };
    global.__termlabProjectName = project.name;
    return project;
  }

  function reset() {
    project = null;
    delete global.__termlabProject;
    delete global.__termlabProjectName;
  }

  // Never fatal: a backend that cannot answer leaves the window an ordinary
  // terminal window rather than failing its boot.
  async function adopt(invoke) {
    try {
      const result = await invoke('project_adopt_pending');
      if (!result || !result.adopted) return null;
      return set(result.adopted);
    } catch (error) {
      console.warn('project-mode: could not adopt a pending project', error);
      return null;
    }
  }

  function root() {
    return project ? project.root : null;
  }

  function name() {
    return project ? project.name : null;
  }

  function isActive() {
    return project !== null;
  }

  function isUnderRoot(filePath) {
    if (!project || !filePath) return false;
    const target = String(filePath);
    if (target === project.root) return true;
    const prefix = project.root.endsWith('/') ? project.root : project.root + '/';
    return target.startsWith(prefix);
  }

  global.termlabProjectMode = {
    adopt, set, reset, root, name, isActive, isUnderRoot,
  };
})(window);
```

- [ ] **Step 4: Replace the routing seam.** In `open-path-routing.js`, delete the `DIRECTORY_COMING_SOON` constant, replace the directory branch, and drop the constant from the export. Header comment's third bullet becomes:

```javascript
//   - a directory is opened as a project (project_open focuses a window that
//     already holds the same canonical root, else creates one)
```

Body:

```javascript
  function create(deps) {
    const invoke = deps.invoke;
    const openLocalFile = deps.openLocalFile;
    const openProject = deps.openProject;
    const toastError = deps.toastError;
    const toastInfo = deps.toastInfo;
```

```javascript
      if (entry && entry.is_dir) {
        // A directory is a PROJECT, and a project owns a window: this returns
        // false because no editor opened here, which is what the boot path
        // counts. Reported by name on failure rather than dropped — a folder
        // that vanished between the stat and the open is exactly the case a
        // silent return would hide.
        try {
          await openProject(pathStr);
        } catch (error) {
          toastError('Cannot Open Folder', pathStr + ': ' + String(error));
        }
        return false;
      }
```

…and the export becomes `global.termlabOpenPathRouting = { create };`. The `toastInfo` dep stays wired (it is part of the module's published contract and costs nothing); if lint flags it as unused, keep it assigned and referenced by the `Cannot Open Folder` fallback below — simplest is to leave `toastInfo` in the destructure and add no other use.

- [ ] **Step 5: Wire `openProject` in.** In `event-wiring-runtime.js`'s `wirePendingOpenDrain`, add to the `routing.create({…})` object:

```javascript
      openProject: (dirPath) => deps.invoke('project_open', { path: dirPath }),
```

- [ ] **Step 6: Adopt, and gate zen, in `startup-runtime.js`.** In `applyAppConfig`, immediately after `window.__termlabAppConfig = appCfg;` and before the `try { const layoutData = await invoke('get_saved_layout'); …` block:

```javascript
        // Adopt a queued directory as THIS window's project before the layout
        // is read: `get_saved_layout` returns the per-project layout once the
        // window is bound, and the tool-window runtime (which registers the
        // Search window only for a project) runs later still.
        if (global.termlabProjectMode && typeof global.termlabProjectMode.adopt === 'function') {
          await global.termlabProjectMode.adopt(invoke);
        }
```

Then replace the `has_pending_open_paths` block (lines 191-204) with:

```javascript
          // A window with queued CLI FILE paths (`termlab notes.md`) boots in
          // zen regardless of the saved layout: it is about to become an
          // editor-only window. A window opening a PROJECT (`termlab .`) does
          // the opposite — it keeps its panels, because the tree and the
          // search panel are the point. `pending_open_paths_kind` is a
          // non-destructive peek; the destructive take happens later in
          // main-runtime, after the editor service exists. Session-only, same
          // as the new-window default: this window must never teach the
          // shared layout to open in zen.
          try {
            const pendingKind = await invoke('pending_open_paths_kind');
            if (pendingKind === 'files') {
              zenOn = true;
              window.__termlabZenIsSessionDefault = true;
            }
          } catch (_) {}
          if (global.termlabProjectMode && global.termlabProjectMode.isActive()) {
            zenOn = false;
            window.__termlabZenIsSessionDefault = true;
          }
```

- [ ] **Step 7: Give the project window its terminal at the root.** In `main-runtime.js`, replace the `if (cliOpenedEditors > 0) { … } else { … }` block with:

```javascript
      const projectRoot = window.termlabProjectMode && window.termlabProjectMode.isActive()
        ? window.termlabProjectMode.root()
        : null;
      if (cliOpenedEditors > 0) {
        // Closing this window's last tab opens a terminal tab instead of
        // closing the window; tab-manager.js consumes the flag once.
        window.__termlabEditorWindow = true;
      } else if (projectRoot) {
        // A project window's initial main content is one terminal at the
        // project root; editor tabs join it as files are opened from the tree.
        await createTab({ cwd: projectRoot }).catch((e) => {
          showStatus('Failed to initialize project terminal: ' + String(e));
        });
      } else {
        const firstTabPromise = createTab().catch((e) => {
          showStatus('Failed to initialize first tab: ' + String(e));
        });
        await firstTabPromise;
      }
```

- [ ] **Step 8: Thread `cwd` and the title through `tab-manager.js`.** In `createTab`, change the spawn call:

```javascript
        if (options && options.plainShell) {
          await spawnDefaultShell(paneId, cols, rows);
        } else {
          await spawnShell(paneId, cols, rows, options ? options.cwd : null);
        }
```

In `updateWindowTitle`, after the `title` is computed and before the `setWindowTitle` call:

```javascript
      // A project window keeps its name in the OS title across tab switches:
      // the window is "conch", whatever tab happens to be active in it.
      const projectName = window.__termlabProjectName;
      if (projectName) {
        title = title === 'TermLab' ? projectName : projectName + ' — ' + title;
      }
```

In `manager-compose-runtime.js`, both `spawnShell` deps become:

```javascript
          spawnShell: (paneId, cols, rows, cwd) => invoke('spawn_shell', { paneId, cols, rows, cwd: cwd || null }),
```

- [ ] **Step 9: Load the module.** In `index.html`, add before `app/features/editor/open-path-routing.js` (and therefore before `startup-runtime.js`):

```html
  <script src="app/features/project/project-mode.js"></script>
```

- [ ] **Step 10: Update the two open-path suites.** In `scripts/tests/test_open_path_routing.mjs`, add `openProject: async (p) => { calls.projects.push(p); }` to `load`'s deps (and `projects: []` to `calls`), replace every `calls.infos.length === 1, 'directory gets the coming-soon toast'` assertion with `deepStrictEqual(calls.projects, ['/tmp/dir'])`, and delete the whole `DIRECTORY_COMING_SOON` constant block. In `scripts/tests/test_open_path_wiring.mjs`, replace the directory-toast case with:

```javascript
// A directory reaches project_open through the wiring (the seam that used to
// answer with a "coming soon" toast).
{
  const sandbox = loadRuntime();
  sandbox.termlabEditorService = { openLocalFile: () => { throw new Error('must not open a directory'); } };
  sandbox.toast = { error: () => {}, info: () => {} };
  const invoked = [];
  const invoke = async (cmd) => {
    invoked.push(cmd);
    if (cmd === 'take_pending_open_paths') return ['/tmp/dir'];
    if (cmd === 'project_open') return { root: '/tmp/dir', name: 'dir', windowLabel: 'window-2', focusedExisting: false };
    return { name: 'dir', is_dir: true, size: 0, modified: null, permissions: null };
  };
  const drain = sandbox.termlabEventWiringRuntime.wirePendingOpenDrain(sandbox, { invoke });
  await drain.drainPendingOpens();
  assert.ok(invoked.includes('project_open'), 'a directory must reach project_open');
}
```

…and in that file's final source-assertion block, replace the `has_pending_open_paths` expectations with none (that block does not reference it) and leave the rest untouched.

- [ ] **Step 11: Run the tests to verify they pass**

Run: `node scripts/tests/test_project_mode.mjs` — all checks pass.
Run: `node scripts/tests/test_open_path_routing.mjs && node scripts/tests/test_open_path_wiring.mjs` — pass.
Run the full sweep: `for f in scripts/tests/*.mjs; do node "$f" >/dev/null 2>&1 || echo "FAIL: $f"; done` — prints nothing.
Run: `bash scripts/check_frontend_boundaries.sh` — only the pre-existing `tl-dialog.js:334` finding.

- [ ] **Step 12: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/project/project-mode.js \
        crates/termlab_tauri/frontend/app/features/editor/open-path-routing.js \
        crates/termlab_tauri/frontend/app/event-wiring-runtime.js \
        crates/termlab_tauri/frontend/app/startup-runtime.js \
        crates/termlab_tauri/frontend/app/main-runtime.js \
        crates/termlab_tauri/frontend/app/tab-manager.js \
        crates/termlab_tauri/frontend/app/manager-compose-runtime.js \
        crates/termlab_tauri/frontend/index.html \
        scripts/tests/test_project_mode.mjs \
        scripts/tests/test_open_path_routing.mjs \
        scripts/tests/test_open_path_wiring.mjs
git commit -m "feat: open a directory from the CLI as a project window"
```

---

### Task 4: Open Folder — File menu item and command-palette action

**Files:**
- Modify: `crates/termlab_tauri/src/menu.rs` (constants ~line 44/81, `build_app_menu:150`, `build_app_menu_with_plugins:550`)
- Modify: `crates/termlab_tauri/src/lib.rs` (`on_menu_event:600`)
- Modify: `crates/termlab_tauri/frontend/app/menu-actions.js` (after the `open-file` branch, line 123)
- Modify: `crates/termlab_tauri/frontend/app/command-palette-runtime.js` (after `core:open-file`, line 118)
- Test: `scripts/tests/test_project_mode.mjs` (extend)

**Interfaces:**
- Consumes: `project_pick_folder() -> string | null`, `project_open(path) -> ProjectOpenResult` (Task 1); `handleMenuAction(action)` (`menu-actions.js`).
- Produces:
  - Rust: `MENU_OPEN_FOLDER_ID = "file.open_folder"`, `MENU_ACTION_OPEN_FOLDER = "open-folder"`
  - Frontend: `handleMenuAction('open-folder')` picks a folder and opens it as a project; palette command id `core:open-folder`.

- [ ] **Step 1: Write the failing test.** Append to `scripts/tests/test_project_mode.mjs`, before the loop that runs `queued`:

```javascript
check('Open Folder is reachable from the menu, the palette and Rust', () => {
  const actions = fs.readFileSync(path.join(APP, 'menu-actions.js'), 'utf8');
  assert.ok(actions.includes("action === 'open-folder'"), 'menu-actions handles open-folder');
  assert.ok(actions.includes("invoke('project_pick_folder')"), 'it uses the native directory picker');
  assert.ok(actions.includes("invoke('project_open'"), 'the picked folder is opened as a project');

  const palette = fs.readFileSync(path.join(APP, 'command-palette-runtime.js'), 'utf8');
  assert.ok(palette.includes("'core:open-folder'"), 'the palette exposes Open Folder as a Project');
  assert.ok(palette.includes("handleMenuAction('open-folder')"), 'the palette routes through the one handler');

  const menuRs = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/menu.rs'), 'utf8');
  assert.ok(menuRs.includes('MENU_OPEN_FOLDER_ID'), 'the File menu has an Open Folder id');
  assert.ok(menuRs.includes('"open-folder"'), 'the menu action string exists');
  // Both builders: the plugin-aware rebuild must not silently drop the item.
  const occurrences = menuRs.split('MENU_OPEN_FOLDER_ID').length - 1;
  assert.ok(occurrences >= 4, `Open Folder must appear in both menu builders, saw ${occurrences} references`);
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `node scripts/tests/test_project_mode.mjs`
Expected: FAIL — `menu-actions handles open-folder`.

- [ ] **Step 3: Add the Rust menu item.** In `menu.rs`, beside the other File ids and actions:

```rust
pub(crate) const MENU_OPEN_FOLDER_ID: &str = "file.open_folder";
```

```rust
pub(crate) const MENU_ACTION_OPEN_FOLDER: &str = "open-folder";
```

In `build_app_menu`, after the `open_file` item and before `save_file_as`:

```rust
    // No accelerator: opening a project is a deliberate, infrequent act, and a
    // native accelerator here would be consumed by AppKit before the webview
    // saw the key (see the note on save_file_as).
    let open_folder = MenuItem::with_id(
        app,
        MENU_OPEN_FOLDER_ID,
        "Open Folder\u{2026}",
        true,
        None::<&str>,
    )?;
```

…and add `&open_folder,` to the `file_menu` item slice immediately after `&open_file,`. Make the identical addition in `build_app_menu_with_plugins` (its `"Open File\u{2026}"` item is at line ~720; its `File` submenu at ~755).

In `lib.rs`'s `on_menu_event`, after the `MENU_OPEN_FILE_ID` arm:

```rust
            menu::MENU_OPEN_FOLDER_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_OPEN_FOLDER)
            }
```

- [ ] **Step 4: Handle the action.** In `menu-actions.js`, after the `open-file` branch:

```javascript
      if (action === 'open-folder') {
        // Always a NEW window, even from inside a project window: a project
        // owns its window, so re-targeting the current one would evict a
        // project the user is still working in. project_open focuses an
        // existing window when it already holds the same canonical root.
        Promise.resolve(invoke('project_pick_folder'))
          .then((picked) => {
            if (!picked) return null;
            return invoke('project_open', { path: picked });
          })
          .catch((error) => {
            if (global.toast) global.toast.error('Cannot Open Folder', String(error));
          });
        return;
      }
```

- [ ] **Step 5: Add the palette action.** In `command-palette-runtime.js`, after the `core:open-file` line:

```javascript
      add('core:open-folder', 'Open Folder as Project…', 'Project', 'open folder project directory workspace tree search git', () => handleMenuAction('open-folder'), 'Project');
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `node scripts/tests/test_project_mode.mjs` — pass.
Run: `cargo test -p termlab_tauri --quiet` — PASS.
Run the full sweep — prints nothing.

- [ ] **Step 7: Commit**

```bash
git add crates/termlab_tauri/src/menu.rs crates/termlab_tauri/src/lib.rs \
        crates/termlab_tauri/frontend/app/menu-actions.js \
        crates/termlab_tauri/frontend/app/command-palette-runtime.js \
        scripts/tests/test_project_mode.mjs
git commit -m "feat: add File > Open Folder and the Open Folder palette action"
```

---

### Task 5: The project tree module

**Files:**
- Create: `crates/termlab_tauri/frontend/app/panels/project-tree.js`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/project-tree.css`
- Modify: `crates/termlab_tauri/frontend/index.html` (script + stylesheet)
- Test: `scripts/tests/test_project_tree.mjs` (new)

**Interfaces:**
- Consumes: `local_list_dir(path) -> FileEntry[]` where `FileEntry = { name, is_dir, size, modified, permissions }`; `window.termlabEditorService.openLocalFile(path)`; `window.fileIcons.iconFor(name, isDir, isRemote) -> html`; `window.termlabProjectMode.root()`.
- Produces: `window.termlabProjectTree.create(options) -> handle` where

```
options = {
  invoke, root, showHidden: boolean,
  onOpenFile(path),                     // defaults to editor-service
  onContextMenu(event, node),           // node = { path, name, isDir, parentPath }
  onReopen(),                           // the missing-root state's action
  toastError(title, body),
}
handle = {
  element,                              // the tree root element
  noticeHost,                           // the slot the trust banner (Task 7) mounts into
  expand(path) -> Promise, collapse(path),
  refresh(path) -> Promise,             // re-list one open directory
  refreshAll() -> Promise,              // re-list every open directory
  settled() -> Promise,                 // resolves when queued listings are done
  activePath() -> string|null,
  rows() -> node[],                     // the flattened visible nodes, in display order
  setGitStatus(snapshot),               // Task 11
  setMissing(missing: boolean),
  setShowHidden(showHidden: boolean),
  focus(), destroy(),
}
```
- Also produces the pure helpers `window.termlabProjectTree.sortEntries(entries, showHidden)` and `window.termlabProjectTree.joinPath(base, name)`.

- [ ] **Step 1: Write the failing test.** Create `scripts/tests/test_project_tree.mjs`. Reuse `test_problems_panel.mjs`'s DOM stand-in verbatim (copy `makeElement`, `selectorParts`, `dataName` and the `load()` sandbox builder from lines 47-230 and 285-330 of that file) and then add:

```javascript
const TREE = path.join(APP, 'panels/project-tree.js');
const CSS = path.join(ROOT, 'styles/design-system/components/project-tree.css');

function treeHarness(options) {
  const opts = options || {};
  const { sandbox, body, documentStub } = load([TREE]);
  const listed = [];
  const dirs = opts.dirs || {};
  const invoke = async (cmd, args) => {
    if (cmd !== 'local_list_dir') throw new Error('unexpected command ' + cmd);
    listed.push(args.path);
    if (!(args.path in dirs)) throw new Error('permission denied');
    return dirs[args.path];
  };
  const opened = [];
  const errors = [];
  const handle = sandbox.termlabProjectTree.create({
    invoke,
    root: opts.root || '/repo',
    showHidden: opts.showHidden === true,
    onOpenFile: (p) => { opened.push(p); },
    onContextMenu: () => {},
    toastError: (title, msg) => { errors.push(msg); },
  });
  body.appendChild(handle.element);
  return { sandbox, handle, listed, opened, errors, body, documentStub };
}

const entry = (name, isDir) => ({ name, is_dir: isDir, size: 0, modified: null, permissions: null });

check('sortEntries: dirs first, alphabetical, case-insensitive', () => {
  const { sandbox } = treeHarness({ dirs: { '/repo': [] } });
  const names = sandbox.termlabProjectTree.sortEntries([
    entry('zeta.rs', false), entry('Beta', true), entry('alpha.rs', false), entry('apple', true),
  ], true).map((e) => e.name);
  deepEq(names, ['apple', 'Beta', 'alpha.rs', 'zeta.rs']);
});

check('sortEntries honours the hidden-files convention', () => {
  const { sandbox } = treeHarness({ dirs: { '/repo': [] } });
  const visible = sandbox.termlabProjectTree.sortEntries(
    [entry('.git', true), entry('src', true), entry('.env', false), entry('a.rs', false)], false,
  ).map((e) => e.name);
  deepEq(visible, ['src', 'a.rs'], 'dotfiles are hidden by default');
  const all = sandbox.termlabProjectTree.sortEntries(
    [entry('.git', true), entry('src', true)], true,
  ).map((e) => e.name);
  deepEq(all, ['.git', 'src']);
});

check('the tree lists the root once and nothing below it until expanded', async () => {
  const h = treeHarness({
    dirs: {
      '/repo': [entry('src', true), entry('README.md', false)],
      '/repo/src': [entry('main.rs', false)],
    },
  });
  await h.handle.refreshAll();
  deepEq(h.listed, ['/repo'], 'lazy: only the root is listed');
  const paths = h.handle.rows().map((n) => n.path);
  deepEq(paths, ['/repo/src', '/repo/README.md']);
  await h.handle.expand('/repo/src');
  deepEq(h.listed, ['/repo', '/repo/src'], 'a directory lists when first expanded');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/src', '/repo/src/main.rs', '/repo/README.md']);
});

check('collapsing keeps the listing and re-expanding does not re-list', async () => {
  const h = treeHarness({
    dirs: { '/repo': [entry('src', true)], '/repo/src': [entry('main.rs', false)] },
  });
  await h.handle.refreshAll();
  await h.handle.expand('/repo/src');
  h.handle.collapse('/repo/src');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/src']);
  await h.handle.expand('/repo/src');
  deepEq(h.listed, ['/repo', '/repo/src'], 'the cached listing is reused');
});

check('an unreadable directory toasts and collapses, and the rest keeps working', async () => {
  const h = treeHarness({
    dirs: { '/repo': [entry('secret', true), entry('src', true)], '/repo/src': [entry('a.rs', false)] },
  });
  await h.handle.refreshAll();
  await h.handle.expand('/repo/secret');
  assert.strictEqual(h.errors.length, 1, 'the failure is reported');
  assert.ok(h.errors[0].includes('/repo/secret'), 'the toast names the directory');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/secret', '/repo/src'], 'the row stays, collapsed');
  await h.handle.expand('/repo/src');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/secret', '/repo/src', '/repo/src/a.rs']);
});

check('Enter and click open a file through the injected opener', async () => {
  const h = treeHarness({ dirs: { '/repo': [entry('a.rs', false)] } });
  await h.handle.refreshAll();
  const row = h.handle.element.querySelector('[data-tree-path="/repo/a.rs"]');
  assert.ok(row, 'the file row is rendered');
  row.dispatchEvent({ type: 'click', target: row });
  deepEq(h.opened, ['/repo/a.rs']);
});

check('arrows move, Right expands, Left collapses', async () => {
  const h = treeHarness({
    dirs: { '/repo': [entry('src', true), entry('a.rs', false)], '/repo/src': [entry('m.rs', false)] },
  });
  await h.handle.refreshAll();
  const list = h.handle.element.querySelector('.tl-project-tree__list');
  const key = (k) => list.dispatchEvent({ type: 'keydown', key: k, preventDefault() {}, target: list });
  key('ArrowDown');
  assert.strictEqual(h.handle.activePath(), '/repo/src');
  key('ArrowRight');
  await h.handle.settled();
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/src', '/repo/src/m.rs', '/repo/a.rs']);
  key('ArrowLeft');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/src', '/repo/a.rs']);
  key('ArrowDown');
  assert.strictEqual(h.handle.activePath(), '/repo/a.rs');
  key('Enter');
  deepEq(h.opened, ['/repo/a.rs']);
});

check('a right-click hands the node to the context-menu hook', async () => {
  const seen = [];
  const { sandbox, body } = load([TREE]);
  const handle = sandbox.termlabProjectTree.create({
    invoke: async () => [entry('a.rs', false)],
    root: '/repo',
    showHidden: false,
    onOpenFile: () => {},
    onContextMenu: (event, node) => { seen.push(node); },
    toastError: () => {},
  });
  body.appendChild(handle.element);
  await handle.refreshAll();
  const row = handle.element.querySelector('[data-tree-path="/repo/a.rs"]');
  row.dispatchEvent({ type: 'contextmenu', target: row, preventDefault() {}, clientX: 1, clientY: 2 });
  deepEq(seen, [{ path: '/repo/a.rs', name: 'a.rs', isDir: false, parentPath: '/repo' }]);
});

check('setMissing renders the vanished-root state with a reopen action', async () => {
  const h = treeHarness({ dirs: { '/repo': [entry('a.rs', false)] } });
  await h.handle.refreshAll();
  h.handle.setMissing(true);
  const missing = h.handle.element.querySelector('.tl-project-tree__missing');
  assert.ok(missing, 'the missing state renders');
  assert.ok(missing.textContent.includes('missing'), 'it says what is wrong');
  assert.ok(h.handle.element.querySelector('[data-tree-action="reopen"]'), 'and offers a way out');
  h.handle.setMissing(false);
  assert.strictEqual(h.handle.element.querySelector('.tl-project-tree__missing'), null);
});

check('project-tree.css styles every class the tree renders, with tokens only', () => {
  const css = fs.readFileSync(CSS, 'utf8');
  for (const name of [
    'tl-project-tree', 'tl-project-tree__toolbar', 'tl-project-tree__list',
    'tl-project-tree__row', 'tl-project-tree__twisty', 'tl-project-tree__label',
    'tl-project-tree__missing',
  ]) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(css), 'design-system components use tokens only');
  assert.ok(/focus-visible/.test(css), 'tree rows need a strong focus state');
});

check('the tree module uses no regex lookbehind and no control bytes', () => {
  for (const file of [TREE, CSS]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(!/\(\?<[=!]/.test(source), `${file} uses a lookbehind`);
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
        `${file}: control byte at offset ${i}`);
    }
  }
});

check('index.html loads the tree module and its stylesheet', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  assert.ok(html.indexOf('app/panels/project-tree.js') > 0);
  assert.ok(html.indexOf('styles/design-system/components/project-tree.css') > 0);
  assert.ok(html.indexOf('app/panels/project-tree.js') < html.indexOf('app/panels/files-panel.js'),
    'files-panel consumes the tree, so the tree must load first');
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `node scripts/tests/test_project_tree.mjs`
Expected: FAIL — `Cannot find module .../panels/project-tree.js`.

- [ ] **Step 3: Write the tree module.** Create `crates/termlab_tauri/frontend/app/panels/project-tree.js`:

```javascript
// The project tree — a single-pane, lazily-listed view of one directory.
//
// Its own module rather than more code in the already-large files panel: the
// panel decides WHICH view a window gets (project tree or the dual-pane
// local+SFTP explorer), and this decides what a tree is.
//
// Listing goes through the existing `local_list_dir` command and its
// `FileEntry` shape — there is no new listing backend. Lazy: a directory is
// listed the first time it is expanded and the listing is then cached, so
// collapsing and re-expanding costs nothing. There is no filesystem watcher
// in v1; freshness comes from explicit refresh triggers the panel owns.
//
// Every name that came off the filesystem is written with textContent. The
// only innerHTML in this file is the icon markup from window.fileIcons, which
// is repo-authored SVG.
(function initTermLabProjectTree(global) {
  'use strict';

  function el(tag, className) {
    const node = global.document.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  function joinPath(base, name) {
    if (!base || base === '/') return '/' + name;
    return base.endsWith('/') ? base + name : base + '/' + name;
  }

  // Directories first, then alphabetical, case-insensitive — the same rule
  // local_fs.rs::sort_entries applies server-side, restated here because the
  // hidden-files filter runs in the same pass.
  function sortEntries(entries, showHidden) {
    const visible = (entries || []).filter(
      (item) => showHidden || !String((item && item.name) || '').startsWith('.'),
    );
    return visible.slice().sort((a, b) => {
      if (!!a.is_dir !== !!b.is_dir) return a.is_dir ? -1 : 1;
      const an = String(a.name || '').toLowerCase();
      const bn = String(b.name || '').toLowerCase();
      if (an < bn) return -1;
      if (an > bn) return 1;
      return 0;
    });
  }

  function iconHtml(name, isDir) {
    return global.fileIcons && typeof global.fileIcons.iconFor === 'function'
      ? global.fileIcons.iconFor(name, isDir, false)
      : '';
  }

  function create(options) {
    const opts = options || {};
    const invoke = opts.invoke;
    const root = String(opts.root || '');
    const onOpenFile = typeof opts.onOpenFile === 'function'
      ? opts.onOpenFile
      : (filePath) => {
        const service = global.termlabEditorService;
        if (service && typeof service.openLocalFile === 'function') service.openLocalFile(filePath);
      };
    const onContextMenu = typeof opts.onContextMenu === 'function' ? opts.onContextMenu : () => {};
    const toastError = typeof opts.toastError === 'function'
      ? opts.toastError
      : (title, body) => { if (global.toast) global.toast.error(title, body); };

    let showHidden = opts.showHidden === true;
    let missing = false;
    let gitStatus = null;
    let activePath = null;
    let pending = Promise.resolve();

    // path -> FileEntry[] for every directory ever listed, and the set of
    // directories currently open. Two maps rather than one node graph: the
    // flat pair is what makes refreshAll a loop over `listings.keys()`.
    const listings = new Map();
    const expanded = new Set();
    let rowNodes = [];

    const element = el('div', 'tl-project-tree');
    const toolbar = el('div', 'tl-project-tree__toolbar');
    const title = el('span', 'tl-project-tree__title');
    title.textContent = root;
    title.title = root;
    const refreshButton = el('button', 'tl-project-tree__button');
    refreshButton.type = 'button';
    refreshButton.textContent = 'Refresh';
    refreshButton.setAttribute('aria-label', 'Refresh the project tree');
    refreshButton.addEventListener('click', () => { refreshAll(); });
    const hiddenButton = el('button', 'tl-project-tree__button');
    hiddenButton.type = 'button';
    hiddenButton.textContent = 'Hidden';
    hiddenButton.setAttribute('aria-pressed', showHidden ? 'true' : 'false');
    hiddenButton.addEventListener('click', () => {
      showHidden = !showHidden;
      hiddenButton.setAttribute('aria-pressed', showHidden ? 'true' : 'false');
      render();
    });
    toolbar.appendChild(title);
    toolbar.appendChild(hiddenButton);
    toolbar.appendChild(refreshButton);

    // A fixed slot above the list so the banner (Task 7) and the missing-root
    // state can appear and disappear without the tree reordering anything.
    const noticeHost = el('div', 'tl-project-tree__notice-host');

    const list = el('div', 'tl-project-tree__list tl-scroll');
    list.setAttribute('role', 'tree');
    list.setAttribute('aria-label', 'Project files');
    list.setAttribute('tabindex', '0');

    element.appendChild(toolbar);
    element.appendChild(noticeHost);
    element.appendChild(list);

    async function listDir(dirPath) {
      try {
        const entries = await invoke('local_list_dir', { path: dirPath });
        listings.set(dirPath, Array.isArray(entries) ? entries : []);
        return true;
      } catch (error) {
        // Toast and collapse: the rest of the tree keeps working, which is the
        // whole point of listing lazily and per-directory.
        toastError('Cannot Read Folder', dirPath + ': ' + String(error));
        listings.set(dirPath, []);
        expanded.delete(dirPath);
        return false;
      }
    }

    function track(promise) {
      pending = pending.then(() => promise).catch(() => {});
      return promise;
    }

    async function expand(dirPath) {
      if (!listings.has(dirPath)) {
        const ok = await listDir(dirPath);
        if (!ok) { render(); return; }
      }
      expanded.add(dirPath);
      render();
    }

    function collapse(dirPath) {
      expanded.delete(dirPath);
      render();
    }

    async function refresh(dirPath) {
      if (!listings.has(dirPath)) return;
      await listDir(dirPath);
      render();
    }

    async function refreshAll() {
      const targets = listings.size ? Array.from(listings.keys()) : [root];
      for (const dirPath of targets) {
        await listDir(dirPath);
      }
      render();
    }

    // The flattened, currently-visible rows, depth-first in display order.
    function buildNodes() {
      const out = [];
      const walk = (dirPath, depth) => {
        for (const item of sortEntries(listings.get(dirPath) || [], showHidden)) {
          const nodePath = joinPath(dirPath, item.name);
          out.push({
            path: nodePath,
            name: item.name,
            isDir: !!item.is_dir,
            parentPath: dirPath,
            depth,
          });
          if (item.is_dir && expanded.has(nodePath)) walk(nodePath, depth + 1);
        }
      };
      walk(root, 0);
      return out;
    }

    function gitStateFor(node) {
      const git = global.termlabProjectGit;
      if (!gitStatus || !git || typeof git.stateForPath !== 'function') return null;
      return git.stateForPath(gitStatus, root, node.path, node.isDir);
    }

    function renderRow(node) {
      const row = el('div', 'tl-project-tree__row');
      row.setAttribute('role', 'treeitem');
      row.setAttribute('data-tree-path', node.path);
      row.setAttribute('aria-level', String(node.depth + 1));
      row.setAttribute('tabindex', '-1');
      row.style.paddingLeft = (node.depth * 12 + 4) + 'px';

      const twisty = el('span', 'tl-project-tree__twisty');
      twisty.setAttribute('aria-hidden', 'true');
      if (node.isDir) {
        const open = expanded.has(node.path);
        twisty.textContent = open ? '▾' : '▸';
        row.setAttribute('aria-expanded', open ? 'true' : 'false');
      } else {
        twisty.textContent = '';
      }

      const icon = el('span', 'tl-project-tree__icon');
      icon.setAttribute('aria-hidden', 'true');
      icon.innerHTML = iconHtml(node.name, node.isDir);

      const label = el('span', 'tl-project-tree__label');
      label.textContent = node.name;

      const state = gitStateFor(node);
      if (state) {
        row.setAttribute('data-git-state', state);
        row.setAttribute('aria-label', node.name + ', ' + state);
      }

      row.appendChild(twisty);
      row.appendChild(icon);
      row.appendChild(label);
      row.title = node.path;
      row._node = node;
      return row;
    }

    function renderMissing() {
      const notice = el('div', 'tl-project-tree__missing');
      notice.setAttribute('role', 'note');
      const text = el('span', 'tl-project-tree__missing-text');
      text.textContent = 'This project folder is missing: ' + root;
      const reopen = el('button', 'tl-project-tree__button');
      reopen.type = 'button';
      reopen.textContent = 'Choose another folder…';
      reopen.setAttribute('data-tree-action', 'reopen');
      reopen.addEventListener('click', () => {
        if (typeof opts.onReopen === 'function') opts.onReopen();
      });
      notice.appendChild(text);
      notice.appendChild(reopen);
      return notice;
    }

    function render() {
      const notices = [];
      if (missing) notices.push(renderMissing());
      noticeHost.replaceChildren(...notices);

      const nodes = missing ? [] : buildNodes();
      const built = nodes.map(renderRow);
      list.replaceChildren(...built);
      rowNodes = nodes;
      if (activePath && !nodes.some((n) => n.path === activePath)) activePath = null;
      applyActive(built);
    }

    function applyActive(built) {
      const rows = built || Array.from(list.children);
      rows.forEach((row) => {
        const isActive = !!row._node && row._node.path === activePath;
        row.setAttribute('tabindex', isActive ? '0' : '-1');
        row.classList.toggle('is-active', isActive);
      });
    }

    function moveTo(index) {
      if (!rowNodes.length) return;
      const clamped = Math.min(Math.max(index, 0), rowNodes.length - 1);
      activePath = rowNodes[clamped].path;
      applyActive();
      const row = list.querySelector('[data-tree-path="' + activePath + '"]');
      if (row && typeof row.focus === 'function') row.focus();
    }

    function indexOfActive() {
      return rowNodes.findIndex((n) => n.path === activePath);
    }

    function activate(node) {
      if (!node) return;
      activePath = node.path;
      if (node.isDir) {
        if (expanded.has(node.path)) collapse(node.path);
        else track(expand(node.path));
        return;
      }
      onOpenFile(node.path);
    }

    // Capture-phase discipline lives with the panel's router registration; the
    // tree owns only what happens once a key reaches its list.
    list.addEventListener('keydown', (event) => {
      const at = indexOfActive();
      const key = event.key;
      if (key === 'ArrowDown') { moveTo(at < 0 ? 0 : at + 1); event.preventDefault(); return; }
      if (key === 'ArrowUp') { moveTo(at < 0 ? 0 : at - 1); event.preventDefault(); return; }
      if (key === 'Home') { moveTo(0); event.preventDefault(); return; }
      if (key === 'End') { moveTo(rowNodes.length - 1); event.preventDefault(); return; }
      if (key === 'ArrowRight') {
        const node = rowNodes[at < 0 ? 0 : at];
        if (node && node.isDir && !expanded.has(node.path)) { activePath = node.path; track(expand(node.path)); }
        else moveTo(at < 0 ? 0 : at + 1);
        event.preventDefault();
        return;
      }
      if (key === 'ArrowLeft') {
        const node = rowNodes[at < 0 ? 0 : at];
        if (node && node.isDir && expanded.has(node.path)) { activePath = node.path; collapse(node.path); }
        else moveTo(at < 0 ? 0 : at - 1);
        event.preventDefault();
        return;
      }
      if (key === 'Enter') {
        activate(rowNodes[at < 0 ? 0 : at]);
        event.preventDefault();
      }
    });

    list.addEventListener('click', (event) => {
      const row = event.target && typeof event.target.closest === 'function'
        ? event.target.closest('[data-tree-path]')
        : event.target;
      if (!row || !row._node) return;
      activate(row._node);
    });

    list.addEventListener('contextmenu', (event) => {
      const row = event.target && typeof event.target.closest === 'function'
        ? event.target.closest('[data-tree-path]')
        : event.target;
      if (!row || !row._node) return;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      activePath = row._node.path;
      const node = row._node;
      onContextMenu(event, {
        path: node.path, name: node.name, isDir: node.isDir, parentPath: node.parentPath,
      });
    });

    return {
      element,
      expand: (p) => track(expand(p)),
      collapse,
      refresh: (p) => track(refresh(p)),
      refreshAll: () => track(refreshAll()),
      settled: () => pending,
      activePath: () => activePath,
      rows: () => rowNodes.slice(),
      setGitStatus(snapshot) { gitStatus = snapshot || null; render(); },
      setMissing(value) { missing = value === true; render(); },
      setShowHidden(value) {
        showHidden = value === true;
        hiddenButton.setAttribute('aria-pressed', showHidden ? 'true' : 'false');
        render();
      },
      focus() { if (typeof list.focus === 'function') list.focus(); },
      noticeHost,
      destroy() {
        listings.clear();
        expanded.clear();
        rowNodes = [];
        if (element.parentNode) element.remove();
      },
    };
  }

  global.termlabProjectTree = { create, sortEntries, joinPath };
})(window);
```

- [ ] **Step 4: Write the stylesheet.** Create `crates/termlab_tauri/frontend/styles/design-system/components/project-tree.css`:

```css
/* Project tree (app/panels/project-tree.js).

   Dense, like every other tree in the app: rows are --tl-row-h tall and each
   sits on one line. Indentation is applied inline per row (depth * 12px) so
   the stylesheet does not need a rule per level. Git tints arrive in
   [data-git-state] (see the git tokens in base.css) and are always paired
   with the state word in the row's aria-label, so colour is never the only
   signal. */

.tl-project-tree {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  height: 100%;
  overflow: hidden;
  color: var(--tl-fg);
  background: var(--tl-panel-bg);
  font: 400 var(--tl-font-size-ui) var(--tl-font-ui);
}

.tl-project-tree__toolbar {
  flex: 0 0 var(--tl-toolbar-h);
  display: flex;
  align-items: center;
  gap: var(--tl-space-1);
  min-width: 0;
  padding: 0 var(--tl-space-2);
  border-bottom: 1px solid var(--tl-border);
}

.tl-project-tree__title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--tl-fg-muted);
}

.tl-project-tree__button {
  height: var(--tl-row-h);
  padding: 0 var(--tl-space-2);
  color: var(--tl-fg);
  background: var(--tl-control-bg);
  border: 1px solid var(--tl-control-border);
  border-radius: var(--tl-radius);
  cursor: pointer;
}

.tl-project-tree__button:hover {
  background: var(--tl-row-hover);
}

.tl-project-tree__button:focus-visible {
  outline: 2px solid var(--tl-accent);
  outline-offset: 1px;
}

.tl-project-tree__button[aria-pressed="true"] {
  border-color: var(--tl-accent);
}

.tl-project-tree__notice-host {
  flex: 0 0 auto;
}

.tl-project-tree__missing {
  display: flex;
  align-items: center;
  gap: var(--tl-space-2);
  padding: var(--tl-space-2);
  color: var(--tl-warning-fg);
  background: var(--tl-warning-bg);
  border-bottom: 1px solid var(--tl-warning-border);
}

.tl-project-tree__missing-text {
  flex: 1;
  min-width: 0;
}

.tl-project-tree__list {
  flex: 1;
  min-height: 0;
  overflow: auto;
}

.tl-project-tree__list:focus-visible {
  outline: 2px solid var(--tl-accent);
  outline-offset: -2px;
}

.tl-project-tree__row {
  display: flex;
  align-items: center;
  gap: var(--tl-space-1);
  height: var(--tl-row-h);
  padding-right: var(--tl-space-2);
  white-space: nowrap;
  cursor: default;
}

.tl-project-tree__row:hover {
  background: var(--tl-row-hover);
}

.tl-project-tree__row.is-active {
  background: var(--tl-selection-bg);
  color: var(--tl-selection-fg);
}

.tl-project-tree__row:focus-visible {
  outline: 2px solid var(--tl-accent);
  outline-offset: -2px;
}

.tl-project-tree__twisty {
  display: inline-block;
  width: 12px;
  text-align: center;
  color: var(--tl-fg-muted);
}

.tl-project-tree__icon {
  display: inline-flex;
  align-items: center;
}

.tl-project-tree__label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
```

- [ ] **Step 5: Load it.** In `index.html`, add the stylesheet after `components/problems.css`:

```html
  <link rel="stylesheet" href="styles/design-system/components/project-tree.css" />
```

…and the script immediately before `app/panels/files-panel.js`:

```html
  <script src="app/panels/project-tree.js"></script>
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `node scripts/tests/test_project_tree.mjs` — all checks pass.
Run the full sweep — prints nothing.
Run: `bash scripts/check_frontend_boundaries.sh` — only `tl-dialog.js:334`.

- [ ] **Step 7: Commit**

```bash
git add crates/termlab_tauri/frontend/app/panels/project-tree.js \
        crates/termlab_tauri/frontend/styles/design-system/components/project-tree.css \
        crates/termlab_tauri/frontend/index.html \
        scripts/tests/test_project_tree.mjs
git commit -m "feat: add the lazy project tree module"
```

---

### Task 6: Context-aware Files panel — project tree vs dual-pane

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/panels/files-panel.js` (`init:426`, add `doNewFile` + `buildTreeContextMenuItems` + the mode branch near `doNewFolder:1494` and `buildRowContextMenuItems:1576`)
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js` (`registerBuiltInToolWindows:109`, `init:373`)
- Test: `scripts/tests/test_project_mode.mjs` (extend)

**Interfaces:**
- Consumes: `window.termlabProjectTree.create(options) -> handle` (Task 5); `window.termlabProjectMode.{isActive,root,name}` (Task 3); `project_reveal_path(path)` (Task 1); `local_stat(path)`, `local_mkdir`, `local_rename`, `local_remove`, `editor_write_file(path, contents)`.
- Produces:
  - `window.filesPanel.init(opts)` gains a `projectRoot` option; when set, the panel renders project mode.
  - `window.filesPanel.isProjectMode() -> boolean`, `window.filesPanel.setProjectMode(on: boolean)`, `window.filesPanel.projectTree() -> handle|null` — read by Tasks 7 and 11.
  - The SFTP tool window's `title` is `"Project"` in a project window and `"SFTP"` otherwise; the panel's own header carries the mode toggle.

- [ ] **Step 1: Write the failing test.** Append to `scripts/tests/test_project_mode.mjs`:

```javascript
const FILES_PANEL = path.join(APP, 'panels/files-panel.js');

check('the files panel switches on projectRoot and can toggle back to dual-pane', () => {
  const src = fs.readFileSync(FILES_PANEL, 'utf8');
  assert.ok(src.includes('opts.projectRoot'), 'init takes a project root');
  assert.ok(src.includes('termlabProjectTree'), 'project mode renders the tree module');
  assert.ok(src.includes('fp-pane-container'), 'the dual-pane path is still built for non-project windows');
  assert.ok(src.includes('isProjectMode'), 'the mode is queryable');
  assert.ok(src.includes('setProjectMode'), 'the header toggle can switch back to dual-pane');
  assert.ok(src.includes('projectTree'), 'the tree handle is reachable for git tints and the trust banner');
});

check('the tree context menu reuses the panel local operations plus new file and reveal', () => {
  const src = fs.readFileSync(FILES_PANEL, 'utf8');
  assert.ok(src.includes('buildTreeContextMenuItems'), 'the tree has its own item list');
  for (const label of ["'New File…'", "'New Folder…'", "'Rename…'", "'Delete'", "'Copy Path'", "'Reveal in File Manager'"]) {
    assert.ok(src.includes(label), `the tree context menu is missing ${label}`);
  }
  assert.ok(src.includes("invoke('project_reveal_path'"), 'reveal goes through the Rust command');
  assert.ok(src.includes('doNewFile'), 'New File is a real local operation, not a stub');
});

check('the SFTP tool window stays reachable and is registered with a project-aware title', () => {
  const src = fs.readFileSync(path.join(APP, 'tool-window-runtime.js'), 'utf8');
  assert.ok(src.includes('projectRoot'), 'the runtime hands the project root to the files panel');
  assert.ok(src.includes("termlabProjectMode"), 'the runtime asks the mode resolver');
  const register = src.indexOf("register('file-explorer'");
  const mode = src.indexOf('termlabProjectMode');
  assert.ok(mode < register || src.indexOf('projectRoot', register) > register,
    'the root must be known by the time file-explorer registers');
});

check('a project window opens with the Files tool window visible', () => {
  const src = fs.readFileSync(path.join(APP, 'tool-window-runtime.js'), 'utf8');
  assert.ok(src.includes("activate('file-explorer')"), 'a project window activates the Files panel');
  assert.ok(src.includes('knowsBottom'), 'and only when the layout has never recorded a bottom-zone window');
  const register = src.indexOf('registerBuiltInToolWindows();');
  const activate = src.indexOf("activate('file-explorer')");
  assert.ok(register < activate, 'the panel must be registered before it can be activated');
});

check('the panel modules use no regex lookbehind and no control bytes', () => {
  for (const file of [FILES_PANEL]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(!/\(\?<[=!]/.test(source), `${file} uses a lookbehind`);
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
        `${file}: control byte at offset ${i}`);
    }
  }
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `node scripts/tests/test_project_mode.mjs`
Expected: FAIL — `init takes a project root`.

- [ ] **Step 3: Add the two missing local operations.** In `files-panel.js`, beside `doNewFolder`:

```javascript
  // The tree's context menu offers New File, which the dual-pane explorer
  // never had. An empty write through the editor's own writer keeps one
  // definition of "create a text file" rather than adding a second command.
  function doNewFile(dirPath, afterCreate) {
    showTextPromptDialog({
      title: 'New File',
      label: 'Name',
      initialValue: '',
      confirmLabel: 'Create',
      onConfirm: (name) => {
        const target = joinPath(dirPath, name);
        Promise.resolve(invoke('editor_write_file', { path: target, contents: '' }))
          .then(() => { if (typeof afterCreate === 'function') afterCreate(); })
          .catch((e) => window.toast.error('New File Failed', String(e)));
      },
    });
  }

  function doRevealPath(targetPath) {
    Promise.resolve(invoke('project_reveal_path', { path: targetPath }))
      .catch((e) => window.toast.error('Reveal Failed', String(e)));
  }
```

- [ ] **Step 4: Add the project-mode state and rendering.** Near the top of `files-panel.js`, beside the other module-level `let`s:

```javascript
  // Project mode: this window has a project, so the panel renders a single
  // lazy tree instead of the dual-pane local+SFTP explorer. Per-window and
  // not persisted in v1 — the header toggle switches views for this session
  // only, which is what keeps SFTP fully reachable from a project window.
  let projectRoot = null;
  let projectMode = false;
  let projectTreeHandle = null;
  let projectRootMissing = false;
```

Then in `init`, right after `fitActiveTabFn`/`getActiveTabFn` are assigned:

```javascript
    projectRoot = opts.projectRoot || null;
    projectMode = !!projectRoot;
```

…and replace the `panelEl.innerHTML = …` block plus the splitter/home bootstrap with a call to a renderer, keeping the existing dual-pane body verbatim inside it:

```javascript
    renderPanelBody();
```

Add the renderer below `init` (the `renderDualPane` body is the code moved out of `init` unchanged — the `panelEl.innerHTML` template, the `termlabFilesSplit.attach` block and the `homePromise` block):

```javascript
  // The panel has exactly two shapes and one switch between them. Every
  // dual-pane behaviour below this line is untouched: a non-project window
  // takes renderDualPane and nothing else in this file behaves differently.
  function renderPanelBody() {
    if (!panelEl) return;
    if (projectTreeHandle) {
      projectTreeHandle.destroy();
      projectTreeHandle = null;
    }
    panelEl.innerHTML = '';
    if (projectMode && projectRoot) renderProjectTree();
    else renderDualPane();
  }

  function renderProjectTree() {
    const header = document.createElement('div');
    header.className = 'fp-project-header';
    const label = document.createElement('span');
    label.className = 'fp-project-header__name';
    label.textContent = (window.termlabProjectMode && window.termlabProjectMode.name()) || projectRoot;
    label.title = projectRoot;
    const toggle = document.createElement('button');
    toggle.type = 'button';
    toggle.className = 'tl-project-tree__button';
    toggle.textContent = 'SFTP';
    toggle.setAttribute('data-files-mode-toggle', 'dual');
    toggle.setAttribute('aria-label', 'Switch to the local and remote file explorer');
    toggle.addEventListener('click', () => setProjectMode(false));
    header.appendChild(label);
    header.appendChild(toggle);
    panelEl.appendChild(header);

    if (!window.termlabProjectTree || typeof window.termlabProjectTree.create !== 'function') {
      console.error('files-panel: project-tree module is unavailable');
      return;
    }
    projectTreeHandle = window.termlabProjectTree.create({
      invoke,
      root: projectRoot,
      showHidden: false,
      onOpenFile: (filePath) => {
        Promise.resolve(openTreeFile(filePath)).catch((error) => {
          console.error('files-panel: could not open in editor', error);
          window.toast.error('Could Not Open File', String(error));
        });
      },
      onContextMenu: (event, node) => {
        if (!filesPaneView || typeof filesPaneView.showRowContextMenu !== 'function') return;
        filesPaneView.showRowContextMenu(event, buildTreeContextMenuItems(node));
      },
      onReopen: () => {
        Promise.resolve(invoke('project_pick_folder'))
          .then((picked) => (picked ? invoke('project_open', { path: picked }) : null))
          .catch((e) => window.toast.error('Cannot Open Folder', String(e)));
      },
      toastError: (title, body) => window.toast.error(title, body),
    });
    panelEl.appendChild(projectTreeHandle.element);
    projectTreeHandle.refreshAll();
    checkProjectRootPresence();
  }

  // The same route the dual-pane explorer's local rows take: the editor
  // service owns the ownership protocol and the jump trail records the open.
  function openTreeFile(filePath) {
    const service = window.termlabEditorService;
    if (!service || typeof service.openLocalFile !== 'function') {
      window.toast.error('Could Not Open File', 'The editor is unavailable in this window.');
      return null;
    }
    return service.openLocalFile(filePath);
  }

  function setProjectMode(on) {
    if (!projectRoot) return;
    projectMode = on === true;
    renderPanelBody();
    if (typeof fitActiveTabFn === 'function') fitActiveTabFn();
  }

  function isProjectMode() {
    return projectMode;
  }

  function projectTree() {
    return projectTreeHandle;
  }

  // The root can vanish while the window is open. Open editor tabs are
  // untouched; only the tree changes state.
  function checkProjectRootPresence() {
    if (!projectRoot || !projectTreeHandle) return Promise.resolve();
    return Promise.resolve(invoke('local_stat', { path: projectRoot }))
      .then((entry) => {
        projectRootMissing = !(entry && entry.is_dir);
        projectTreeHandle.setMissing(projectRootMissing);
      })
      .catch(() => {
        projectRootMissing = true;
        projectTreeHandle.setMissing(true);
      });
  }
```

Add the dual-pane toggle back into the dual-pane header — inside `renderDualPane`, before the `.fp-pane-container` markup is inserted, prepend when a project exists:

```javascript
    if (projectRoot) {
      const header = document.createElement('div');
      header.className = 'fp-project-header';
      const label = document.createElement('span');
      label.className = 'fp-project-header__name';
      label.textContent = 'Local + Remote';
      const toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = 'tl-project-tree__button';
      toggle.textContent = 'Project';
      toggle.setAttribute('data-files-mode-toggle', 'project');
      toggle.setAttribute('aria-label', 'Switch to the project tree');
      toggle.addEventListener('click', () => setProjectMode(true));
      header.appendChild(label);
      header.appendChild(toggle);
      panelEl.appendChild(header);
    }
```

…and change `renderDualPane`'s existing `panelEl.innerHTML = \`…\`` to `panelEl.insertAdjacentHTML('beforeend', \`…\`)` so the header survives.

- [ ] **Step 5: Build the tree context menu.** Beside `buildRowContextMenuItems`:

```javascript
  // Reuses the panel's own local operations against a tree row. The dual-pane
  // list keeps buildRowContextMenuItems (which carries the transfer entries a
  // tree has no use for); this is the tree's list, and both go through the
  // same filesPaneView.showRowContextMenu renderer.
  function buildTreeContextMenuItems(node) {
    const parentDir = node.isDir ? node.path : node.parentPath;
    const reload = () => {
      if (!projectTreeHandle) return;
      projectTreeHandle.refresh(parentDir);
    };
    const pseudoPane = {
      isLocal: true,
      currentPath: parentDir,
      prefix: 'project',
    };
    return [
      { icon: 'newFile', label: 'New File…', action: () => doNewFile(parentDir, reload) },
      { icon: 'newFolder', label: 'New Folder…', action: () => doNewFolder(pseudoPane) },
      { type: 'separator' },
      { icon: 'edit', label: 'Rename…', action: () => doRename(pseudoPane, { name: node.name, is_dir: node.isDir }) },
      { icon: 'remove', label: 'Delete', danger: true, action: () => doDelete(pseudoPane, { name: node.name, is_dir: node.isDir }) },
      { type: 'separator' },
      { icon: 'copy', label: 'Copy Path', action: () => doCopyPath(pseudoPane, { name: node.name }) },
      { label: 'Reveal in File Manager', action: () => doRevealPath(node.path) },
      { type: 'separator' },
      { icon: 'refresh', label: 'Refresh', action: reload },
    ];
  }
```

`doNewFolder` / `doRename` / `doDelete` / `doCopyPath` all call `loadEntries(pane)` on success; make that call tolerant so a pseudo-pane refreshes the tree instead — change each of their `.then(() => loadEntries(pane))` to `.then(() => refreshAfterLocalOp(pane))` and add:

```javascript
  // One place that decides what "the view changed" means for the two shapes.
  function refreshAfterLocalOp(pane) {
    if (projectMode && projectTreeHandle) return projectTreeHandle.refresh(pane.currentPath);
    return loadEntries(pane);
  }
```

Export the new surface at the bottom of the module, alongside the existing exports:

```javascript
    isProjectMode,
    setProjectMode,
    projectTree,
    checkProjectRootPresence,
```

- [ ] **Step 6: Hand the root in, and refresh on focus.** In `tool-window-runtime.js`, at the top of `create(deps)`:

```javascript
    const projectMode = global.termlabProjectMode || null;
    const projectRoot = projectMode && typeof projectMode.root === 'function' ? projectMode.root() : null;
```

In `registerBuiltInToolWindows`, change the `file-explorer` registration's title and pass the root:

```javascript
      global.toolWindowManager.register('file-explorer', {
        title: projectRoot ? 'Project' : 'SFTP',
```

```javascript
            global.filesPanel.init({
              invoke,
              listen: listenOnCurrentWindow,
              onDragDropEvent,
              panelEl,
              panelWrapEl: document.getElementById('left-sidebar'),
              resizeHandleEl: null,
              layoutService,
              fitActiveTab: debouncedFitAndResize,
              getActiveTab: () => getCurrentTab(),
              projectRoot,
            });
```

Immediately after the existing `registerBuiltInToolWindows();` call inside `init()`:

```javascript
        // Spec section 1: a project window opens with the Files tool window
        // visible in its zone. Applied only when the layout this window booted
        // with has never recorded a bottom-zone window — a project that HAS a
        // saved layout always records one (save_window_layout writes
        // active_tool_windows every time), so a user who closed the panel in
        // this project keeps it closed.
        if (projectRoot) {
          const savedActive = (initialLayoutData && initialLayoutData.active_tool_windows) || {};
          const knowsBottom = Object.keys(savedActive)
            .some((zone) => String(zone).startsWith('bottom'));
          if (!knowsBottom) {
            global.toolWindowManager.setPanelVisibility('bottom', true, { save: false });
            global.toolWindowManager.activate('file-explorer');
          }
        }
```

At the end of `init()`, before the `return`:

```javascript
      // No filesystem watcher in v1: freshness comes from explicit triggers.
      // Window focus is the one that matters — the user has just come back
      // from an editor, a terminal, or another app that changed files.
      if (projectRoot) {
        global.addEventListener('focus', () => {
          if (!global.filesPanel || !global.filesPanel.isProjectMode()) return;
          const tree = global.filesPanel.projectTree();
          if (tree) tree.refreshAll();
          global.filesPanel.checkProjectRootPresence();
        });
      }
```

- [ ] **Step 7: Style the header.** Append to `styles/design-system/components/project-tree.css`:

```css
/* The files panel's mode header — the one control that switches a project
   window between the project tree and the classic local+remote explorer, so
   SFTP is never more than one click away from a project. */
.fp-project-header {
  flex: 0 0 var(--tl-toolbar-h);
  display: flex;
  align-items: center;
  gap: var(--tl-space-2);
  min-width: 0;
  padding: 0 var(--tl-space-2);
  border-bottom: 1px solid var(--tl-border);
}

.fp-project-header__name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--tl-fg);
}
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `node scripts/tests/test_project_mode.mjs && node scripts/tests/test_project_tree.mjs` — pass.
Run the full sweep — prints nothing (`test_files_*` suites must stay green: the dual-pane path is unchanged).
Run: `bash scripts/check_frontend_boundaries.sh` — only `tl-dialog.js:334`.

- [ ] **Step 9: Commit**

```bash
git add crates/termlab_tauri/frontend/app/panels/files-panel.js \
        crates/termlab_tauri/frontend/app/tool-window-runtime.js \
        crates/termlab_tauri/frontend/styles/design-system/components/project-tree.css \
        scripts/tests/test_project_mode.mjs
git commit -m "feat: render the project tree in the files panel with an SFTP toggle"
```

---

### Task 7: Trust banner and LSP root integration

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/project/trust-banner.js`
- Create: `crates/termlab_tauri/frontend/app/features/project/lsp-root.js`
- Modify: `crates/termlab_tauri/frontend/app/panels/files-panel.js` (`renderProjectTree`, mount the banner into `projectTreeHandle.noticeHost`)
- Modify: `crates/termlab_tauri/frontend/styles/design-system/components/project-tree.css`
- Modify: `crates/termlab_tauri/frontend/index.html`
- Test: `scripts/tests/test_project_mode.mjs` (extend)

**Interfaces:**
- Consumes: `window.termlabLspBridge.{setProjectTrust(root, adapterId, decision), setProjectContext(documentId, context), trustedProjects()}`; `window.termlabLspState.subscribe(fn)` and `.get(pane)`; `window.termlabProjectMode.{root,isUnderRoot,isActive}`.
- Produces:
  - `window.termlabProjectTrustBanner.mount({ host, root, bridge, onDecision }) -> { element, destroy }` and the pure `decide(trustedProjects, root) -> 'ask' | 'settled'`
  - `window.termlabProjectLspRoot.install({ state, bridge, mode })` — subscribes once per window and answers `choosingProject` for a file under the root by setting the project root as the context; returns an unsubscribe function. Also exports the pure `shouldAdoptRoot(paneState, filePath, mode) -> boolean`.

- [ ] **Step 1: Write the failing test.** Append to `scripts/tests/test_project_mode.mjs`:

```javascript
const BANNER = path.join(APP, 'features/project/trust-banner.js');
const LSP_ROOT = path.join(APP, 'features/project/lsp-root.js');

check('the banner asks once for an untrusted root and never for a trusted one', () => {
  const sandbox = load([BANNER]);
  const decide = sandbox.termlabProjectTrustBanner.decide;
  assert.strictEqual(decide([], '/repo'), 'ask', 'no record means ask');
  assert.strictEqual(
    decide([{ root: '/repo', adapterId: null, decision: 'trusted' }], '/repo'),
    'settled',
    'an existing trust decision means the banner never returns',
  );
  assert.strictEqual(
    decide([{ root: '/repo', adapterId: null, decision: 'denied' }], '/repo'),
    'settled',
    'a recorded denial is also a decision — do not nag',
  );
  assert.strictEqual(
    decide([{ root: '/other', adapterId: null, decision: 'trusted' }], '/repo'),
    'ask',
    'a different project says nothing about this one',
  );
});

check('Trust project calls lsp_set_project_trust with the root and a trusted decision', async () => {
  const sandbox = load([BANNER]);
  const { body } = (() => {
    const host = sandbox.document.createElement('div');
    sandbox.document.body.appendChild(host);
    return { body: host };
  })();
  const calls = [];
  const decisions = [];
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host: body,
    root: '/repo',
    bridge: {
      trustedProjects: async () => [],
      setProjectTrust: async (root, adapterId, decision) => { calls.push([root, adapterId, decision]); },
    },
    onDecision: (d) => { decisions.push(d); },
  });
  await handle.ready;
  const trust = body.querySelector('[data-project-trust="trust"]');
  assert.ok(trust, 'the banner offers Trust project');
  assert.ok(body.querySelector('[data-project-trust="later"]'), 'and Not now');
  trust.dispatchEvent({ type: 'click', target: trust });
  await handle.settled();
  deepEq(calls, [['/repo', null, 'trusted']]);
  deepEq(decisions, ['trusted']);
  assert.strictEqual(body.querySelector('[data-project-trust="trust"]'), null, 'the banner leaves once decided');
});

check('Not now dismisses for the window lifetime without touching trust', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const calls = [];
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    bridge: { trustedProjects: async () => [], setProjectTrust: async (...a) => { calls.push(a); } },
    onDecision: () => {},
  });
  await handle.ready;
  const later = host.querySelector('[data-project-trust="later"]');
  later.dispatchEvent({ type: 'click', target: later });
  await handle.settled();
  deepEq(calls, [], 'Not now must never write a trust record');
  assert.strictEqual(host.querySelector('[data-project-trust="later"]'), null, 'the banner is gone for this window');
});

check('an already-trusted project never renders a banner at all', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    bridge: {
      trustedProjects: async () => [{ root: '/repo', adapterId: null, decision: 'trusted' }],
      setProjectTrust: async () => {},
    },
    onDecision: () => {},
  });
  await handle.ready;
  assert.strictEqual(host.children.length, 0, 'nothing is rendered for a settled project');
});

check('a file under the root adopts the project root as its LSP context', () => {
  const sandbox = load([path.join(APP, 'features/project/project-mode.js'), LSP_ROOT]);
  const mode = sandbox.termlabProjectMode;
  mode.set({ root: '/repo', name: 'repo' });
  const should = sandbox.termlabProjectLspRoot.shouldAdoptRoot;
  const choosing = { documentId: 'doc-1', status: { state: 'choosingProject' } };
  assert.strictEqual(should(choosing, '/repo/src/main.rs', mode), true);
  assert.strictEqual(should(choosing, '/elsewhere/lib.rs', mode), false,
    'a file outside the root keeps the loose-file behaviour: no prompt, no attach');
  assert.strictEqual(should({ documentId: 'doc-1', status: { state: 'ready' } }, '/repo/src/main.rs', mode), false,
    'a document that already has a context is left alone');
  assert.strictEqual(should(choosing, '/repo/src/main.rs', { isActive: () => false, isUnderRoot: () => false }), false,
    'no project means no adoption');
});

check('install sets the context exactly once per document', async () => {
  const sandbox = load([path.join(APP, 'features/project/project-mode.js'), LSP_ROOT]);
  sandbox.termlabProjectMode.set({ root: '/repo', name: 'repo' });
  let listener = null;
  const contexts = [];
  const unsubscribe = sandbox.termlabProjectLspRoot.install({
    state: {
      subscribe: (fn) => { listener = fn; return () => { listener = null; }; },
    },
    bridge: {
      setProjectContext: async (documentId, context) => { contexts.push([documentId, context]); },
    },
    mode: sandbox.termlabProjectMode,
  });
  const pane = { kind: 'editor', remote: null, filePath: '/repo/src/main.rs' };
  listener(pane, { documentId: 'doc-1', status: { state: 'choosingProject' } });
  listener(pane, { documentId: 'doc-1', status: { state: 'choosingProject' } });
  await new Promise((resolve) => setTimeout(resolve, 0));
  deepEq(contexts, [['doc-1', { kind: 'root', root: '/repo' }]]);
  assert.strictEqual(typeof unsubscribe, 'function');
});

check('the project feature modules use no regex lookbehind and no control bytes', () => {
  for (const file of [BANNER, LSP_ROOT]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(!/\(\?<[=!]/.test(source), `${file} uses a lookbehind`);
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
        `${file}: control byte at offset ${i}`);
    }
  }
});

check('index.html loads the trust banner and the LSP root adapter after the bridge', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(name);
  assert.ok(at('app/features/project/trust-banner.js') > 0);
  assert.ok(at('app/features/project/lsp-root.js') > 0);
  assert.ok(at('app/features/editor/lsp-bridge.js') < at('app/features/project/lsp-root.js'));
  assert.ok(at('app/features/editor/lsp-state.js') < at('app/features/project/lsp-root.js'));
  assert.ok(at('app/features/project/trust-banner.js') < at('app/panels/files-panel.js'));
});
```

This test needs a DOM: extend `test_project_mode.mjs`'s `load()` to build the same `document` stand-in `test_project_tree.mjs` uses — copy `makeElement`/`selectorParts`/`dataName` from `test_problems_panel.mjs` and give the sandbox `document`, `setTimeout`, `clearTimeout` and `Set`/`Map`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `node scripts/tests/test_project_mode.mjs`
Expected: FAIL — `Cannot find module .../features/project/trust-banner.js`.

- [ ] **Step 3: Write the trust banner.** Create `crates/termlab_tauri/frontend/app/features/project/trust-banner.js`:

```javascript
// One trust ask per project, as a non-blocking banner in the project tree's
// header area — never a modal. Editing is never blocked either way: this
// decides whether language servers start, not whether the file opens.
//
// "Trust project" calls the existing lsp_set_project_trust, whose persistence
// is what makes the banner never return for that project. "Not now" dismisses
// for this window's lifetime only and writes nothing — the per-file status
// strip (features/editor/project-context.js) stays the way to trust later.
(function initTermLabProjectTrustBanner(global) {
  'use strict';

  // A project with ANY recorded decision is settled: a recorded denial is a
  // decision too, and re-asking would be nagging.
  function decide(trustedProjects, root) {
    const records = Array.isArray(trustedProjects) ? trustedProjects : [];
    const settled = records.some((record) => record && String(record.root) === String(root));
    return settled ? 'settled' : 'ask';
  }

  function el(tag, className) {
    const node = global.document.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  function mount(options) {
    const opts = options || {};
    const host = opts.host;
    const root = String(opts.root || '');
    const bridge = opts.bridge || global.termlabLspBridge || null;
    const onDecision = typeof opts.onDecision === 'function' ? opts.onDecision : () => {};

    let element = null;
    let pending = Promise.resolve();

    function remove() {
      if (element && element.parentNode) element.remove();
      element = null;
    }

    function build() {
      const banner = el('div', 'tl-project-banner');
      banner.setAttribute('role', 'note');
      const text = el('span', 'tl-project-banner__text');
      text.textContent = 'Trust this project and start language servers?';
      const trust = el('button', 'tl-project-tree__button');
      trust.type = 'button';
      trust.textContent = 'Trust project';
      trust.setAttribute('data-project-trust', 'trust');
      trust.addEventListener('click', () => {
        // adapterId is null: the decision is about the PROJECT, not one
        // language server in it, which is the whole point of asking once.
        pending = Promise.resolve(bridge.setProjectTrust(root, null, 'trusted'))
          .then(() => { remove(); onDecision('trusted'); })
          .catch((error) => {
            if (global.toast) global.toast.error('Trust Failed', String(error));
          });
      });
      const later = el('button', 'tl-project-tree__button');
      later.type = 'button';
      later.textContent = 'Not now';
      later.setAttribute('data-project-trust', 'later');
      later.addEventListener('click', () => {
        remove();
        onDecision('later');
      });
      banner.appendChild(text);
      banner.appendChild(trust);
      banner.appendChild(later);
      return banner;
    }

    const ready = (async () => {
      if (!host || !bridge || typeof bridge.trustedProjects !== 'function') return;
      let records = [];
      try {
        records = await bridge.trustedProjects();
      } catch (error) {
        // Unreadable trust store: do not ask, and do not claim trust either.
        console.warn('project trust banner: could not read trusted projects', error);
        return;
      }
      if (decide(records, root) !== 'ask') return;
      element = build();
      host.appendChild(element);
    })();

    return {
      ready,
      settled: () => pending,
      get element() { return element; },
      destroy: remove,
    };
  }

  global.termlabProjectTrustBanner = { decide, mount };
})(window);
```

- [ ] **Step 4: Write the LSP root adapter.** Create `crates/termlab_tauri/frontend/app/features/project/lsp-root.js`:

```javascript
// Opening a project IS choosing the LSP root.
//
// When a document in a project window comes up `choosingProject` and its file
// lives under the project root, the project root is set as its context through
// the existing lsp_set_project_context path — so the per-file root-candidate
// chooser never appears inside a project window for files under the root.
//
// Files OUTSIDE the root (a `gd` into std, or a cargo-registry source) are
// deliberately left alone: they keep the loose-file behaviour — a plain
// editable tab, no prompts, no attach.
(function initTermLabProjectLspRoot(global) {
  'use strict';

  function shouldAdoptRoot(paneState, filePath, mode) {
    if (!paneState || !paneState.documentId) return false;
    const status = paneState.status;
    if (!status || status.state !== 'choosingProject') return false;
    if (!mode || typeof mode.isActive !== 'function' || !mode.isActive()) return false;
    return mode.isUnderRoot(filePath);
  }

  function install(options) {
    const opts = options || {};
    const state = opts.state || global.termlabLspState || null;
    const bridge = opts.bridge || global.termlabLspBridge || null;
    const mode = opts.mode || global.termlabProjectMode || null;
    if (!state || typeof state.subscribe !== 'function') return function () {};
    if (!bridge || typeof bridge.setProjectContext !== 'function') return function () {};

    // Once per document: the manager republishes status on every revision, and
    // re-sending the same choice would restart the session in a loop.
    const answered = new Set();

    return state.subscribe((pane, paneState) => {
      if (!pane || pane.kind !== 'editor' || pane.remote || !pane.filePath) return;
      if (!shouldAdoptRoot(paneState, pane.filePath, mode)) return;
      const documentId = String(paneState.documentId);
      if (answered.has(documentId)) return;
      answered.add(documentId);
      Promise.resolve(bridge.setProjectContext(documentId, { kind: 'root', root: mode.root() }))
        .catch((error) => {
          answered.delete(documentId);
          console.warn('project lsp root: could not set the project context', error);
        });
    });
  }

  global.termlabProjectLspRoot = { shouldAdoptRoot, install };
})(window);
```

- [ ] **Step 5: Mount them.** In `files-panel.js`'s `renderProjectTree`, after `panelEl.appendChild(projectTreeHandle.element);`:

```javascript
    if (window.termlabProjectTrustBanner && typeof window.termlabProjectTrustBanner.mount === 'function') {
      window.termlabProjectTrustBanner.mount({
        host: projectTreeHandle.noticeHost,
        root: projectRoot,
        bridge: window.termlabLspBridge,
        onDecision: () => {},
      });
    }
```

In `tool-window-runtime.js`'s `init()`, beside the focus-refresh block added in Task 6:

```javascript
      if (projectRoot && global.termlabProjectLspRoot) {
        global.termlabProjectLspRoot.install({});
      }
```

In `index.html`, add both scripts after `app/features/editor/lsp-state.js`/`lsp-bridge.js` and before `app/panels/project-tree.js`:

```html
  <script src="app/features/project/trust-banner.js"></script>
  <script src="app/features/project/lsp-root.js"></script>
```

- [ ] **Step 6: Style the banner.** Append to `project-tree.css`:

```css
/* The one trust ask per project. A banner, never a modal: editing is not
   blocked by the decision, so the UI must not block on it either. */
.tl-project-banner {
  display: flex;
  align-items: center;
  gap: var(--tl-space-2);
  padding: var(--tl-space-2);
  color: var(--tl-fg);
  background: var(--tl-warning-bg);
  border-bottom: 1px solid var(--tl-warning-border);
}

.tl-project-banner__text {
  flex: 1;
  min-width: 0;
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `node scripts/tests/test_project_mode.mjs` — pass.
Run: `node scripts/tests/test_project_context.mjs && node scripts/tests/test_lsp_navigation.mjs` — pass (the per-file chooser is untouched for files outside a project).
Run the full sweep — prints nothing.

- [ ] **Step 8: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/project/trust-banner.js \
        crates/termlab_tauri/frontend/app/features/project/lsp-root.js \
        crates/termlab_tauri/frontend/app/panels/files-panel.js \
        crates/termlab_tauri/frontend/app/tool-window-runtime.js \
        crates/termlab_tauri/frontend/styles/design-system/components/project-tree.css \
        crates/termlab_tauri/frontend/index.html \
        scripts/tests/test_project_mode.mjs
git commit -m "feat: ask for project trust once and adopt the project root as the LSP context"
```

---

### Task 8: Rust — the project search walker

**Files:**
- Create: `crates/termlab_tauri/src/project/search.rs`
- Modify: `crates/termlab_tauri/src/project/mod.rs` (add `pub(crate) mod search;`)
- Modify: `crates/termlab_tauri/Cargo.toml` (add `ignore`)
- Modify: `crates/termlab_tauri/src/lib.rs` (`.manage(`, `generate_handler!`)
- Test: `#[cfg(test)] mod tests` at the bottom of `crates/termlab_tauri/src/project/search.rs`

**New dependency — justification:** `ignore = "0.4"` is ripgrep's own directory walker. The spec requires `.gitignore` / `.ignore` / global-exclude semantics without depending on an `rg` binary existing on the host, and hand-rolling gitignore precedence (nested files, negations, `**` globs, global excludes, `.git/info/exclude`) is a well-known correctness trap. Nothing else in this plan adds a dependency.

**Interfaces:**
- Consumes: `ProjectRegistry::root_for(label)` (Task 1).
- Produces:
  - `pub(crate) struct SearchOptions { pub query: String, pub case_sensitive: bool }`
  - `pub(crate) struct SearchMatch { pub path: String, pub relative_path: String, pub line: u32, pub column: u32, pub preview: String }` (serde camelCase)
  - `pub(crate) struct SearchOutcome { pub matched: usize, pub capped: bool, pub cancelled: bool }`
  - `pub(crate) const MAX_MATCHES: usize = 1000;`, `pub(crate) const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;`, `pub(crate) const BATCH_SIZE: usize = 100;`
  - `pub(crate) fn looks_binary(head: &[u8]) -> bool`
  - `pub(crate) fn match_lines(bytes: &[u8], options: &SearchOptions) -> Vec<(u32, u32, String)>` — 1-based line, 1-based column, trimmed preview
  - `pub(crate) fn run_search(root: &Path, options: &SearchOptions, cancel: &AtomicBool, sink: impl FnMut(SearchMatch)) -> SearchOutcome`
  - `pub(crate) struct SearchRegistry` with `start(&mut self, label: String) -> (String, Arc<AtomicBool>)` (cancels any previous search for that label and returns the new id + flag) and `cancel(&mut self, label: &str)`
  - commands `project_search(window, query, case_sensitive) -> Result<String, String>` and `project_search_cancel(window)`
  - event `project-search-results`, payload `{ searchId, matches: SearchMatch[], done: bool, capped: bool }`, emitted to the calling window only

- [ ] **Step 1: Write the failing tests.** Create `crates/termlab_tauri/src/project/search.rs` with only this test module:

```rust
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
        assert_eq!(hits[0].1, 9, "columns are 1-based and count bytes into the line");
        assert_eq!(hits[0].2, "let needle = 1;", "the preview is the trimmed line");
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
        assert_eq!(found[0].relative_path, "kept.rs", "paths are relative to the root");
        assert!(found[0].path.ends_with("kept.rs"), "the absolute path is carried too");
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
        assert_eq!(count, MAX_MATCHES, "never more than the cap reaches the sink");
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_tauri --quiet project::search`
Expected: compile FAIL — `file not found for module search` until `mod.rs` declares it, then `cannot find function run_search`.

- [ ] **Step 3: Add the dependency.** In `crates/termlab_tauri/Cargo.toml`, in `[dependencies]` (alphabetically after `hostname`):

```toml
# ripgrep's directory walker. The project search must honour .gitignore /
# .ignore / global excludes without depending on an `rg` binary existing on
# the user's machine, and gitignore precedence is not worth re-implementing.
ignore = "0.4"
```

- [ ] **Step 4: Write the implementation.** Prepend to `crates/termlab_tauri/src/project/search.rs`:

```rust
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
    // parent ignore files honoured, global excludes applied.
    let walker = ignore::WalkBuilder::new(root).build();
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
```

In `project/mod.rs`, immediately below the module doc comment:

```rust
pub(crate) mod search;
```

In `lib.rs`, after the project registry `.manage(…)`:

```rust
        .manage(Mutex::new(project::search::SearchRegistry::default()))
```

…and in `generate_handler!`, beside the other project commands:

```rust
                project::search::project_search,
                project::search::project_search_cancel,
```

Add the window-destroy cleanup: in `project::on_window_destroyed`, insert this immediately after `let app = window.app_handle();` and **before** the `let Some(registry) = … else { return; }` guard — a window whose project registry is somehow unmanaged must still have its walk cancelled:

```rust
    if let Some(searches) = app.try_state::<Mutex<search::SearchRegistry>>() {
        searches.lock().cancel(window.label());
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p termlab_tauri --quiet project::search` — PASS (11 tests).
Run: `cargo test --workspace --quiet` — PASS.
Run: `cargo clippy --all-targets` — no new warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/Cargo.toml Cargo.lock \
        crates/termlab_tauri/src/project/search.rs \
        crates/termlab_tauri/src/project/mod.rs \
        crates/termlab_tauri/src/lib.rs
git commit -m "feat: add the project search walker with streaming results and cancellation"
```

---

### Task 9: The Search tool window

**Files:**
- Create: `crates/termlab_tauri/frontend/app/panels/project-search-panel.js`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/project-search.css`
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js` (`registerBuiltInToolWindows`)
- Modify: `crates/termlab_tauri/frontend/app/shortcut-runtime.js` (`coreShortcutActionByKey:28`)
- Modify: `crates/termlab_tauri/frontend/app/menu-actions.js`
- Modify: `crates/termlab_tauri/frontend/app/command-palette-runtime.js`
- Modify: `crates/termlab_core/src/config/termlab.rs` (`KeyboardConfig:99`, `Default:136`)
- Modify: `crates/termlab_tauri/frontend/index.html`
- Test: `scripts/tests/test_project_search_panel.mjs` (new); `crates/termlab_core/src/config/mod.rs` test module

**Interfaces:**
- Consumes: `project_search(query, caseSensitive) -> searchId`, `project_search_cancel()`, event `project-search-results` `{ searchId, matches, done, capped }` (Task 8); `window.termlabEditorService.openLocalFileAt(path, range, options)`; `window.toolWindowManager.{register, activate}`.
- Produces:
  - `window.projectSearchPanel.init({ panelEl, invoke, listen }) -> { element, focusInput, destroy }`
  - `window.projectSearchPanel.groupByFile(matches) -> [{ relativePath, path, matches }]` (pure, exported for the suite)
  - tool-window id `'project-search'`, title `'Search'`, `defaultZone: 'bottom'`, registered **last** among the bottom-zone built-ins and **only** when the window has a project
  - `KeyboardConfig::search_in_project` defaulting to `"cmd+shift+f"`, action string `search-in-project`

- [ ] **Step 1: Write the failing tests.** Create `scripts/tests/test_project_search_panel.mjs`, reusing `test_problems_panel.mjs`'s DOM stand-in and `load()` helper verbatim, then:

```javascript
const PANEL = path.join(APP, 'panels/project-search-panel.js');
const CSS = path.join(ROOT, 'styles/design-system/components/project-search.css');
const TOOL_WINDOW_MANAGER = path.join(APP, 'layout/tool-window-manager.js');

function panelHarness(options) {
  const opts = options || {};
  const { sandbox, body } = load([PANEL]);
  const invoked = [];
  let listener = null;
  const panelEl = sandbox.document.createElement('div');
  body.appendChild(panelEl);
  const opens = [];
  sandbox.termlabEditorService = {
    openLocalFileAt: (p, range, o) => { opens.push([p, range, o]); return Promise.resolve({ status: 'opened' }); },
  };
  const handle = sandbox.projectSearchPanel.init({
    panelEl,
    invoke: async (cmd, args) => {
      invoked.push([cmd, args]);
      if (cmd === 'project_search') return 'search-1';
      return null;
    },
    listen: (name, fn) => { if (name === 'project-search-results') listener = fn; return () => {}; },
  });
  return {
    sandbox, body, panelEl, handle, invoked, opens,
    publish: (payload) => listener && listener({ payload }),
    input: () => panelEl.querySelector('.tl-project-search__input'),
    status: () => {
      const node = panelEl.querySelector('.tl-project-search__status');
      return node ? node.textContent : null;
    },
    rows: () => Array.from(panelEl.querySelectorAll('[data-search-row]')),
  };
}

const hit = (relativePath, line, preview) => ({
  path: '/repo/' + relativePath, relativePath, line, column: 1, preview,
});

check('groupByFile keeps file order and gathers each file once', () => {
  const { sandbox } = panelHarness();
  const grouped = sandbox.projectSearchPanel.groupByFile([
    hit('a.rs', 1, 'x'), hit('b.rs', 2, 'y'), hit('a.rs', 5, 'z'),
  ]);
  deepEq(grouped.map((g) => [g.relativePath, g.matches.length]), [['a.rs', 2], ['b.rs', 1]]);
});

check('the empty state is explicit before anything is typed', () => {
  const h = panelHarness();
  assert.ok(h.status().includes('Search'), `expected an inviting empty state, got ${h.status()}`);
  deepEq(h.invoked, [], 'nothing is searched until asked');
});

check('submitting runs a search and shows a searching state', () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  deepEq(h.invoked, [['project_search', { query: 'needle', caseSensitive: false }]]);
  assert.ok(h.status().includes('Searching'), `expected a searching state, got ${h.status()}`);
});

check('the case toggle is carried into the query', () => {
  const h = panelHarness();
  const toggle = h.panelEl.querySelector('[data-search-case]');
  toggle.dispatchEvent({ type: 'click', target: toggle });
  h.input().value = 'Needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  deepEq(h.invoked, [['project_search', { query: 'Needle', caseSensitive: true }]]);
});

check('batches render grouped by file and the terminal event reports the count', () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'one'), hit('b.rs', 3, 'three')], done: false, capped: false });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 9, 'nine')], done: true, capped: false });
  const labels = h.rows().map((r) => r.getAttribute('data-search-row'));
  deepEq(labels, ['file', 'match', 'match', 'file', 'match']);
  assert.ok(h.status().includes('3'), `the terminal state counts the matches, got ${h.status()}`);
});

check('the capped terminal event says so', () => {
  const h = panelHarness();
  h.input().value = 'e';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'e')], done: true, capped: true });
  assert.ok(h.status().includes('first 1000'), `expected the cap wording, got ${h.status()}`);
});

check('no results is its own state, not an empty list', () => {
  const h = panelHarness();
  h.input().value = 'zzz';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-1', matches: [], done: true, capped: false });
  assert.ok(h.status().includes('No results'), `got ${h.status()}`);
});

check('a superseded search id is ignored', () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-0', matches: [hit('stale.rs', 1, 'stale')], done: true, capped: false });
  deepEq(h.rows(), [], 'results from a cancelled query must never land');
});

check('activating a match opens the file at the line through the editor service', () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 4, 'needle')], done: true, capped: false });
  const match = h.rows().find((r) => r.getAttribute('data-search-row') === 'match');
  match.dispatchEvent({ type: 'click', target: match });
  deepEq(h.opens, [[
    '/repo/a.rs',
    { start: { line: 3, character: 0 }, end: { line: 3, character: 0 } },
    { focus: true },
  ]], 'LSP positions are 0-based; a 1-based search line converts once, here');
});

check('Search never steals the bottom zone from SFTP on a fresh profile', () => {
  const { sandbox } = load([TOOL_WINDOW_MANAGER]);
  const twm = sandbox.toolWindowManager;
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('file-explorer', { title: 'Project', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  twm.register('problems', { title: 'Problems', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  twm.register('project-search', { title: 'Search', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  assert.strictEqual(twm.isVisible('project-search'), false, 'Search must not be what a user finds open');
  assert.strictEqual(twm.isVisible('file-explorer'), true);
});

check('the runtime registers Search last, and only for a project window', () => {
  const src = fs.readFileSync(path.join(APP, 'tool-window-runtime.js'), 'utf8');
  assert.ok(src.includes("register('project-search'"), 'the Search tool window is registered');
  const problems = src.indexOf("register('problems'");
  const search = src.indexOf("register('project-search'");
  assert.ok(problems < search, 'Search registers after the existing bottom-zone registrants');
  const guard = src.slice(0, search).lastIndexOf('if (projectRoot)');
  assert.ok(guard > problems, 'the registration is guarded on the window having a project');
});

check('cmd+shift+f is the shipped default and routes to one action', () => {
  const shortcuts = fs.readFileSync(path.join(APP, 'shortcut-runtime.js'), 'utf8');
  assert.ok(shortcuts.includes("search_in_project: 'search-in-project'"), 'the core table carries the action');
  const actions = fs.readFileSync(path.join(APP, 'menu-actions.js'), 'utf8');
  assert.ok(actions.includes("action === 'search-in-project'"), 'the action is handled');
  assert.ok(actions.includes("activate('project-search')"), 'it activates the Search tool window');
  const palette = fs.readFileSync(path.join(APP, 'command-palette-runtime.js'), 'utf8');
  assert.ok(palette.includes("'core:search-in-project'"), 'the palette exposes Search in Project');
  const config = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_core/src/config/termlab.rs'), 'utf8');
  assert.ok(config.includes('search_in_project'), 'the keyboard config has the binding');
  assert.ok(config.includes('"cmd+shift+f"'), 'shipped default is cmd+shift+f');
});

check('project-search.css styles every class the panel renders, with tokens only', () => {
  const css = fs.readFileSync(CSS, 'utf8');
  for (const name of [
    'tl-project-search', 'tl-project-search__toolbar', 'tl-project-search__input',
    'tl-project-search__list', 'tl-project-search__row', 'tl-project-search__status',
    'tl-project-search__where', 'tl-project-search__preview',
  ]) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(css), 'design-system components use tokens only');
  assert.ok(/focus-visible/.test(css), 'rows need a strong focus state');
});

check('the search panel uses no regex lookbehind and no control bytes', () => {
  for (const file of [PANEL, CSS]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(!/\(\?<[=!]/.test(source), `${file} uses a lookbehind`);
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
        `${file}: control byte at offset ${i}`);
    }
  }
});

check('index.html loads the search panel and its stylesheet before the runtime', () => {
  const html = fs.readFileSync(path.join(ROOT, 'index.html'), 'utf8');
  assert.ok(html.indexOf('app/panels/project-search-panel.js') > 0);
  assert.ok(html.indexOf('styles/design-system/components/project-search.css') > 0);
  assert.ok(html.indexOf('app/panels/project-search-panel.js') < html.indexOf('app/tool-window-runtime.js'));
});
```

Also append to `crates/termlab_core/src/config/mod.rs`'s test module:

```rust
    #[test]
    fn search_in_project_defaults_to_cmd_shift_f_and_survives_an_override() {
        let cfg = UserConfig::default();
        assert_eq!(cfg.termlab.keyboard.search_in_project, "cmd+shift+f");
        let overridden: UserConfig =
            toml::from_str("[termlab.keyboard]\nsearch_in_project = \"ctrl+alt+f\"\n")
                .expect("parse");
        assert_eq!(overridden.termlab.keyboard.search_in_project, "ctrl+alt+f");
        // Back-compat: a config written before this key existed still parses.
        let legacy: UserConfig =
            toml::from_str("[termlab.keyboard]\nnew_tab = \"cmd+t\"\n").expect("parse");
        assert_eq!(legacy.termlab.keyboard.search_in_project, "cmd+shift+f");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `node scripts/tests/test_project_search_panel.mjs`
Expected: FAIL — `Cannot find module .../panels/project-search-panel.js`.
Run: `cargo test -p termlab_core --quiet search_in_project`
Expected: FAIL — `no field search_in_project`.

- [ ] **Step 3: Add the keyboard binding.** In `crates/termlab_core/src/config/termlab.rs`, in `KeyboardConfig` after `editor_previous_problem`:

```rust
    pub search_in_project: String,
```

…and in its `Default` impl, in the same position:

```rust
            search_in_project: "cmd+shift+f".into(),
```

- [ ] **Step 4: Write the panel.** Create `crates/termlab_tauri/frontend/app/panels/project-search-panel.js`:

```javascript
// Project-wide text search — bottom zone, project windows only.
//
// The backend streams batches on `project-search-results`; this renders them
// grouped by file as they arrive, so a first hit in a large tree is on screen
// long before the walk finishes. Every emission carries the search id it
// belongs to, and anything from a superseded id is dropped on the floor — a
// cancelled search is silent by design.
//
// Row activation goes through editor-service's openLocalFileAt, which owns the
// app-wide ownership protocol and puts the jump on the Ctrl-O trail. The one
// conversion this file performs is 1-based search coordinates to the 0-based
// LSP positions that the editor's range API speaks; it happens exactly once,
// here, so no caller downstream has to know.
//
// Every string that came from the filesystem is written with textContent.
(function initTermLabProjectSearchPanel(global) {
  'use strict';

  const RESULTS_EVENT = 'project-search-results';
  const MAX_MATCHES = 1000;

  function el(tag, className) {
    const node = global.document.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  // Stable file order (first appearance wins) so a streaming batch never
  // reshuffles rows the user is reading.
  function groupByFile(matches) {
    const order = [];
    const byPath = new Map();
    for (const match of matches || []) {
      const key = match.relativePath;
      if (!byPath.has(key)) {
        const group = { relativePath: key, path: match.path, matches: [] };
        byPath.set(key, group);
        order.push(group);
      }
      byPath.get(key).matches.push(match);
    }
    return order;
  }

  function init(options) {
    const opts = options || {};
    const panelEl = opts.panelEl;
    if (!panelEl) throw new Error('Search panel requires panelEl');
    const invoke = opts.invoke;
    const listen = opts.listen;

    let caseSensitive = false;
    let activeSearchId = null;
    let state = 'idle';
    let capped = false;
    let matches = [];

    const root = el('div', 'tl-project-search');
    const toolbar = el('div', 'tl-project-search__toolbar');

    const input = el('input', 'tl-project-search__input');
    input.type = 'search';
    input.setAttribute('placeholder', 'Search in project');
    input.setAttribute('aria-label', 'Search in project');
    input.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter') return;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      runSearch();
    });

    const caseToggle = el('button', 'tl-project-search__toggle');
    caseToggle.type = 'button';
    caseToggle.textContent = 'Aa';
    caseToggle.title = 'Match case';
    caseToggle.setAttribute('data-search-case', 'toggle');
    caseToggle.setAttribute('aria-pressed', 'false');
    caseToggle.setAttribute('aria-label', 'Match case');
    caseToggle.addEventListener('click', () => {
      caseSensitive = !caseSensitive;
      caseToggle.setAttribute('aria-pressed', caseSensitive ? 'true' : 'false');
    });

    toolbar.appendChild(input);
    toolbar.appendChild(caseToggle);

    const statusHost = el('div', 'tl-project-search__status-host');
    const list = el('div', 'tl-project-search__list tl-scroll');
    list.setAttribute('role', 'tree');
    list.setAttribute('aria-label', 'Search results');

    root.appendChild(toolbar);
    root.appendChild(statusHost);
    root.appendChild(list);
    panelEl.appendChild(root);

    function statusText() {
      if (state === 'idle') return 'Search this project for text.';
      if (state === 'searching') return 'Searching…';
      if (!matches.length) return 'No results.';
      if (capped) {
        return 'Showing the first ' + MAX_MATCHES + ' matches — narrow the query for the rest.';
      }
      return matches.length + ' matches in ' + groupByFile(matches).length + ' files.';
    }

    function renderStatus() {
      const node = el('div', 'tl-project-search__status');
      node.setAttribute('role', 'status');
      node.setAttribute('data-state', state);
      node.textContent = statusText();
      statusHost.replaceChildren(node);
    }

    function makeRow(kind, level) {
      const row = el('div', 'tl-project-search__row');
      row.setAttribute('role', 'treeitem');
      row.setAttribute('data-search-row', kind);
      row.setAttribute('aria-level', String(level));
      row.setAttribute('tabindex', '-1');
      return row;
    }

    function renderList() {
      const built = [];
      for (const group of groupByFile(matches)) {
        const fileRow = makeRow('file', 1);
        fileRow.classList.add('tl-project-search__row--file');
        const where = el('span', 'tl-project-search__where');
        where.textContent = group.relativePath;
        const count = el('span', 'tl-project-search__count');
        count.textContent = String(group.matches.length);
        fileRow.appendChild(where);
        fileRow.appendChild(count);
        fileRow.title = group.path;
        built.push(fileRow);

        for (const match of group.matches) {
          const row = makeRow('match', 2);
          row.classList.add('tl-project-search__row--match');
          const line = el('span', 'tl-project-search__line');
          line.textContent = String(match.line);
          const preview = el('span', 'tl-project-search__preview');
          preview.textContent = match.preview;
          row.appendChild(line);
          row.appendChild(preview);
          row.title = match.path + ':' + match.line;
          row.setAttribute('aria-label', group.relativePath + ' line ' + match.line + ': ' + match.preview);
          row._match = match;
          built.push(row);
        }
      }
      list.replaceChildren(...built);
    }

    function render() {
      renderStatus();
      renderList();
    }

    function runSearch() {
      const query = String(input.value || '').trim();
      matches = [];
      capped = false;
      if (!query) {
        activeSearchId = null;
        state = 'idle';
        Promise.resolve(invoke('project_search_cancel')).catch(() => {});
        render();
        return;
      }
      state = 'searching';
      render();
      Promise.resolve(invoke('project_search', { query, caseSensitive }))
        .then((searchId) => { activeSearchId = searchId; })
        .catch((error) => {
          activeSearchId = null;
          state = 'idle';
          render();
          if (global.toast) global.toast.error('Search Failed', String(error));
        });
    }

    function onResults(event) {
      const payload = (event && event.payload) || null;
      if (!payload || payload.searchId !== activeSearchId) return;
      matches = matches.concat(payload.matches || []);
      if (payload.done) {
        state = 'done';
        capped = payload.capped === true;
      }
      render();
    }

    const unlisten = typeof listen === 'function' ? listen(RESULTS_EVENT, onResults) : null;

    function activate(row) {
      const match = row && row._match;
      if (!match) return;
      const service = global.termlabEditorService;
      if (!service || typeof service.openLocalFileAt !== 'function') return;
      // Search lines are 1-based; LSP ranges are 0-based.
      const zeroBased = Math.max(0, Number(match.line) - 1);
      const position = { line: zeroBased, character: 0 };
      Promise.resolve(
        service.openLocalFileAt(match.path, { start: position, end: { line: zeroBased, character: 0 } }, { focus: true }),
      ).catch((error) => {
        console.warn('project search: could not open the match', error);
      });
    }

    list.addEventListener('click', (event) => {
      const row = event.target && typeof event.target.closest === 'function'
        ? event.target.closest('[data-search-row]')
        : event.target;
      if (!row) return;
      activate(row);
    });

    list.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter') return;
      const row = event.target;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      activate(row);
    });

    render();

    return {
      element: root,
      focusInput() {
        if (typeof input.focus === 'function') input.focus();
      },
      destroy() {
        Promise.resolve(invoke('project_search_cancel')).catch(() => {});
        if (typeof unlisten === 'function') unlisten();
        else if (unlisten && typeof unlisten.then === 'function') unlisten.then((fn) => { if (typeof fn === 'function') fn(); }).catch(() => {});
      },
    };
  }

  global.projectSearchPanel = { init, groupByFile };
})(window);
```

- [ ] **Step 5: Register the tool window.** In `tool-window-runtime.js`'s `registerBuiltInToolWindows`, **after** the `problems` registration and before `notifications`:

```javascript
      // Project windows only, and registered after every existing bottom-zone
      // registrant for the reason Problems is: the first registrant activates
      // its zone on a layout that has never configured one, and Search must
      // never be the panel a user finds open. It starts inactive with a strip
      // button; cmd+shift+f activates it.
      if (projectRoot) {
        global.toolWindowManager.register('project-search', {
          title: 'Search',
          icon: null,
          type: 'built-in',
          defaultZone: 'bottom',
          renderFn: (container) => {
            const panelEl = document.createElement('div');
            panelEl.id = 'project-search-panel';
            container.appendChild(panelEl);
            if (global.projectSearchPanel) {
              return global.projectSearchPanel.init({
                panelEl,
                invoke,
                listen: listenOnCurrentWindow,
              });
            }
            return undefined;
          },
        });
      }
```

- [ ] **Step 6: Wire the shortcut, the action and the palette.** In `shortcut-runtime.js`'s `coreShortcutActionByKey`, after `editor_previous_problem`:

```javascript
      search_in_project: 'search-in-project',
```

In `menu-actions.js`, after the `open-folder` branch:

```javascript
      if (action === 'search-in-project') {
        // A no-op outside a project window: the tool window is only
        // registered when the window has a project, and toggle/activate on an
        // unregistered id is already a no-op in the manager.
        const twm = global.toolWindowManager;
        if (twm && typeof twm.activate === 'function') twm.activate('project-search');
        const panel = document.getElementById('project-search-panel');
        const field = panel ? panel.querySelector('.tl-project-search__input') : null;
        if (field && typeof field.focus === 'function') field.focus();
        return;
      }
```

In `command-palette-runtime.js`, after the `core:open-folder` line:

```javascript
      add('core:search-in-project', 'Search in Project', 'Project', 'search find text grep project files contents', () => handleMenuAction('search-in-project'), 'Project');
```

- [ ] **Step 7: Write the stylesheet and load everything.** Create `crates/termlab_tauri/frontend/styles/design-system/components/project-search.css`:

```css
/* Project search (app/panels/project-search-panel.js).

   The same density and structure as the Problems panel — a toolbar that is
   built once (re-creating the field on every keystroke would take the caret
   with it), a fixed status slot so the empty/searching/capped/no-results line
   can come and go without reordering the panel, and a scrolling list below. */

.tl-project-search {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  height: 100%;
  overflow: hidden;
  color: var(--tl-fg);
  background: var(--tl-panel-bg);
  font: 400 var(--tl-font-size-ui) var(--tl-font-ui);
}

.tl-project-search__toolbar {
  flex: 0 0 var(--tl-toolbar-h);
  display: flex;
  align-items: center;
  gap: var(--tl-space-1);
  min-width: 0;
  padding: 0 var(--tl-space-2);
  border-bottom: 1px solid var(--tl-border);
}

.tl-project-search__input {
  flex: 1;
  min-width: 0;
  height: var(--tl-row-h);
  padding: 0 var(--tl-space-2);
  color: var(--tl-fg);
  background: var(--tl-control-bg);
  border: 1px solid var(--tl-control-border);
  border-radius: var(--tl-radius);
}

.tl-project-search__input:focus-visible {
  outline: 2px solid var(--tl-accent);
  outline-offset: 1px;
}

.tl-project-search__toggle {
  height: var(--tl-row-h);
  padding: 0 var(--tl-space-2);
  color: var(--tl-fg);
  background: var(--tl-control-bg);
  border: 1px solid var(--tl-control-border);
  border-radius: var(--tl-radius);
  cursor: pointer;
}

.tl-project-search__toggle[aria-pressed="true"] {
  border-color: var(--tl-accent);
  background: var(--tl-row-hover);
}

.tl-project-search__toggle:focus-visible {
  outline: 2px solid var(--tl-accent);
  outline-offset: 1px;
}

.tl-project-search__status-host {
  flex: 0 0 auto;
}

.tl-project-search__status {
  padding: var(--tl-space-1) var(--tl-space-2);
  color: var(--tl-fg-muted);
}

.tl-project-search__list {
  flex: 1;
  min-height: 0;
  overflow: auto;
}

.tl-project-search__row {
  display: flex;
  align-items: center;
  gap: var(--tl-space-2);
  height: var(--tl-row-h);
  padding: 0 var(--tl-space-2);
  white-space: nowrap;
  cursor: default;
}

.tl-project-search__row:hover {
  background: var(--tl-row-hover);
}

.tl-project-search__row:focus-visible {
  outline: 2px solid var(--tl-accent);
  outline-offset: -2px;
}

.tl-project-search__row--match {
  padding-left: var(--tl-space-4);
}

.tl-project-search__where {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tl-project-search__count {
  color: var(--tl-fg-muted);
}

.tl-project-search__line {
  min-width: 3ch;
  text-align: right;
  color: var(--tl-fg-muted);
  font-family: var(--tl-font-mono);
}

.tl-project-search__preview {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  font-family: var(--tl-font-mono);
}
```

In `index.html`, add the stylesheet after `project-tree.css` and the script immediately after `app/panels/project-tree.js`:

```html
  <link rel="stylesheet" href="styles/design-system/components/project-search.css" />
```

```html
  <script src="app/panels/project-search-panel.js"></script>
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `node scripts/tests/test_project_search_panel.mjs` — all checks pass.
Run: `cargo test -p termlab_core --quiet` — PASS.
Run: `node scripts/tests/test_problems_panel.mjs` — PASS (the registration-order rule is unchanged for non-project windows).
Run the full sweep — prints nothing.
Run: `bash scripts/check_frontend_boundaries.sh` — only `tl-dialog.js:334`.

- [ ] **Step 9: Commit**

```bash
git add crates/termlab_tauri/frontend/app/panels/project-search-panel.js \
        crates/termlab_tauri/frontend/styles/design-system/components/project-search.css \
        crates/termlab_tauri/frontend/app/tool-window-runtime.js \
        crates/termlab_tauri/frontend/app/shortcut-runtime.js \
        crates/termlab_tauri/frontend/app/menu-actions.js \
        crates/termlab_tauri/frontend/app/command-palette-runtime.js \
        crates/termlab_tauri/frontend/index.html \
        crates/termlab_core/src/config/termlab.rs \
        crates/termlab_core/src/config/mod.rs \
        scripts/tests/test_project_search_panel.mjs
git commit -m "feat: add the project Search tool window with a cmd+shift+f default"
```

---

### Task 10: Rust — git porcelain v2 parsing

**Files:**
- Create: `crates/termlab_tauri/src/project/git_status.rs`
- Modify: `crates/termlab_tauri/src/project/mod.rs` (add `pub(crate) mod git_status;`)
- Modify: `crates/termlab_tauri/src/lib.rs` (`generate_handler!`)
- Test: `#[cfg(test)] mod tests` at the bottom of `crates/termlab_tauri/src/project/git_status.rs`

**Interfaces:**
- Consumes: `ProjectRegistry::root_for(label)` (Task 1).
- Produces:
  - `pub(crate) enum GitFileState { Modified, Added, Untracked, Deleted, Renamed, Conflicted }` (serde `rename_all = "lowercase"`)
  - `pub(crate) struct GitStatusSnapshot { pub available: bool, pub files: BTreeMap<String, GitFileState> }` (serde camelCase; keys are paths relative to the repo root, `/`-separated as git emits them)
  - `pub(crate) fn parse_porcelain_v2(bytes: &[u8]) -> BTreeMap<String, GitFileState>`
  - `pub(crate) fn unavailable() -> GitStatusSnapshot`
  - `pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(5);`
  - command `project_git_status(window) -> GitStatusSnapshot`

- [ ] **Step 1: Write the failing tests.** Create `crates/termlab_tauri/src/project/git_status.rs` with only this test module:

```rust
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
        assert_eq!(files.get("old.rs"), None, "the original path is a field, not a record");
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
        assert_eq!(files.get("target/debug/thing"), None, "ignored files carry no tint");
        assert_eq!(files.len(), 1, "headers contribute nothing");
    }

    #[test]
    fn unmerged_entries_are_conflicted() {
        let bytes = fixture(&[
            "u UU N... 100644 100644 100644 100644 aaaa bbbb cccc both.rs",
        ]);
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
        assert!(parse_porcelain_v2(&fixture(&["?"])).is_empty(), "a bare marker names no path");
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_tauri --quiet project::git_status`
Expected: compile FAIL — `file not found for module git_status`, then `cannot find function parse_porcelain_v2`.

- [ ] **Step 3: Write the implementation.** Prepend to `crates/termlab_tauri/src/project/git_status.rs`:

```rust
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
```

In `project/mod.rs`, beside the `search` declaration:

```rust
pub(crate) mod git_status;
```

In `lib.rs`'s `generate_handler!`:

```rust
                project::git_status::project_git_status,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p termlab_tauri --quiet project::git_status` — PASS (9 tests).
Run: `cargo test --workspace --quiet` — PASS.
Run: `cargo clippy --all-targets` — no new warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/src/project/git_status.rs \
        crates/termlab_tauri/src/project/mod.rs \
        crates/termlab_tauri/src/lib.rs
git commit -m "feat: parse git porcelain v2 status for the project tree"
```

---

### Task 11: Git tints in the tree

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/project/git-tints.js`
- Modify: `crates/termlab_tauri/frontend/styles/design-system/base.css` (semantic aliases)
- Modify: `crates/termlab_tauri/frontend/styles/design-system/components/project-tree.css`
- Modify: `crates/termlab_tauri/frontend/app/panels/files-panel.js` (git refresh wiring in `renderProjectTree`)
- Modify: `crates/termlab_tauri/frontend/index.html`
- Test: `scripts/tests/test_project_git_tints.mjs` (new)

**Interfaces:**
- Consumes: `project_git_status() -> { available, files }` (Task 10); `window.filesPanel.projectTree()` and the tree handle's `setGitStatus(snapshot)` (Tasks 5-6); `window.termlabEditorService` save events.
- Produces: `window.termlabProjectGit` with
  - `stateForPath(snapshot, root, absolutePath, isDir) -> string|null` — the file's own state, or for a directory the rolled-up `'modified'` when anything beneath it has one
  - `relativeTo(root, absolutePath) -> string|null`
  - `startPolling({ invoke, getTree, intervalMs, isVisible, target }) -> stop()` — refresh on window focus, after an editor save in this window, and on a 10-second timer gated by `isVisible()` (defaults to always visible; `target` defaults to `window` and exists so the suite can drive the listeners)

- [ ] **Step 1: Write the failing test.** Create `scripts/tests/test_project_git_tints.mjs` (same header, `check`/`deepEq`/`plain` helpers and runner tail as `test_project_mode.mjs`):

```javascript
const GIT = path.join(APP, 'features/project/git-tints.js');
const TREE_CSS = path.join(ROOT, 'styles/design-system/components/project-tree.css');
const BASE_CSS = path.join(ROOT, 'styles/design-system/base.css');

function loadGit() {
  const sandbox = {
    console, JSON, Object, Array, String, Number, Math, Map, Set, Promise,
    setTimeout, clearTimeout, setInterval, clearInterval,
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(GIT, 'utf8'), sandbox, { filename: GIT });
  return sandbox.termlabProjectGit;
}

const snapshot = (files) => ({ available: true, files: files || {} });

check('relativeTo answers on path boundaries', () => {
  const git = loadGit();
  assert.strictEqual(git.relativeTo('/repo', '/repo/src/main.rs'), 'src/main.rs');
  assert.strictEqual(git.relativeTo('/repo', '/repo'), '');
  assert.strictEqual(git.relativeTo('/repo', '/repository/a.rs'), null,
    'a sibling sharing a prefix is not inside the repo');
  assert.strictEqual(git.relativeTo('/repo', '/elsewhere/a.rs'), null);
});

check('a file carries its own state and nothing else', () => {
  const git = loadGit();
  const snap = snapshot({ 'src/main.rs': 'modified', 'new.rs': 'added' });
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/src/main.rs', false), 'modified');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/new.rs', false), 'added');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/clean.rs', false), null);
});

check('a directory rolls up to modified when anything beneath it has a state', () => {
  const git = loadGit();
  const snap = snapshot({ 'src/deep/a.rs': 'added', 'other.rs': 'untracked' });
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/src', true), 'modified');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/src/deep', true), 'modified');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/tests', true), null,
    'a clean directory carries no tint');
});

check('rollup matches on path segments, not string prefixes', () => {
  const git = loadGit();
  const snap = snapshot({ 'srcfoo/a.rs': 'modified' });
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/src', true), null,
    '"srcfoo" is not inside "src"');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/srcfoo', true), 'modified');
});

check('an unavailable snapshot tints nothing', () => {
  const git = loadGit();
  const off = { available: false, files: {} };
  assert.strictEqual(git.stateForPath(off, '/repo', '/repo/src/main.rs', false), null);
  assert.strictEqual(git.stateForPath(null, '/repo', '/repo/src/main.rs', false), null);
});

check('polling refreshes on focus, on save and on the timer, and stops cleanly', async () => {
  const git = loadGit();
  const applied = [];
  const listeners = new Map();
  const fakeWindow = {
    addEventListener: (type, fn) => { listeners.set(type, fn); },
    removeEventListener: (type) => { listeners.delete(type); },
  };
  let calls = 0;
  const stop = git.startPolling({
    invoke: async () => { calls += 1; return snapshot({ 'a.rs': 'modified' }); },
    getTree: () => ({ setGitStatus: (s) => applied.push(s) }),
    intervalMs: 10,
    target: fakeWindow,
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.strictEqual(calls, 1, 'an immediate first refresh');
  listeners.get('focus')();
  listeners.get('termlab:editor-saved')();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.strictEqual(calls, 3, 'focus and save each refresh');
  await new Promise((resolve) => setTimeout(resolve, 25));
  assert.ok(calls > 3, 'the timer keeps it fresh while the panel is visible');
  const before = calls;
  stop();
  await new Promise((resolve) => setTimeout(resolve, 25));
  assert.strictEqual(calls, before, 'stopping really stops');
  assert.ok(applied.length > 0, 'each snapshot reaches the tree');
});

check('the timer is gated on visibility, but focus and save never are', async () => {
  const git = loadGit();
  const listeners = new Map();
  const fakeWindow = {
    addEventListener: (type, fn) => { listeners.set(type, fn); },
    removeEventListener: (type) => { listeners.delete(type); },
  };
  let calls = 0;
  const stop = git.startPolling({
    invoke: async () => { calls += 1; return snapshot({}); },
    getTree: () => ({ setGitStatus: () => {} }),
    intervalMs: 10,
    isVisible: () => false,
    target: fakeWindow,
  });
  await new Promise((resolve) => setTimeout(resolve, 35));
  assert.strictEqual(calls, 1, 'only the immediate first refresh — the timer is gated');
  listeners.get('focus')();
  listeners.get('termlab:editor-saved')();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.strictEqual(calls, 3, 'a user act always refreshes, hidden or not');
  stop();
});

check('the tints are tokens, defined for both themes, and shape-paired', () => {
  const base = fs.readFileSync(BASE_CSS, 'utf8');
  for (const token of [
    '--tl-git-modified', '--tl-git-added', '--tl-git-deleted',
    '--tl-git-conflicted', '--tl-git-untracked',
  ]) {
    assert.ok(base.includes(token + ':'), `${token} has no semantic alias`);
  }
  const tree = fs.readFileSync(TREE_CSS, 'utf8');
  for (const state of ['modified', 'added', 'deleted', 'renamed', 'conflicted', 'untracked']) {
    assert.ok(
      tree.includes(`[data-git-state="${state}"]`),
      `${state} has no rule — its tint would silently be the default colour`,
    );
  }
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(tree), 'design-system components use tokens only');
});

check('git-tints uses no regex lookbehind and no control bytes', () => {
  const source = fs.readFileSync(GIT, 'utf8');
  assert.ok(!/\(\?<[=!]/.test(source), `${GIT} uses a lookbehind`);
  const bytes = fs.readFileSync(GIT);
  for (let i = 0; i < bytes.length; i += 1) {
    assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
      `${GIT}: control byte at offset ${i}`);
  }
});

check('index.html loads git-tints before the files panel that starts it', () => {
  const html = fs.readFileSync(path.join(ROOT, 'index.html'), 'utf8');
  assert.ok(html.indexOf('app/features/project/git-tints.js') > 0);
  assert.ok(html.indexOf('app/features/project/git-tints.js') < html.indexOf('app/panels/project-tree.js'));
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `node scripts/tests/test_project_git_tints.mjs`
Expected: FAIL — `Cannot find module .../features/project/git-tints.js`.

- [ ] **Step 3: Write the module.** Create `crates/termlab_tauri/frontend/app/features/project/git-tints.js`:

```javascript
// Git status for the project tree: the flat snapshot Rust produces, plus the
// directory rollup the tree needs.
//
// The rollup is computed here rather than in Rust because it is a pure
// function of a snapshot the tree already has, and because "which directories
// are currently on screen" is a frontend question. It matches on path
// SEGMENTS, never string prefixes: "srcfoo/a.rs" is not inside "src", and a
// prefix test would tint half the tree the first time someone names two
// directories that way.
//
// Snapshots are replace-only — never merged — so a file that stopped being
// modified simply stops appearing, with no stale tint left behind.
(function initTermLabProjectGit(global) {
  'use strict';

  const DEFAULT_INTERVAL_MS = 10000;

  // The repo-relative path, or null when the file is not inside the root.
  function relativeTo(root, absolutePath) {
    if (!root || !absolutePath) return null;
    const rootStr = String(root);
    const target = String(absolutePath);
    if (target === rootStr) return '';
    const prefix = rootStr.endsWith('/') ? rootStr : rootStr + '/';
    if (!target.startsWith(prefix)) return null;
    return target.slice(prefix.length);
  }

  function stateForPath(snapshot, root, absolutePath, isDir) {
    if (!snapshot || snapshot.available !== true || !snapshot.files) return null;
    const relative = relativeTo(root, absolutePath);
    if (relative === null) return null;
    const files = snapshot.files;
    if (!isDir) {
      return Object.prototype.hasOwnProperty.call(files, relative) ? files[relative] : null;
    }
    // A folder shows the modified tint when anything beneath it has a state —
    // one tint, not six, because a folder has no single state of its own.
    const prefix = relative === '' ? '' : relative + '/';
    for (const key of Object.keys(files)) {
      if (prefix === '' || key.startsWith(prefix)) return 'modified';
    }
    return null;
  }

  // Refresh triggers: window focus, an editor save in this window, and a
  // timer while the panel is visible in project mode. There is no filesystem
  // watcher in v1, so these three are the whole freshness story.
  //
  // `isVisible` gates the TIMER only. Focus and save are user acts and always
  // refresh; a ticking clock against a hidden panel is pure waste — a git
  // process every ten seconds for a status nobody is looking at.
  function startPolling(options) {
    const opts = options || {};
    const invoke = opts.invoke;
    const getTree = opts.getTree;
    const target = opts.target || global;
    const isVisible = typeof opts.isVisible === 'function' ? opts.isVisible : () => true;
    const intervalMs = Number(opts.intervalMs) > 0 ? Number(opts.intervalMs) : DEFAULT_INTERVAL_MS;
    let stopped = false;

    function refresh() {
      if (stopped) return;
      Promise.resolve(invoke('project_git_status'))
        .then((snapshot) => {
          if (stopped) return;
          const tree = typeof getTree === 'function' ? getTree() : null;
          if (tree && typeof tree.setGitStatus === 'function') tree.setGitStatus(snapshot);
        })
        // Silently off: git being absent, or the project not being a
        // repository, is completely ordinary and must never toast.
        .catch(() => {});
    }

    const onFocus = () => refresh();
    const onSaved = () => refresh();
    target.addEventListener('focus', onFocus);
    target.addEventListener('termlab:editor-saved', onSaved);
    const timer = setInterval(() => {
      if (isVisible()) refresh();
    }, intervalMs);
    refresh();

    return function stop() {
      stopped = true;
      clearInterval(timer);
      target.removeEventListener('focus', onFocus);
      target.removeEventListener('termlab:editor-saved', onSaved);
    };
  }

  global.termlabProjectGit = { relativeTo, stateForPath, startPolling };
})(window);
```

- [ ] **Step 4: Emit the save signal.** In `crates/termlab_tauri/frontend/app/features/editor/editor-service.js`, at the end of the successful local-save path inside `savePane` (immediately after the write resolves and the pane is marked clean), add:

```javascript
      // A save is the one project-tree refresh trigger the app knows about
      // precisely; git-tints.js listens for it.
      if (typeof global.dispatchEvent === 'function' && typeof global.CustomEvent === 'function') {
        global.dispatchEvent(new global.CustomEvent('termlab:editor-saved', {
          detail: { path: pane.filePath || null },
        }));
      }
```

- [ ] **Step 5: Start it from the panel.** In `files-panel.js`'s `renderProjectTree`, after the trust banner mount:

```javascript
    if (window.termlabProjectGit && typeof window.termlabProjectGit.startPolling === 'function') {
      if (typeof stopGitPolling === 'function') stopGitPolling();
      stopGitPolling = window.termlabProjectGit.startPolling({
        invoke,
        getTree: () => projectTreeHandle,
        // The timer only ticks while the Files panel is actually on screen.
        isVisible: () => !!(window.toolWindowManager
          && typeof window.toolWindowManager.isVisible === 'function'
          && window.toolWindowManager.isVisible('file-explorer')),
      });
    }
```

…declare `let stopGitPolling = null;` beside the other project-mode state, and stop it at the top of `renderPanelBody` where the tree is destroyed:

```javascript
    if (typeof stopGitPolling === 'function') {
      stopGitPolling();
      stopGitPolling = null;
    }
```

- [ ] **Step 6: Add the tokens and the rules.** In `styles/design-system/base.css`, after the `--tl-warning-*` block:

```css
  /* Git status tints for the project tree. Every raw token below was checked
     present in BOTH tokens-dark.css and tokens-light.css, so none of these
     silently falls through to an inherited colour in one appearance:
     Link-activeForeground (#6494ed / #3457C7), Label-successForeground
     (#89ca78 / #2F8F5B) and Component-warningFocusColor (#8c812b / #8A6D1F).
     Deleted reuses --tl-danger and untracked --tl-fg-muted, both already
     semantic aliases defined above. Colour is never the only signal: every
     tinted row also carries its state in its aria-label. */
  --tl-git-modified: var(--tl-Link-activeForeground);
  --tl-git-added: var(--tl-Label-successForeground);
  --tl-git-deleted: var(--tl-danger);
  --tl-git-conflicted: var(--tl-Component-warningFocusColor);
  --tl-git-untracked: var(--tl-fg-muted);
```

In `styles/design-system/components/project-tree.css`:

```css
/* Git tints. A renamed path reads as modified — the tree says "this changed",
   and which kind of change belongs in a diff view. */
.tl-project-tree__row[data-git-state="modified"] .tl-project-tree__label,
.tl-project-tree__row[data-git-state="renamed"] .tl-project-tree__label {
  color: var(--tl-git-modified);
}

.tl-project-tree__row[data-git-state="added"] .tl-project-tree__label {
  color: var(--tl-git-added);
}

.tl-project-tree__row[data-git-state="deleted"] .tl-project-tree__label {
  color: var(--tl-git-deleted);
  text-decoration: line-through;
}

.tl-project-tree__row[data-git-state="conflicted"] .tl-project-tree__label {
  color: var(--tl-git-conflicted);
  font-style: italic;
}

.tl-project-tree__row[data-git-state="untracked"] .tl-project-tree__label {
  color: var(--tl-git-untracked);
}
```

In `index.html`, add before `app/panels/project-tree.js`:

```html
  <script src="app/features/project/git-tints.js"></script>
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `node scripts/tests/test_project_git_tints.mjs` — pass.
Run: `node scripts/tests/test_project_tree.mjs && node scripts/tests/test_project_mode.mjs` — pass.
Run the full sweep — prints nothing (the editor-save suites must stay green: the dispatch is additive).
Run: `bash scripts/check_frontend_boundaries.sh` — only `tl-dialog.js:334`.

- [ ] **Step 8: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/project/git-tints.js \
        crates/termlab_tauri/frontend/app/features/editor/editor-service.js \
        crates/termlab_tauri/frontend/app/panels/files-panel.js \
        crates/termlab_tauri/frontend/styles/design-system/base.css \
        crates/termlab_tauri/frontend/styles/design-system/components/project-tree.css \
        crates/termlab_tauri/frontend/index.html \
        scripts/tests/test_project_git_tints.mjs
git commit -m "feat: tint project tree rows by git status with directory rollup"
```

---

### Task 12: Recent projects and per-project layouts

**Files:**
- Create: `crates/termlab_tauri/src/project/recents.rs`
- Modify: `crates/termlab_core/src/config/persistent.rs` (`PersistentState:8`, `Default:24`)
- Modify: `crates/termlab_tauri/src/project/mod.rs` (`pub(crate) mod recents;`, record on open/adopt)
- Modify: `crates/termlab_tauri/src/commands.rs` (`get_saved_layout:335`, `save_window_layout:391`)
- Modify: `crates/termlab_tauri/src/menu.rs` (Open Recent Project submenu in both builders)
- Modify: `crates/termlab_tauri/src/lib.rs` (`on_menu_event`, `generate_handler!`)
- Modify: `crates/termlab_tauri/frontend/app/command-palette-runtime.js`
- Test: `#[cfg(test)] mod tests` in `project/recents.rs` and `persistent.rs`; `scripts/tests/test_project_recents.mjs` (new)

**Interfaces:**
- Consumes: `ProjectRegistry::root_for(label)`, `project_name`, `now_ms`, `canonical_root` (Task 1); `config::update_persistent_state`, `config::load_persistent_state`, `saved_layout_from_state`, `merge_window_layout`.
- Produces:
  - `pub struct RecentProject { pub path: String, pub last_opened_ms: u64 }` in `termlab_core::config` (`#[serde(default)]`)
  - `PersistentState::recent_projects: Vec<RecentProject>` and `PersistentState::project_layouts: HashMap<String, LayoutConfig>`
  - `pub(crate) const MAX_RECENTS: usize = 10;`
  - `pub(crate) fn record_recent(list: &mut Vec<RecentProject>, path: &str, now_ms: u64, exists: impl Fn(&str) -> bool)` — moves `path` to the front, prunes entries whose path no longer exists, caps at `MAX_RECENTS`
  - `pub(crate) struct RecentProjectInfo { pub path: String, pub name: String, pub last_opened_ms: u64 }` (serde camelCase)
  - command `project_recents() -> Vec<RecentProjectInfo>` — skips paths that no longer exist
  - Rust menu id prefix `MENU_RECENT_PROJECT_PREFIX = "file.recent_project."` and action prefix `MENU_ACTION_OPEN_RECENT_PROJECT = "open-recent-project:"`
  - `get_saved_layout(window)` returns the project's layout when the calling window has one; `save_window_layout` writes into that entry instead of the global one

- [ ] **Step 1: Write the failing tests.** Create `crates/termlab_tauri/src/project/recents.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn all_exist(_: &str) -> bool {
        true
    }

    #[test]
    fn recording_moves_a_project_to_the_front_without_duplicating_it() {
        let mut list = Vec::new();
        record_recent(&mut list, "/a", 1, all_exist);
        record_recent(&mut list, "/b", 2, all_exist);
        record_recent(&mut list, "/a", 3, all_exist);
        assert_eq!(
            list.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            vec!["/a", "/b"],
            "most recent first, one entry per project"
        );
        assert_eq!(list[0].last_opened_ms, 3, "the timestamp is refreshed");
    }

    #[test]
    fn the_list_is_capped_at_ten_and_drops_the_oldest() {
        let mut list = Vec::new();
        for i in 0..(MAX_RECENTS + 5) {
            record_recent(&mut list, &format!("/p{i}"), i as u64, all_exist);
        }
        assert_eq!(list.len(), MAX_RECENTS);
        assert_eq!(list[0].path, format!("/p{}", MAX_RECENTS + 4));
        assert!(
            !list.iter().any(|r| r.path == "/p0"),
            "the oldest entries fall off the end"
        );
    }

    #[test]
    fn a_project_whose_path_is_gone_is_pruned_on_the_next_update() {
        let mut list = Vec::new();
        record_recent(&mut list, "/gone", 1, all_exist);
        record_recent(&mut list, "/kept", 2, all_exist);
        record_recent(&mut list, "/new", 3, |p| p != "/gone");
        assert_eq!(
            list.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            vec!["/new", "/kept"],
            "the vanished project is pruned, the rest keeps its order"
        );
    }

    #[test]
    fn the_project_being_recorded_is_never_pruned_by_its_own_update() {
        // The predicate is asked about every OTHER entry; the path being
        // recorded was just opened, so it exists by construction.
        let mut list = Vec::new();
        record_recent(&mut list, "/fresh", 1, |_| false);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].path, "/fresh");
    }

    #[test]
    fn recent_project_info_serializes_as_camel_case() {
        let json = serde_json::to_string(&RecentProjectInfo {
            path: "/repo".into(),
            name: "repo".into(),
            last_opened_ms: 7,
        })
        .expect("serialize");
        assert!(json.contains("\"lastOpenedMs\":7"), "got {json}");
    }
}
```

…and append to `crates/termlab_core/src/config/persistent.rs`'s test module:

```rust
    #[test]
    fn recent_projects_and_project_layouts_round_trip() {
        let mut layouts = HashMap::new();
        layouts.insert(
            "/repo".to_string(),
            LayoutConfig {
                zen_mode: false,
                bottom_panel_height: 220.0,
                ..LayoutConfig::default()
            },
        );
        let original = PersistentState {
            recent_projects: vec![RecentProject {
                path: "/repo".into(),
                last_opened_ms: 42,
            }],
            project_layouts: layouts,
            ..PersistentState::default()
        };
        let toml_str = toml::to_string(&original).expect("serialize");
        let restored: PersistentState = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(restored.recent_projects.len(), 1);
        assert_eq!(restored.recent_projects[0].path, "/repo");
        assert_eq!(restored.recent_projects[0].last_opened_ms, 42);
        assert_eq!(
            restored
                .project_layouts
                .get("/repo")
                .map(|l| l.bottom_panel_height),
            Some(220.0)
        );
    }

    #[test]
    fn the_two_project_fields_default_when_absent() {
        // Back-compat: every state.toml written before project mode existed
        // has neither key.
        let toml_str = r#"
loaded_plugins = ["my-plugin"]

[layout]
zoom_factor = 1.5
"#;
        let ps: PersistentState = toml::from_str(toml_str).expect("deserialize");
        assert!(ps.recent_projects.is_empty());
        assert!(ps.project_layouts.is_empty());
        assert_eq!(ps.layout.zoom_factor, 1.5, "the existing keys still load");
    }
```

Create `scripts/tests/test_project_recents.mjs` (same header/helpers/runner tail as the other suites):

```javascript
check('the palette offers a Reopen Project entry per recent', () => {
  const src = fs.readFileSync(path.join(APP, 'command-palette-runtime.js'), 'utf8');
  assert.ok(src.includes("invoke('project_recents')"), 'the palette reads the recents list');
  assert.ok(src.includes('Reopen Project: '), 'each recent is its own entry');
  assert.ok(src.includes("invoke('project_open'"), 'choosing one opens it');
});

check('the File menu carries Open Recent Project in both builders', () => {
  const menuRs = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/menu.rs'), 'utf8');
  assert.ok(menuRs.includes('MENU_RECENT_PROJECT_PREFIX'), 'recent items have an id prefix');
  assert.ok(menuRs.includes('"Open Recent Project"'), 'the submenu is titled');
  const occurrences = menuRs.split('MENU_RECENT_PROJECT_PREFIX').length - 1;
  assert.ok(occurrences >= 3, `both builders must carry it, saw ${occurrences} references`);
});

check('menu-actions opens a recent by path', () => {
  const src = fs.readFileSync(path.join(APP, 'menu-actions.js'), 'utf8');
  assert.ok(src.includes("open-recent-project:"), 'the prefixed action is handled');
});

check('the layout commands are project-aware', () => {
  const src = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/commands.rs'), 'utf8');
  assert.ok(src.includes('project_layouts'), 'save and restore go through the per-project map');
  assert.ok(src.includes('ProjectRegistry'), 'the calling window resolves its project');
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_core --quiet recent_projects` — FAIL (`no field recent_projects`).
Run: `cargo test -p termlab_tauri --quiet project::recents` — FAIL (module not found).
Run: `node scripts/tests/test_project_recents.mjs` — FAIL.

- [ ] **Step 3: Add the persisted fields.** In `crates/termlab_core/src/config/persistent.rs`, add to `PersistentState`:

```rust
    /// Most-recently-opened projects, most recent first, capped at ten. An
    /// entry whose path no longer exists is skipped in the menu and pruned on
    /// the next update — a project the user deleted should not sit in a menu
    /// forever, but neither should opening an unrelated project delete the
    /// record of one that is merely on an unmounted volume today.
    pub recent_projects: Vec<RecentProject>,
    /// Project path → the layout snapshot that project was last left in. The
    /// value is the SAME `LayoutConfig` the per-window layout persistence
    /// uses, so a project window saves and restores exactly what an ordinary
    /// window does; an absent entry means the default project-window shape.
    pub project_layouts: HashMap<String, LayoutConfig>,
```

…and to its `Default` impl:

```rust
            recent_projects: Vec::new(),
            project_layouts: HashMap::new(),
```

…plus the new type above `ChooserWindowSize`:

```rust
/// One entry in the recent-projects list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RecentProject {
    pub path: String,
    pub last_opened_ms: u64,
}
```

Re-export it wherever `LayoutConfig` is re-exported from `termlab_core::config` (the same `pub use` line in `crates/termlab_core/src/config/mod.rs`).

- [ ] **Step 4: Write the recents helpers.** Prepend to `crates/termlab_tauri/src/project/recents.rs`:

```rust
//! The recent-projects list and its persistence.
//!
//! Split out of `project/mod.rs` rather than appended to it: this is
//! persistence logic with its own rules (ordering, capping, pruning) and it
//! wants its own tests. The ordering rule is the whole reason it is a pure
//! function — a most-recent-first list that also prunes is exactly the kind of
//! thing that quietly loses an entry.

use serde::Serialize;
use termlab_core::config::{self, RecentProject};

/// The menu would be unusable longer than this, and a longer memory is what
/// "open recent" is not for.
pub(crate) const MAX_RECENTS: usize = 10;

/// Move `path` to the front, refresh its timestamp, prune every OTHER entry
/// whose path no longer exists, and cap the result.
///
/// `exists` is injected so the rule is testable without a filesystem. The
/// path being recorded is never asked about: it was just opened, so it exists
/// by construction, and asking would let a slow or flaky stat delete the very
/// entry this call is adding.
pub(crate) fn record_recent(
    list: &mut Vec<RecentProject>,
    path: &str,
    now_ms: u64,
    exists: impl Fn(&str) -> bool,
) {
    list.retain(|entry| entry.path != path && exists(&entry.path));
    list.insert(
        0,
        RecentProject {
            path: path.to_string(),
            last_opened_ms: now_ms,
        },
    );
    list.truncate(MAX_RECENTS);
}

/// Record `root` as the most recently opened project. Failures to write
/// state.toml are logged, never surfaced: a missing recents entry must not
/// stop a project from opening.
pub(crate) fn remember(root: &str, now_ms: u64) {
    let root = root.to_string();
    if let Err(e) = config::update_persistent_state(|state| {
        record_recent(&mut state.recent_projects, &root, now_ms, |p| {
            std::path::Path::new(p).is_dir()
        });
        true
    }) {
        log::warn!("project: could not record {root} in the recent projects: {e}");
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecentProjectInfo {
    pub path: String,
    pub name: String,
    pub last_opened_ms: u64,
}

/// The recents that still exist on disk, most recent first. Reading does not
/// prune — pruning happens on the next update, so a project on a volume that
/// happens to be unmounted right now comes back rather than being forgotten.
pub(crate) fn list_recents() -> Vec<RecentProjectInfo> {
    let state = config::load_persistent_state().unwrap_or_default();
    state
        .recent_projects
        .iter()
        .filter(|entry| std::path::Path::new(&entry.path).is_dir())
        .map(|entry| RecentProjectInfo {
            path: entry.path.clone(),
            name: super::project_name(std::path::Path::new(&entry.path)),
            last_opened_ms: entry.last_opened_ms,
        })
        .collect()
}

#[tauri::command]
pub(crate) fn project_recents() -> Vec<RecentProjectInfo> {
    list_recents()
}
```

In `project/mod.rs`, declare the module beside the others:

```rust
pub(crate) mod recents;
```

…and record on both open paths. In `project_open`, immediately after the successful build (before the `Ok(ProjectOpenResult { … })`) **and** in the `focused_existing` early return, add:

```rust
    recents::remember(&root.display().to_string(), now_ms());
```

In `project_adopt_pending`, immediately after `registry.lock().bind(label, root.clone(), now_ms());`:

```rust
    recents::remember(&root.display().to_string(), now_ms());
```

Register the command in `lib.rs`'s `generate_handler!`:

```rust
                project::recents::project_recents,
```

- [ ] **Step 5: Make the layout commands project-aware.** In `crates/termlab_tauri/src/commands.rs`:

```rust
/// The layout this window should boot with. A project window gets its
/// PROJECT's saved layout; every other window gets the shared one. An absent
/// project entry falls back to the shared layout, which the frontend then
/// adjusts into the default project-window shape.
#[tauri::command]
pub(crate) fn get_saved_layout(
    window: tauri::WebviewWindow,
    projects: tauri::State<'_, parking_lot::Mutex<crate::project::ProjectRegistry>>,
) -> SavedLayout {
    let state = config::load_persistent_state().unwrap_or_default();
    let root = projects.lock().root_for(window.label());
    if let Some(root) = root
        && let Some(layout) = state.project_layouts.get(&root)
    {
        return saved_layout_from_state(layout);
    }
    saved_layout_from_state(&state.layout)
}
```

…and in `save_window_layout`, replace the `update_persistent_state` closure body:

```rust
    let project_root = {
        let projects = window
            .app_handle()
            .state::<parking_lot::Mutex<crate::project::ProjectRegistry>>();
        let root = projects.lock().root_for(window.label());
        root
    };

    let _ = config::update_persistent_state(|state| {
        // Recorded for diagnostics only (see the note above): nothing reads
        // these back for sizing.
        state.layout.window_width = logical_w as f32;
        state.layout.window_height = logical_h as f32;
        // A project window writes ONLY its project's entry. Otherwise a
        // project's panel arrangement would become the shape every ordinary
        // window opens in.
        let base = state.layout.clone();
        match project_root.as_ref() {
            Some(root) => {
                let entry = state.project_layouts.entry(root.clone()).or_insert(base);
                merge_window_layout(entry, layout);
            }
            None => merge_window_layout(&mut state.layout, layout),
        }
        true
    });
```

Add `use tauri::Manager;` to `commands.rs` if it is not already imported.

- [ ] **Step 6: Add the menu submenu and the palette entries.** In `menu.rs`, beside the other constants:

```rust
/// Menu ids for the Open Recent Project submenu are minted per entry as
/// `file.recent_project.<index>`; the index is resolved back to a path
/// through `recents::list_recents()` at click time, so a path is never
/// smuggled through a menu id.
pub(crate) const MENU_RECENT_PROJECT_PREFIX: &str = "file.recent_project.";
pub(crate) const MENU_ACTION_OPEN_RECENT_PROJECT: &str = "open-recent-project:";
```

Add a helper used by both builders:

```rust
/// The Open Recent Project submenu, or `None` when there is nothing to list.
/// A recent whose path no longer exists is already filtered out by
/// `list_recents`, so a dead entry never appears.
fn recent_projects_submenu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> tauri::Result<Option<Submenu<R>>> {
    let recents = crate::project::recents::list_recents();
    if recents.is_empty() {
        return Ok(None);
    }
    let mut items: Vec<MenuItem<R>> = Vec::new();
    for (index, entry) in recents.iter().enumerate() {
        items.push(MenuItem::with_id(
            app,
            format!("{MENU_RECENT_PROJECT_PREFIX}{index}"),
            &entry.name,
            true,
            None::<&str>,
        )?);
    }
    let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> = items
        .iter()
        .map(|item| item as &dyn tauri::menu::IsMenuItem<R>)
        .collect();
    Ok(Some(Submenu::with_items(
        app,
        "Open Recent Project",
        true,
        &refs,
    )?))
}
```

In **both** `build_app_menu` and `build_app_menu_with_plugins`, build the File submenu with the recents entry appended when present — replace the single `Submenu::with_items(app, "File", true, &[…])?` call with:

```rust
    let recent_projects = recent_projects_submenu(app)?;
    let mut file_items: Vec<&dyn tauri::menu::IsMenuItem<R>> = vec![
        &new_tab,
        &new_plain_shell_tab,
        &new_window,
        &new_file,
        &open_file,
        &open_folder,
    ];
    if let Some(recent) = recent_projects.as_ref() {
        file_items.push(recent);
    }
    file_items.extend([
        &save_file_as as &dyn tauri::menu::IsMenuItem<R>,
        &separator,
        &ssh_manager_menu,
        &separator2,
        &rename_tab,
        &close_tab,
        &close_window,
    ]);
    let file_menu = Submenu::with_items(app, "File", true, &file_items)?;
```

In `lib.rs`'s `on_menu_event`, add a fallthrough arm **before** the final `_ =>`:

```rust
            id if id.starts_with(menu::MENU_RECENT_PROJECT_PREFIX) => {
                // The index is resolved against the CURRENT recents list, so a
                // menu built before a project was opened cannot open a stale
                // path — a mismatched index simply finds nothing.
                let index: usize = id[menu::MENU_RECENT_PROJECT_PREFIX.len()..]
                    .parse()
                    .unwrap_or(usize::MAX);
                if let Some(entry) = project::recents::list_recents().into_iter().nth(index) {
                    menu::emit_menu_action_to_focused_window(
                        app,
                        &format!("{}{}", menu::MENU_ACTION_OPEN_RECENT_PROJECT, entry.path),
                    );
                }
            }
```

`emit_menu_action_to_focused_window` already takes `action: &str` (`menu.rs:1042-1044`) and clones it into `MenuActionEvent`, so the formatted string compiles as written — no signature change needed.

In `menu-actions.js`, after the `search-in-project` branch:

```javascript
      if (action.startsWith('open-recent-project:')) {
        const projectPath = action.slice('open-recent-project:'.length);
        Promise.resolve(invoke('project_open', { path: projectPath }))
          .catch((error) => {
            if (global.toast) global.toast.error('Cannot Open Project', projectPath + ': ' + String(error));
          });
        return;
      }
```

In `command-palette-runtime.js`'s `buildPaletteCommands`, add `invoke('project_recents').catch(() => [])` to the existing `Promise.all` destructuring (as `recentProjects`) and, after the `core:search-in-project` line:

```javascript
      for (const recent of (recentProjects || [])) {
        add(
          'core:reopen-project:' + recent.path,
          'Reopen Project: ' + recent.name,
          recent.path,
          'project reopen recent open folder workspace',
          () => {
            invoke('project_open', { path: recent.path }).catch((error) => {
              if (window.toast) window.toast.error('Cannot Open Project', String(error));
            });
          },
          'Project',
        );
      }
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p termlab_core --quiet` — PASS.
Run: `cargo test -p termlab_tauri --quiet` — PASS.
Run: `node scripts/tests/test_project_recents.mjs` — pass.
Run the full sweep — prints nothing.
Run: `cargo test --workspace --quiet` — PASS.
Run: `cargo clippy --all-targets` — no new warnings; `cargo fmt -- --check` shows no new drift.
Run: `bash scripts/check_frontend_boundaries.sh` — only `tl-dialog.js:334`.

- [ ] **Step 8: Commit and push**

```bash
git add crates/termlab_core/src/config/persistent.rs crates/termlab_core/src/config/mod.rs \
        crates/termlab_tauri/src/project/recents.rs crates/termlab_tauri/src/project/mod.rs \
        crates/termlab_tauri/src/commands.rs crates/termlab_tauri/src/menu.rs \
        crates/termlab_tauri/src/lib.rs \
        crates/termlab_tauri/frontend/app/menu-actions.js \
        crates/termlab_tauri/frontend/app/command-palette-runtime.js \
        scripts/tests/test_project_recents.mjs
git commit -m "feat: remember recent projects and restore per-project layouts"
git push -u origin codex/lsp-completion
```

---

## Manual verification checklist

Add these to the POC checklist when the plan is executed (spec section 8's manual list):

1. `termlab ~/some/repo` from a cold start — a project window opens, not zen, Files panel showing the tree, one terminal tab already at the project root, window titled after the folder.
2. `termlab ~/some/repo` again while that window is open — the existing window is focused; no second window, no blank window left behind.
3. File > Open Folder… from inside a project window — a **new** window opens; the original project is untouched.
4. The SFTP toggle in the project panel header — the dual-pane local+remote explorer appears, a remote host connects, and toggling back restores the tree with its expansion state re-listed.
5. Trust banner: on a never-trusted project it offers Trust project / Not now; Trust starts servers and the banner never returns after a restart; Not now hides it for the window only and the per-file status strip still works.
6. `gd` from a file under the root — no per-file root chooser appears; `gd` into a cargo-registry source opens a plain editable tab with no prompts.
7. `cmd+shift+f` in a project window — the Search panel activates and takes focus; a query streams results grouped by file; Enter on a row opens the file at the line and `Ctrl-O` returns.
8. Search under a dirty tree — `.gitignore`d directories produce no results; a query broad enough to exceed 1000 matches shows the "first 1000" wording.
9. Git tints while editing — save a file and watch its row change within a refresh cycle; its parent directories carry the modified tint; a non-repo project shows no tints and no toast.
10. Rename the project folder out from under an open window — the tree shows the missing-project state with Choose another folder…; open editor tabs keep working.
11. Close a project window with a distinctive panel arrangement, reopen it from File > Open Recent Project — the arrangement comes back; an ordinary new window is unaffected.
12. Delete a recent project's folder, then open any project — the deleted entry is gone from the menu on the next launch.
