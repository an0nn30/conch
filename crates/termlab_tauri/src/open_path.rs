//! CLI/IPC "open this path" routing: each request opens a new app window and
//! parks the path under that window's label until the window's frontend
//! pulls it with `take_pending_open_paths` — the same request-pull boot
//! pattern the panel host uses, so a slow-booting window cannot miss an
//! event that fired before it subscribed.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct PendingOpens(Mutex<HashMap<String, Vec<String>>>);

impl PendingOpens {
    pub(crate) fn take(&self, label: &str) -> Vec<String> {
        self.0.lock().unwrap().remove(label).unwrap_or_default()
    }

    /// Non-destructive: does this label have queued paths? The boot path asks
    /// this early (to decide the editor-window layout) before the destructive
    /// take runs later in startup.
    pub(crate) fn has(&self, label: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .get(label)
            .is_some_and(|paths| !paths.is_empty())
    }

    #[cfg(test)]
    fn peek(&self, label: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .get(label)
            .cloned()
            .unwrap_or_default()
    }
}

pub(crate) fn seed_for_label(pending: &PendingOpens, label: &str, mut paths: Vec<String>) {
    pending
        .0
        .lock()
        .unwrap()
        .entry(label.to_string())
        .or_default()
        .append(&mut paths);
}

/// Queue `path` under `label`, THEN build the window — and on a build
/// failure, drain the queue entry back out.
///
/// The ordering is the whole point, and it is why the label is allocated
/// separately from the build (see `windows::allocate_window_label`). A window
/// pulls its queue as soon as its webview boots; seeding after `build()`
/// returned meant a fast window racing a busy IPC thread could pull nothing
/// and drop the user's file on the floor. Draining on failure matters for the
/// mirror-image reason: an entry nobody will ever pull would otherwise sit
/// there until some later window happened to reuse the label.
///
/// Pure with respect to Tauri — the build is a closure — so both halves of
/// that contract are unit-testable.
pub(crate) fn seed_then_build<E: std::fmt::Display>(
    pending: &PendingOpens,
    label: &str,
    path: &str,
    build: impl FnOnce() -> Result<(), E>,
) {
    seed_for_label(pending, label, vec![path.to_string()]);
    if let Err(e) = build() {
        let orphaned = pending.take(label);
        log::error!("open-path: could not create window {label} for {orphaned:?}: {e}");
    }
}

/// Event that tells an already-running window it has queued open paths to
/// drain (via `take_pending_open_paths`).
pub(crate) const OPEN_PATHS_PENDING_EVENT: &str = "open-paths-pending";

/// Whether this window label belongs to a full app window (terminal/editor
/// chrome) that can route an open-path request into an editor tab. Panel
/// hosts and choosers boot without the editor runtime, so seeding their
/// queue would strand the path forever.
fn label_can_open_paths(label: &str) -> bool {
    label == "main" || label.starts_with("window-")
}

/// Pick which running window should receive an open-path request: the
/// focused full app window, else any full app window, else none (caller
/// falls back to building a fresh window). Pure over (label, focused) pairs
/// so the preference order is unit-testable without a window server.
pub(crate) fn pick_open_target(windows: &[(String, bool)]) -> Option<String> {
    let candidates: Vec<&(String, bool)> = windows
        .iter()
        .filter(|(label, _)| label_can_open_paths(label))
        .collect();
    candidates
        .iter()
        .find(|(_, focused)| *focused)
        .or_else(|| candidates.first())
        .map(|(label, _)| label.clone())
}

/// Open `path` in the running app: as a new editor tab in an existing window
/// when one is up (the focused one wins), and only in a fresh window when no
/// full app window exists — `termlab notes.md` next to a running instance
/// should read as "open a tab here", not "spawn another app window".
pub(crate) fn open_in_running_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>, path: &str) {
    use tauri::{Emitter, Manager};

    let windows = app.webview_windows();
    let candidates: Vec<(String, bool)> = windows
        .values()
        .map(|window| {
            (
                window.label().to_string(),
                window.is_focused().unwrap_or(false),
            )
        })
        .collect();
    let Some(label) = pick_open_target(&candidates) else {
        open_in_new_window(app, path);
        return;
    };
    let Some(window) = windows.get(&label) else {
        open_in_new_window(app, path);
        return;
    };

    // Seed BEFORE the emit for the same reason seed_then_build seeds before
    // the build: the drain must find the path no matter how fast it runs.
    seed_for_label(&app.state::<PendingOpens>(), &label, vec![path.to_string()]);
    let _ = window.emit(OPEN_PATHS_PENDING_EVENT, ());
    let _ = window.set_focus();
}

pub(crate) fn open_in_new_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>, path: &str) {
    use tauri::Manager;

    let label = crate::windows::allocate_window_label(app);
    let build_app = app.clone();
    let build_label = label.clone();

    seed_then_build(
        &app.state::<PendingOpens>(),
        &label,
        path,
        move || -> Result<(), String> {
            // This runs on the IPC listener thread, and window creation must
            // happen on the main thread (windows.rs documents why: the
            // builder's `build()` posts to the main thread and waits). Block
            // on the result so a failure can still drain the seed — safe
            // here because the main thread is never waiting on the IPC
            // thread, so there is no cycle to deadlock on.
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let inner = build_app.clone();
            build_app
                .run_on_main_thread(move || {
                    let result = crate::windows::create_window_with_label(&inner, &build_label)
                        .map_err(|e| e.to_string());
                    let _ = tx.send(result);
                })
                .map_err(|e| e.to_string())?;
            rx.recv().map_err(|e| e.to_string())?
        },
    );
}

#[tauri::command]
pub(crate) fn has_pending_open_paths(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, PendingOpens>,
) -> bool {
    state.has(window.label())
}

#[tauri::command]
pub(crate) fn take_pending_open_paths(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, PendingOpens>,
) -> Vec<String> {
    state.take(window.label())
}

#[cfg(test)]
mod tests {
    #[test]
    fn has_reports_without_draining() {
        use super::*;
        let pending = PendingOpens::default();
        assert!(!pending.has("main"), "empty map has nothing");
        seed_for_label(&pending, "main", vec!["/a.txt".into()]);
        assert!(pending.has("main"));
        assert_eq!(pending.take("main").len(), 1, "has() must not drain");
        assert!(!pending.has("main"), "drained label reports empty");
    }

    use super::*;

    #[test]
    fn open_target_prefers_focused_full_windows_and_ignores_hosts() {
        let windows = vec![
            ("panelhost-main-1".to_string(), true),
            ("main".to_string(), false),
            ("window-2".to_string(), false),
        ];
        assert_eq!(
            pick_open_target(&windows),
            Some("main".to_string()),
            "a focused panel host must not swallow the open; the first full window wins"
        );

        let focused = vec![("main".to_string(), false), ("window-2".to_string(), true)];
        assert_eq!(
            pick_open_target(&focused),
            Some("window-2".to_string()),
            "the focused full window wins over earlier unfocused ones"
        );

        let hosts_only = vec![("panelhost-main-1".to_string(), true)];
        assert_eq!(
            pick_open_target(&hosts_only),
            None,
            "no full app window means the caller builds a fresh one"
        );
        assert_eq!(pick_open_target(&[]), None);
    }

    #[test]
    fn take_returns_and_clears_per_label() {
        let pending = PendingOpens::default();
        seed_for_label(&pending, "main", vec!["/a.txt".into(), "/b.txt".into()]);
        seed_for_label(&pending, "window-1", vec!["/c.txt".into()]);
        assert_eq!(
            pending.take("main"),
            vec!["/a.txt".to_string(), "/b.txt".to_string()]
        );
        assert!(pending.take("main").is_empty(), "take drains");
        assert_eq!(pending.take("window-1"), vec!["/c.txt".to_string()]);
    }

    #[test]
    fn take_unknown_label_is_empty() {
        let pending = PendingOpens::default();
        assert!(pending.take("window-9").is_empty());
    }

    #[test]
    fn seed_appends_rather_than_replaces() {
        let pending = PendingOpens::default();
        seed_for_label(&pending, "main", vec!["/a.txt".into()]);
        seed_for_label(&pending, "main", vec!["/b.txt".into()]);
        assert_eq!(pending.take("main").len(), 2);
    }

    #[test]
    fn seed_lands_before_the_window_is_built() {
        // The race this guards: the new window's frontend can invoke
        // `take_pending_open_paths` the moment its webview boots. If the
        // seed happened only after `build()` returned, a fast window on a
        // busy IPC thread could pull an empty queue and the path would be
        // silently dropped. So the queue must already be populated at the
        // instant the build is asked for.
        let pending = PendingOpens::default();
        let mut seen_at_build_time: Option<Vec<String>> = None;
        seed_then_build(&pending, "window-7", "/tmp/a.txt", || {
            seen_at_build_time = Some(pending.peek("window-7"));
            Ok::<(), String>(())
        });
        assert_eq!(
            seen_at_build_time,
            Some(vec!["/tmp/a.txt".to_string()]),
            "the path must already be queued when the window build starts"
        );
        assert_eq!(pending.take("window-7"), vec!["/tmp/a.txt".to_string()]);
    }

    #[test]
    fn failed_build_drains_the_seed_back_out() {
        // Seeding first means a build failure leaves an entry nobody will
        // ever pull. It has to be drained back out, or a future window that
        // happens to reuse the label would open a file the user asked for
        // minutes ago in a completely unrelated window.
        let pending = PendingOpens::default();
        seed_then_build(&pending, "window-8", "/tmp/a.txt", || {
            Err::<(), String>("no window server".to_string())
        });
        assert!(
            pending.take("window-8").is_empty(),
            "a failed build must not leave an orphaned pending entry"
        );
    }

    #[test]
    fn failed_build_leaves_other_labels_alone() {
        let pending = PendingOpens::default();
        seed_for_label(&pending, "main", vec!["/keep.txt".into()]);
        seed_then_build(&pending, "window-9", "/tmp/a.txt", || {
            Err::<(), String>("nope".to_string())
        });
        assert_eq!(pending.take("main"), vec!["/keep.txt".to_string()]);
    }
}
