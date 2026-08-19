//! The file chooser as its own OS window, window-modal over the requesting
//! TermLab window.
//!
//! See `docs/superpowers/specs/2026-08-18-chooser-window-design.md` ("Window
//! & lifecycle") for the design this module implements.
//!
//! The registry ([`ChooserRegistry`]) is pure logic with no Tauri handles, so
//! its invariants — one chooser per parent, exactly-once resolution, stale
//! `req_id`s are no-ops — are unit-tested directly. Everything that touches a
//! real window (building it, emitting to the parent, persisting its size)
//! lives in the thin command/hook layer below it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use termlab_core::config::{self, ChooserWindowSize};

use crate::windows;

// ---------------------------------------------------------------------------
// Size floor
// ---------------------------------------------------------------------------

/// Content-fit floor (logical px), below which the sidebar/path-bar/header/
/// footer would not all fit without introducing a second scrolling region.
///
/// Derived from the shipped `file-dialog.css` (verified 2026-08-18 against
/// `crates/termlab_tauri/frontend/styles/design-system/components/file-dialog.css`
/// and `components/dialog.css`), the same layout this chooser inherits
/// verbatim per the design spec:
///
/// - Width: sidebar `flex: 0 0 150px` (`file-dialog.css:44`) + `.tl-filedlg`
///   gap `--tl-space-3` = 12px (`file-dialog.css:32`) + the path bar, the
///   widest single-line row in the main column: up button 26px
///   (`file-dialog.css:158`) + 3 × `--tl-space-1` (4px) gaps
///   (`file-dialog.css:150`) + crumbs/path/filter flex-basis floors
///   140+180+120 (`file-dialog.css:174,219,226`) = 478px main-column width.
///   150 + 12 + 478 = 640px of content, comfortably inside the 720px this
///   exact layout already ships at today as `.tl-dialog--lg`
///   (`components/dialog.css:40`) with `.tl-dialog__body`'s 2×16px
///   horizontal padding (`components/dialog.css:50`) removed (720 - 32 =
///   688px available, comfortably >= 640px needed).
/// - Height: path bar 26px + `--tl-space-2` gap (8px) + column header
///   `--tl-row-h` = 24px (`file-dialog.css:274`) + 8 visible rows ×
///   `--tl-row-h` (192px, chosen so the floor never clips a partial row —
///   the shipped dialog's `min-height: 200px`, `file-dialog.css:254`, was
///   tuned for a fixed dialog and clips a fraction of an 8th row) = 250px of
///   listing, leaving ~170px for the footer strip, the window's own
///   action-button row (not part of `file-dialog.css` — that lived in
///   `tl-dialog__footer`, `components/dialog.css:60-64`, and does not exist
///   in the standalone window yet), and top/bottom padding. 170px is in the
///   right order of magnitude for those (a `.tl-input`-height footer row at
///   26px, a `.tl-btn`-height action row at 26px plus `--tl-space-3` (12px)
///   padding on both sides, plus outer padding) but cannot be pinned exactly
///   until `chooser.html` exists (a later task) — no chooser markup or CSS
///   exists in this codebase as of this task (verified: no `chooser.html`
///   file present).
///
/// Both concrete sums (640 width, 250 height-of-listing-alone) land *under*
/// 720×420 with tens of pixels of slack, not over it — so there is no
/// evidence the floor is too small (which would be the actual bug: clipped
/// chrome). Erring larger than the bare minimum is a safe, ordinary product
/// choice, so this task leaves the constants and the spec's Sizing line
/// unchanged. See the Task 1 report for the full arithmetic.
pub(crate) const CHOOSER_MIN_WIDTH: f64 = 720.0;
pub(crate) const CHOOSER_MIN_HEIGHT: f64 = 420.0;

// ---------------------------------------------------------------------------
// Request/response types
// ---------------------------------------------------------------------------

/// A pending chooser's request, as sent to the chooser window via
/// `get_chooser_request`. `req_id` and `parent_label` are filled in by
/// [`ChooserRegistry::open`], not the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChooserRequest {
    pub req_id: u64,
    pub mode: String,
    pub filename: Option<String>,
    pub select_filename: bool,
    pub parent_label: String,
}

/// Event payload emitted to the parent window as `chooser-resolved` once a
/// chooser's outcome is decided. `choice: null` means cancel.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChooserResolvedEvent {
    pub req_id: u64,
    pub choice: Option<serde_json::Value>,
}

pub(crate) const CHOOSER_RESOLVED_EVENT: &str = "chooser-resolved";

// ---------------------------------------------------------------------------
// Registry (pure logic — no Tauri handles, so it is plain `cargo test`able)
// ---------------------------------------------------------------------------

static NEXT_CHOOSER_REQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub(crate) struct PendingChooser {
    pub req_id: u64,
    pub request: ChooserRequest,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChooserOpenError {
    /// A chooser is already open for this parent. Carries its `req_id` so the
    /// caller can hand back the same value instead of minting a new one.
    AlreadyOpen { req_id: u64 },
}

/// One-per-parent registry of in-flight choosers, keyed by the *parent's*
/// window label (never the chooser window's own label, which is derived from
/// it: `chooser-<parent_label>`).
#[derive(Debug, Default)]
pub(crate) struct ChooserRegistry {
    pending: HashMap<String, PendingChooser>,
}

impl ChooserRegistry {
    /// Register a new chooser for `parent_label`, or — if one is already
    /// live — report its existing `req_id` instead of creating a second
    /// entry. The registry assigns `req_id`, overwriting whatever the caller
    /// put in `request.req_id`.
    pub(crate) fn open(
        &mut self,
        parent_label: String,
        mut request: ChooserRequest,
    ) -> Result<PendingChooser, ChooserOpenError> {
        if let Some(existing) = self.pending.get(&parent_label) {
            return Err(ChooserOpenError::AlreadyOpen {
                req_id: existing.req_id,
            });
        }
        let req_id = NEXT_CHOOSER_REQ.fetch_add(1, Ordering::Relaxed);
        request.req_id = req_id;
        request.parent_label = parent_label.clone();
        let pending = PendingChooser { req_id, request };
        self.pending.insert(parent_label, pending.clone());
        Ok(pending)
    }

    /// Resolve the parent's chooser, but only if `req_id` matches the live
    /// entry. First caller wins: this removes the entry, so a second caller
    /// with any `req_id` (including the same one) finds nothing and is a
    /// no-op. A caller with a stale `req_id` is also a no-op and leaves the
    /// real entry live.
    pub(crate) fn resolve(&mut self, parent_label: &str, req_id: u64) -> Option<PendingChooser> {
        let matches = self
            .pending
            .get(parent_label)
            .is_some_and(|entry| entry.req_id == req_id);
        if matches {
            self.pending.remove(parent_label)
        } else {
            None
        }
    }

    /// Force-remove the parent's entry regardless of `req_id`. Used whenever
    /// there is no specific outcome to check against — the parent died, or the
    /// chooser window's own close button.
    pub(crate) fn resolve_for_parent_death(&mut self, parent_label: &str) -> Option<PendingChooser> {
        self.pending.remove(parent_label)
    }

    /// A parent cancelling its own chooser. `req_id` scopes it:
    ///
    /// * `Some(id)` — resolve only if the live entry IS that chooser. A cancel
    ///   that names a chooser which has already been answered is a no-op and
    ///   leaves whatever is live now untouched. This is the case that matters:
    ///   a cancel and the user's pick can cross on the wire, and by the time
    ///   the cancel lands the parent may have opened a *different* chooser —
    ///   an unscoped force-resolve would answer that one null instead, a ⌘O
    ///   that silently does nothing.
    /// * `None` — force-resolve whatever is live, as before. The parent has no
    ///   `req_id` to name until `open_file_chooser` has returned one, and a
    ///   cancel in that window still has to work.
    pub(crate) fn cancel(
        &mut self,
        parent_label: &str,
        req_id: Option<u64>,
    ) -> Option<PendingChooser> {
        match req_id {
            Some(id) => self.resolve(parent_label, id),
            None => self.resolve_for_parent_death(parent_label),
        }
    }

    /// The live request for this parent, if any (used by `get_chooser_request`).
    pub(crate) fn get(&self, parent_label: &str) -> Option<&PendingChooser> {
        self.pending.get(parent_label)
    }

    /// Whether this parent currently has a live chooser (used by the focus
    /// bounce on `WindowEvent::Focused`).
    pub(crate) fn contains_parent(&self, parent_label: &str) -> bool {
        self.pending.contains_key(parent_label)
    }
}

#[cfg(test)]
mod tests {
    use super::ChooserOpenError::AlreadyOpen;
    use super::*;

    fn req(mode: &str) -> ChooserRequest {
        ChooserRequest {
            req_id: 0,
            mode: mode.to_string(),
            filename: None,
            select_filename: false,
            parent_label: String::new(),
        }
    }

    #[test]
    fn open_registers_and_returns_ids() {
        let mut r = ChooserRegistry::default();
        let a = r.open("window-1".into(), req("open"));
        let b = r.open("window-2".into(), req("save"));
        assert_ne!(a.unwrap().req_id, b.unwrap().req_id);
    }

    #[test]
    fn duplicate_open_returns_existing_req_id_not_new_entry() {
        let mut r = ChooserRegistry::default();
        let first = r.open("window-1".into(), req("open")).unwrap();
        let dup = r.open("window-1".into(), req("open"));
        assert!(matches!(dup, Err(AlreadyOpen { req_id }) if req_id == first.req_id));
    }

    #[test]
    fn resolve_is_exactly_once() {
        let mut r = ChooserRegistry::default();
        let p = r.open("window-1".into(), req("open")).unwrap();
        assert!(r.resolve("window-1", p.req_id).is_some()); // first wins, returns entry
        assert!(r.resolve("window-1", p.req_id).is_none()); // late resolver: no-op
    }

    #[test]
    fn resolve_with_stale_req_id_is_noop() {
        let mut r = ChooserRegistry::default();
        let p = r.open("window-1".into(), req("open")).unwrap();
        assert!(r.resolve("window-1", p.req_id + 999).is_none());
        assert!(r.resolve("window-1", p.req_id).is_some()); // real one still live
    }

    #[test]
    fn parent_death_drains_only_that_parents_entry() {
        let mut r = ChooserRegistry::default();
        r.open("window-1".into(), req("open")).unwrap();
        let keep = r.open("window-2".into(), req("open")).unwrap();
        assert!(r.resolve_for_parent_death("window-1").is_some());
        assert!(r.resolve("window-2", keep.req_id).is_some());
    }

    #[test]
    fn duplicate_open_does_not_touch_the_existing_request() {
        let mut r = ChooserRegistry::default();
        r.open("window-1".into(), req("open")).unwrap();
        let _ = r.open("window-1".into(), req("save"));
        // The mode must still be the original request's — a duplicate open
        // must not overwrite the live entry.
        assert_eq!(r.get("window-1").unwrap().request.mode, "open");
    }

    #[test]
    fn cancel_with_a_stale_req_id_leaves_the_live_chooser_alone() {
        // The race this closes: the parent dispatches a cancel for chooser A,
        // A is answered before it lands, the parent opens chooser B, and only
        // then does the cancel arrive. Unscoped it would resolve B null — a ⌘O
        // that silently does nothing.
        let mut r = ChooserRegistry::default();
        let a = r.open("window-1".into(), req("save")).unwrap();
        assert!(r.resolve("window-1", a.req_id).is_some()); // A is answered
        let b = r.open("window-1".into(), req("open")).unwrap();
        assert_ne!(a.req_id, b.req_id);

        assert!(
            r.cancel("window-1", Some(a.req_id)).is_none(),
            "a cancel naming a chooser that is already gone resolves nothing"
        );
        assert!(
            r.contains_parent("window-1"),
            "and leaves the chooser that IS live untouched"
        );
        assert_eq!(r.get("window-1").unwrap().req_id, b.req_id);
    }

    #[test]
    fn cancel_with_the_matching_req_id_resolves_it() {
        let mut r = ChooserRegistry::default();
        let p = r.open("window-1".into(), req("save")).unwrap();
        let cancelled = r.cancel("window-1", Some(p.req_id));
        assert_eq!(cancelled.map(|c| c.req_id), Some(p.req_id));
        assert!(!r.contains_parent("window-1"));
    }

    #[test]
    fn cancel_without_a_req_id_still_force_resolves() {
        // The parent has no id to name until `open_file_chooser` has returned
        // one, and a cancel in that window must still work.
        let mut r = ChooserRegistry::default();
        let p = r.open("window-1".into(), req("save")).unwrap();
        assert_eq!(r.cancel("window-1", None).map(|c| c.req_id), Some(p.req_id));
        assert!(!r.contains_parent("window-1"));
        // And it is a no-op when there is nothing live, as the mid-build cancel
        // that races ahead of the registry insert always is.
        assert!(r.cancel("window-1", None).is_none());
    }

    #[test]
    fn cancel_never_reaches_another_parents_chooser() {
        let mut r = ChooserRegistry::default();
        let mine = r.open("window-1".into(), req("save")).unwrap();
        let theirs = r.open("window-2".into(), req("open")).unwrap();
        assert!(r.cancel("window-1", Some(mine.req_id)).is_some());
        assert!(r.contains_parent("window-2"));
        assert_eq!(r.get("window-2").unwrap().req_id, theirs.req_id);
    }

    #[test]
    fn get_and_contains_parent_reflect_registry_state() {
        let mut r = ChooserRegistry::default();
        assert!(!r.contains_parent("window-1"));
        assert!(r.get("window-1").is_none());
        r.open("window-1".into(), req("open")).unwrap();
        assert!(r.contains_parent("window-1"));
        assert!(r.get("window-1").is_some());
    }
}

// ---------------------------------------------------------------------------
// Window builder
// ---------------------------------------------------------------------------

/// Clamp a persisted (or default-floor) size to `[floor, monitor work area]`.
/// The floor wins if the two conflict (a monitor smaller than the floor is
/// an edge case `min_inner_size` on the builder is the real backstop for).
fn clamp_dimension(value: f64, floor: f64, monitor_max: Option<f64>) -> f64 {
    let floored = value.max(floor);
    match monitor_max {
        Some(max) if max >= floor => floored.min(max),
        _ => floored,
    }
}

/// The size to open the chooser at: the persisted size if any, else the
/// floor, clamped to `[floor, parent's monitor work area]`.
fn clamped_chooser_size<R: tauri::Runtime>(
    parent: &tauri::WebviewWindow<R>,
    persisted: Option<ChooserWindowSize>,
) -> (f64, f64) {
    let (w, h) = persisted
        .map(|s| (s.width, s.height))
        .unwrap_or((CHOOSER_MIN_WIDTH, CHOOSER_MIN_HEIGHT));

    let (max_w, max_h) = match parent.current_monitor() {
        Ok(Some(monitor)) if monitor.scale_factor() > 0.0 => {
            let scale = monitor.scale_factor();
            (
                Some(monitor.work_area().size.width as f64 / scale),
                Some(monitor.work_area().size.height as f64 / scale),
            )
        }
        _ => (None, None),
    };

    (
        clamp_dimension(w, CHOOSER_MIN_WIDTH, max_w),
        clamp_dimension(h, CHOOSER_MIN_HEIGHT, max_h),
    )
}

/// The chooser's initial position: centered on the parent's current bounds,
/// computed before build. The window is never moved after it is shown.
fn centered_position<R: tauri::Runtime>(
    parent: &tauri::WebviewWindow<R>,
    target_w: f64,
    target_h: f64,
) -> Option<(f64, f64)> {
    let outer_pos = parent.outer_position().ok()?;
    let outer_size = parent.outer_size().ok()?;
    let scale = parent.scale_factor().ok()?;
    if scale <= 0.0 {
        return None;
    }
    let parent_x = outer_pos.x as f64 / scale;
    let parent_y = outer_pos.y as f64 / scale;
    let parent_w = outer_size.width as f64 / scale;
    let parent_h = outer_size.height as f64 / scale;
    Some((
        parent_x + (parent_w - target_w) / 2.0,
        parent_y + (parent_h - target_h) / 2.0,
    ))
}

/// Build (hidden) the chooser window for `parent_label`. Must run on the
/// main thread — see `windows.rs:42-51` for the deadlock rule this follows.
///
/// On failure the caller is responsible for removing the registry entry: a
/// registry entry with no window behind it is a permanently stuck scrim on
/// the parent.
fn create_chooser_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    parent_label: &str,
    request: &ChooserRequest,
) -> Result<(), String> {
    let parent = app
        .get_webview_window(parent_label)
        .ok_or_else(|| format!("parent window '{parent_label}' not found"))?;

    let user_cfg = config::load_user_config().unwrap_or_default();
    let theme = windows::appearance_to_theme(&user_cfg.colors.appearance_mode);
    let use_custom_titlebar = cfg!(target_os = "windows") || cfg!(target_os = "linux");

    let persisted_state = config::load_persistent_state().unwrap_or_default();
    let (target_w, target_h) = clamped_chooser_size(&parent, persisted_state.chooser_window);

    let title = if request.mode == "save" { "Save As" } else { "Open" };
    let chooser_label = format!("chooser-{parent_label}");

    let mut builder =
        WebviewWindowBuilder::new(app, &chooser_label, WebviewUrl::App("chooser.html".into()))
            .title(title)
            .inner_size(target_w, target_h)
            .min_inner_size(CHOOSER_MIN_WIDTH, CHOOSER_MIN_HEIGHT)
            .resizable(true)
            .visible(false)
            .decorations(!use_custom_titlebar)
            .theme(theme)
            // Modal windows do not minimize (spec: "Window & lifecycle").
            .minimizable(false);

    if let Some((x, y)) = centered_position(&parent, target_w, target_h) {
        builder = builder.position(x, y);
    }

    // Owner relationship on macOS/Windows only; Linux relies on the focus
    // bounce (WindowEvent::Focused hook below) for modal behavior.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        builder = builder.parent(&parent).map_err(|e| e.to_string())?;
    }

    let win = builder.build().map_err(|e| e.to_string())?;
    let _ = win.remove_menu();

    // Same rescue as every other hidden-until-ready window (windows.rs:144).
    crate::arm_window_show_fallback(app, win.label());
    Ok(())
}

// ---------------------------------------------------------------------------
// Completion helpers (emit + persist + close)
// ---------------------------------------------------------------------------

/// Persist the chooser window's current inner size (logical px) into
/// `PersistentState.chooser_window`, load-mutate-save since that is the only
/// API `termlab_core::config` offers (`config/mod.rs:253,266`).
fn persist_chooser_size(width: f64, height: f64) {
    let mut state = config::load_persistent_state().unwrap_or_default();
    state.chooser_window = Some(ChooserWindowSize { width, height });
    if let Err(e) = config::save_persistent_state(&state) {
        log::warn!("failed to persist chooser window size: {e}");
    }
}

/// Read a window's current inner size in logical px and persist it. Silently
/// does nothing if either read fails (a closing window can race this).
fn persist_chooser_size_from<R: tauri::Runtime>(win: &tauri::WebviewWindow<R>) {
    let (Ok(inner), Ok(scale)) = (win.inner_size(), win.scale_factor()) else {
        return;
    };
    if scale <= 0.0 {
        return;
    }
    persist_chooser_size(inner.width as f64 / scale, inner.height as f64 / scale);
}

/// Emit the outcome to the parent, persist the chooser's final size, and
/// close it. Used by every completion path that still needs to close the
/// chooser window itself: `resolve_file_chooser`, `cancel_file_chooser`, and
/// the parent-death hook. (The chooser's own close-button path does not use
/// this — see `on_chooser_close_requested` — because the window is already
/// mid-teardown there and a second `close()` call is unnecessary.)
fn complete_chooser<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    parent_label: &str,
    req_id: u64,
    choice: Option<serde_json::Value>,
) {
    let payload = ChooserResolvedEvent { req_id, choice };
    let _ = app.emit_to(parent_label, CHOOSER_RESOLVED_EVENT, &payload);

    let chooser_label = format!("chooser-{parent_label}");
    if let Some(win) = app.get_webview_window(&chooser_label) {
        persist_chooser_size_from(&win);
        let _ = win.close();
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Which window labels may call `open_file_chooser`. Pulled out as a pure
/// function (no Tauri handle needed) so the rejection rules are unit-tested
/// directly, the same way the registry's invariants are — see the design
/// spec, "Window & lifecycle"
/// (`docs/superpowers/specs/2026-08-18-chooser-window-design.md:24`):
/// "callable only from a main-app window (reject labels starting
/// `chooser-`/`settings`)".
fn validate_chooser_caller(label: &str) -> Result<(), String> {
    if label.starts_with("chooser-") {
        return Err("chooser windows cannot open choosers".to_string());
    }
    if label == "settings" {
        return Err("the settings window cannot open a chooser".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod caller_validation_tests {
    use super::validate_chooser_caller;

    #[test]
    fn rejects_chooser_windows() {
        assert!(validate_chooser_caller("chooser-window-1").is_err());
        assert!(validate_chooser_caller("chooser-main").is_err());
    }

    #[test]
    fn rejects_the_settings_window() {
        assert!(validate_chooser_caller("settings").is_err());
    }

    #[test]
    fn allows_ordinary_windows() {
        assert!(validate_chooser_caller("main").is_ok());
        assert!(validate_chooser_caller("window-1").is_ok());
    }
}

/// Open (or focus an existing) chooser for the calling window. Rejects
/// callers whose own label starts with `chooser-` — a chooser cannot open a
/// chooser — and the settings window (design spec, "Window & lifecycle",
/// `docs/superpowers/specs/2026-08-18-chooser-window-design.md:24`).
#[tauri::command]
pub(crate) async fn open_file_chooser(
    window: tauri::WebviewWindow,
    mode: String,
    filename: Option<String>,
    select_filename: bool,
) -> Result<u64, String> {
    let parent_label = window.label().to_string();
    validate_chooser_caller(&parent_label)?;

    let app = window.app_handle().clone();
    let request = ChooserRequest {
        req_id: 0, // assigned by ChooserRegistry::open
        mode,
        filename,
        select_filename,
        parent_label: parent_label.clone(),
    };

    let opened = {
        let registry = app.state::<Mutex<ChooserRegistry>>();
        registry.lock().open(parent_label.clone(), request)
    };

    let pending = match opened {
        Ok(pending) => pending,
        Err(ChooserOpenError::AlreadyOpen { req_id }) => {
            let chooser_label = format!("chooser-{parent_label}");
            if let Some(win) = app.get_webview_window(&chooser_label) {
                let _ = win.set_focus();
            }
            return Ok(req_id);
        }
    };

    // Window creation must happen on the main thread — same deadlock rule as
    // open_new_window/open_settings_window (windows.rs:42-51).
    let handle = app.clone();
    let build_parent_label = parent_label.clone();
    let build_request = pending.request.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    handle
        .clone()
        .run_on_main_thread(move || {
            let result = create_chooser_window(&handle, &build_parent_label, &build_request);
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;
    let build_result = rx.recv().map_err(|e| e.to_string())?;

    if let Err(e) = build_result {
        // A registry entry with no window behind it is a permanently stuck
        // scrim on the parent — remove it before reporting the failure.
        let registry = app.state::<Mutex<ChooserRegistry>>();
        registry.lock().resolve_for_parent_death(&parent_label);
        return Err(e);
    }

    Ok(pending.req_id)
}

/// The pending request for the calling (chooser) window, derived from its
/// own label.
#[tauri::command]
pub(crate) fn get_chooser_request(
    window: tauri::WebviewWindow,
    registry: tauri::State<'_, Mutex<ChooserRegistry>>,
) -> Result<ChooserRequest, String> {
    let label = window.label();
    let parent_label = label
        .strip_prefix("chooser-")
        .ok_or_else(|| "not a chooser window".to_string())?;
    registry
        .lock()
        .get(parent_label)
        .map(|p| p.request.clone())
        .ok_or_else(|| "no pending chooser".to_string())
}

/// The chooser's answer: a pick (`Some`) or an explicit cancel path that
/// still goes through the chooser (`None`, though most cancels go through
/// `cancel_file_chooser` instead). Called by the chooser window itself.
#[tauri::command]
pub(crate) async fn resolve_file_chooser(
    window: tauri::WebviewWindow,
    req_id: u64,
    choice: Option<serde_json::Value>,
) -> Result<(), String> {
    let app = window.app_handle().clone();
    let label = window.label().to_string();
    let parent_label = label
        .strip_prefix("chooser-")
        .ok_or_else(|| "not a chooser window".to_string())?
        .to_string();

    let resolved = {
        let registry = app.state::<Mutex<ChooserRegistry>>();
        registry.lock().resolve(&parent_label, req_id)
    };
    let Some(pending) = resolved else {
        return Ok(()); // late resolver — someone else already settled this
    };

    complete_chooser(&app, &parent_label, pending.req_id, choice);
    Ok(())
}

/// Cancel the calling (PARENT) window's chooser.
///
/// `req_id` scopes the cancel to one specific chooser and SHOULD be passed
/// whenever the caller knows it: two IPC calls from the same window have no
/// ordering guarantee between them, so a cancel can land after the chooser it
/// was for has already been answered — and force-resolving "whatever is live"
/// at that point answers the parent's NEXT chooser null instead.
///
/// `None` keeps the original unconditional behaviour, which is still needed:
/// the parent has no `req_id` to name until `open_file_chooser` has returned
/// one, and a cancel in that window (a pane torn down while the chooser window
/// is still being built) must still work.
#[tauri::command]
pub(crate) async fn cancel_file_chooser(
    window: tauri::WebviewWindow,
    req_id: Option<u64>,
) -> Result<(), String> {
    let app = window.app_handle().clone();
    let parent_label = window.label().to_string();
    if parent_label.starts_with("chooser-") {
        return Err("cancel_file_chooser must be called by the parent window".to_string());
    }

    let resolved = {
        let registry = app.state::<Mutex<ChooserRegistry>>();
        registry.lock().cancel(&parent_label, req_id)
    };
    let Some(pending) = resolved else {
        return Ok(());
    };

    complete_chooser(&app, &parent_label, pending.req_id, None);
    Ok(())
}

/// Show and focus the chooser window once its frontend reports first paint —
/// mirror of `app_ready` (`commands.rs:248-251`).
#[tauri::command]
pub(crate) fn chooser_ready(window: tauri::WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Focus the caller's chooser window, if one exists. Called by the parent
/// when a same-mode accelerator (⌘O/⌘S) shares the existing promise instead
/// of opening a second chooser.
#[tauri::command]
pub(crate) fn focus_file_chooser(window: tauri::WebviewWindow) -> Result<(), String> {
    let parent_label = window.label();
    let chooser_label = format!("chooser-{parent_label}");
    if let Some(win) = window.app_handle().get_webview_window(&chooser_label) {
        let _ = win.set_focus();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle hooks — called from lib.rs's `on_window_event`
// ---------------------------------------------------------------------------

/// `WindowEvent::Destroyed`: if the destroyed window was some chooser's
/// parent, resolve it as cancelled (the emit is a no-op — the parent is
/// gone) and close the chooser window. The `.parent()` owner relationship
/// (macOS/Windows) is not trusted alone to do this on every platform.
pub(crate) fn on_window_destroyed<R: tauri::Runtime>(window: &tauri::Window<R>) {
    let app = window.app_handle();
    let label = window.label().to_string();
    let Some(registry) = app.try_state::<Mutex<ChooserRegistry>>() else {
        return;
    };
    let resolved = registry.lock().resolve_for_parent_death(&label);
    if let Some(pending) = resolved {
        complete_chooser(app, &label, pending.req_id, None);
    }
}

/// `WindowEvent::CloseRequested` on a `chooser-*` window: treat the native
/// close button as Cancel. The close itself is allowed to proceed (this does
/// not call `prevent_close`), so no second `close()` call is made here —
/// only the registry cleanup, the emit, and the size persistence (which
/// needs the window to still be alive, hence doing it here rather than in
/// `Destroyed`).
pub(crate) fn on_chooser_close_requested<R: tauri::Runtime>(window: &tauri::Window<R>) {
    let label = window.label();
    let Some(parent_label) = label.strip_prefix("chooser-") else {
        return;
    };
    let app = window.app_handle();
    let Some(registry) = app.try_state::<Mutex<ChooserRegistry>>() else {
        return;
    };
    let resolved = registry.lock().resolve_for_parent_death(parent_label);
    let Some(pending) = resolved else {
        return;
    };

    let payload = ChooserResolvedEvent {
        req_id: pending.req_id,
        choice: None,
    };
    let _ = window.emit_to(parent_label, CHOOSER_RESOLVED_EVENT, &payload);
    if let (Ok(inner), Ok(scale)) = (window.inner_size(), window.scale_factor())
        && scale > 0.0
    {
        persist_chooser_size(inner.width as f64 / scale, inner.height as f64 / scale);
    }
}

/// `WindowEvent::Focused(true)`: the modal focus bounce. While a chooser is
/// registered for this window, clicking back into the parent redirects
/// focus to its chooser — the cross-platform half of modality (works
/// uniformly including Linux, where `.parent()` is best-effort).
pub(crate) fn on_window_focused<R: tauri::Runtime>(window: &tauri::Window<R>) {
    let app = window.app_handle();
    let label = window.label();
    let Some(registry) = app.try_state::<Mutex<ChooserRegistry>>() else {
        return;
    };
    if !registry.lock().contains_parent(label) {
        return;
    }
    if let Some(chooser_win) = app.get_webview_window(&format!("chooser-{label}")) {
        let _ = chooser_win.set_focus();
    }
}
