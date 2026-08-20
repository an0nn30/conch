//! Pop-out tool windows ("panel hosts"): a tool window rendered in its own
//! OS window instead of docked in a zone.
//!
//! See `docs/superpowers/specs/2026-08-19-popout-tool-windows-design.md`
//! ("Rust: `panel_host.rs`") for the design this module implements, and
//! `chooser_window.rs` for the sibling this one deliberately mirrors —
//! unique request-scoped window labels, registry-removal-first exactly-once
//! teardown, and destroy-not-close for every path except the one below.
//!
//! **The one deliberate divergence from the chooser:** a panel host entry is
//! PERSISTENT. Where a chooser is answered once and gone, a popped-out tool
//! window survives being hidden — its webview and panel state stay alive so
//! summoning it again (`focus_panel_host`) is instant. The registry entry
//! only goes away on dock-back, on the parent's death, or on being displaced
//! by a fresh `open_panel_host` for the same id — never on a plain hide.
//!
//! The registry ([`PanelHostRegistry`]) is pure logic with no Tauri handles,
//! so its invariants are unit-tested directly, exactly as
//! `chooser_window::ChooserRegistry`'s are.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use termlab_core::config::{self, WindowBoundsRecord};

use crate::windows;

// ---------------------------------------------------------------------------
// Size floor / default
// ---------------------------------------------------------------------------

/// Content-fit floor (logical px) for a popped-out tool window — task-1-brief.md.
pub(crate) const PANEL_HOST_MIN_WIDTH: f64 = 360.0;
pub(crate) const PANEL_HOST_MIN_HEIGHT: f64 = 240.0;

/// Default size when a tool window has never been popped out before (no
/// persisted bounds for its id) — task-1-brief.md.
const PANEL_HOST_DEFAULT_WIDTH: f64 = 520.0;
const PANEL_HOST_DEFAULT_HEIGHT: f64 = 400.0;

// ---------------------------------------------------------------------------
// Request/response types
// ---------------------------------------------------------------------------

/// A pending panel host's request, as sent to the host window via
/// `get_panel_host_request`. Filled in by [`PanelHostRegistry::open`], not
/// the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PanelHostRequest {
    pub req_id: u64,
    pub tool_window_id: String,
    pub parent_label: String,
    pub title: String,
}

/// Event payload for every panel-host event that names a single tool window:
/// `panel-host-docked`, `panel-host-hidden`, `panel-host-shown`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PanelHostToolWindowEvent {
    pub tool_window_id: String,
}

/// Event payload for `panel_host_broadcast`'s re-dispatch to every live host
/// of the calling parent.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PanelHostBroadcastEvent {
    pub event: String,
    pub payload: serde_json::Value,
}

pub(crate) const PANEL_HOST_DOCKED_EVENT: &str = "panel-host-docked";
pub(crate) const PANEL_HOST_HIDDEN_EVENT: &str = "panel-host-hidden";
pub(crate) const PANEL_HOST_SHOWN_EVENT: &str = "panel-host-shown";
pub(crate) const PANEL_HOST_BROADCAST_EVENT: &str = "panel-host-event";
/// Emitted to the parent by `abort_panel_host` — the boot's
/// unknown-tool-window-id self-close path. Distinct from
/// `panel-host-docked`: a dock-back means "remount this panel in its zone",
/// while an abort means "this host never had a panel to remount at all" —
/// the manager's response is to reset the tool window's view mode back to
/// dock without expecting anything to remount (Task 3 consumes this).
pub(crate) const PANEL_HOST_ABORTED_EVENT: &str = "panel-host-aborted";

// ---------------------------------------------------------------------------
// Registry (pure logic — no Tauri handles, so it is plain `cargo test`able)
// ---------------------------------------------------------------------------

static NEXT_PANEL_HOST_REQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub(crate) struct PanelHostEntry {
    pub req_id: u64,
    /// The host WINDOW's label, `panelhost-<parent_label>-<req_id>`, built
    /// exactly once here at open time — unique per request, for the same
    /// reason `chooser_window::PendingChooser::window_label` is: Tauri only
    /// clears a label from its window map once that window's `Destroyed`
    /// event round-trips the event loop, so a same-label rebuild (e.g. a
    /// rapid dock-back followed by a re-open) would collide with a teardown
    /// still in flight. Every lookup goes through THIS stored value; nothing
    /// may re-derive the parent by parsing the label (parent labels contain
    /// dashes themselves).
    pub window_label: String,
    pub tool_window_id: String,
    pub parent_label: String,
    pub title: String,
    /// Whether the host window is currently shown. Unlike the chooser, this
    /// entry survives a hide — `visible` is the only thing that changes.
    pub visible: bool,
}

/// Registry of live panel hosts, keyed by `(parent_label, tool_window_id)` —
/// at most one host per tool window per parent. The host window's own label
/// is stored per entry ([`PanelHostEntry::window_label`]) and is unique per
/// request.
#[derive(Debug, Default)]
pub(crate) struct PanelHostRegistry {
    hosts: HashMap<(String, String), PanelHostEntry>,
}

impl PanelHostRegistry {
    /// Register a new host for `(parent_label, tool_window_id)` with a
    /// freshly minted `req_id` and a freshly built, request-unique
    /// `window_label`, returning it alongside whatever entry it displaced (if
    /// any) — the command layer destroys that window first, then builds the
    /// replacement under the fresh label. This is cancel-and-recreate, never
    /// adopt: a live entry at this key is abnormal (the frontend should have
    /// routed a repeat request through `focus_panel_host` instead), and
    /// handing the caller the OLD window would be a summon wearing a
    /// mismatched title.
    pub(crate) fn open(
        &mut self,
        parent_label: String,
        tool_window_id: String,
        title: String,
    ) -> (Option<PanelHostEntry>, PanelHostEntry) {
        let req_id = NEXT_PANEL_HOST_REQ.fetch_add(1, Ordering::Relaxed);
        let window_label = format!("panelhost-{parent_label}-{req_id}");
        let key = (parent_label.clone(), tool_window_id.clone());
        let displaced = self.hosts.remove(&key);
        let entry = PanelHostEntry {
            req_id,
            window_label,
            tool_window_id,
            parent_label,
            title,
            visible: false,
        };
        self.hosts.insert(key, entry.clone());
        (displaced, entry)
    }

    /// The live entry for `(parent_label, tool_window_id)`, if any.
    pub(crate) fn get(&self, parent_label: &str, tool_window_id: &str) -> Option<&PanelHostEntry> {
        self.hosts
            .get(&(parent_label.to_string(), tool_window_id.to_string()))
    }

    /// The live entry whose host WINDOW is `window_label`, if any. Exact
    /// match on the stored label, deliberately — see the comment on
    /// [`PanelHostEntry::window_label`] and
    /// `chooser_window::ChooserRegistry::get_by_window_label`, whose ban on
    /// parsing the parent out of the label applies here identically.
    pub(crate) fn get_by_window_label(&self, window_label: &str) -> Option<&PanelHostEntry> {
        self.hosts.values().find(|e| e.window_label == window_label)
    }

    /// Every live host belonging to `parent_label`, for `panel_host_broadcast`.
    pub(crate) fn hosts_of_parent(&self, parent_label: &str) -> Vec<PanelHostEntry> {
        self.hosts
            .values()
            .filter(|e| e.parent_label == parent_label)
            .cloned()
            .collect()
    }

    /// Remove and return the entry for `(parent_label, tool_window_id)`, if
    /// any. The only place an entry is ever taken out of the registry —
    /// dock-back, displacement (via `open`), and parent-death drain (via
    /// `drain_parent`) all funnel through here or through `open`, so removal
    /// is exactly-once by construction: a second removal of the same key
    /// finds nothing.
    pub(crate) fn remove(
        &mut self,
        parent_label: &str,
        tool_window_id: &str,
    ) -> Option<PanelHostEntry> {
        self.hosts
            .remove(&(parent_label.to_string(), tool_window_id.to_string()))
    }

    /// Remove and return every entry belonging to `parent_label` — the
    /// parent-death path. Other parents' entries are untouched.
    pub(crate) fn drain_parent(&mut self, parent_label: &str) -> Vec<PanelHostEntry> {
        let keys: Vec<(String, String)> = self
            .hosts
            .keys()
            .filter(|(p, _)| p == parent_label)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|k| self.hosts.remove(&k))
            .collect()
    }

    /// Mutate the `visible` flag on the entry for `(parent_label,
    /// tool_window_id)`. Returns whether an entry existed to mutate — a
    /// no-op `false` for a key that is not (or is no longer) registered.
    pub(crate) fn set_visible(
        &mut self,
        parent_label: &str,
        tool_window_id: &str,
        visible: bool,
    ) -> bool {
        match self
            .hosts
            .get_mut(&(parent_label.to_string(), tool_window_id.to_string()))
        {
            Some(entry) => {
                entry.visible = visible;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_registers_and_returns_ids() {
        let mut r = PanelHostRegistry::default();
        let (displaced_a, a) = r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        let (displaced_b, b) = r.open("window-2".into(), "ssh-sessions".into(), "SSH".into());
        assert!(displaced_a.is_none());
        assert!(displaced_b.is_none());
        assert_ne!(a.req_id, b.req_id);
    }

    #[test]
    fn window_labels_are_unique_per_request_and_prefixed() {
        let mut r = PanelHostRegistry::default();
        let (_, a) = r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        assert_eq!(a.window_label, format!("panelhost-window-1-{}", a.req_id));
        assert!(
            a.window_label.starts_with("panelhost-"),
            "validate_panel_host_caller and the CloseRequested/Destroyed hooks key on this prefix"
        );
    }

    #[test]
    fn a_second_open_for_the_same_key_displaces_the_first_and_mints_a_new_req_id() {
        let mut r = PanelHostRegistry::default();
        let (_, first) = r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());

        let (displaced, second) =
            r.open("window-1".into(), "ssh-sessions".into(), "SSH v2".into());
        assert_eq!(
            displaced.map(|d| d.req_id),
            Some(first.req_id),
            "the old entry comes back to the caller, to be destroyed"
        );
        assert_ne!(
            second.req_id, first.req_id,
            "the replacement is a host of its own, never the old id reused"
        );
        assert_ne!(
            second.window_label, first.window_label,
            "and its WINDOW is fresh too: Tauri clears a destroyed window's \
             label asynchronously, so a same-label rebuild would collide"
        );
        assert_eq!(
            r.get("window-1", "ssh-sessions").unwrap().req_id,
            second.req_id
        );
        assert_eq!(
            r.get("window-1", "ssh-sessions").unwrap().title,
            "SSH v2",
            "the live entry is the NEW request, never the one it replaced"
        );
    }

    #[test]
    fn displacement_only_affects_the_matching_key_not_a_different_tool_window_on_the_same_parent()
    {
        let mut r = PanelHostRegistry::default();
        let (_, ssh) = r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        let (_, files) = r.open("window-1".into(), "file-explorer".into(), "Files".into());

        let (displaced, _) = r.open("window-1".into(), "ssh-sessions".into(), "SSH v2".into());
        assert_eq!(displaced.map(|d| d.req_id), Some(ssh.req_id));
        assert_eq!(
            r.get("window-1", "file-explorer").unwrap().req_id,
            files.req_id,
            "an unrelated tool window on the same parent is untouched"
        );
    }

    #[test]
    fn lookup_by_window_label_finds_the_live_entry_and_misses_a_stale_one() {
        let mut r = PanelHostRegistry::default();
        let (_, first) = r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        let (_, other) = r.open("window-2".into(), "file-explorer".into(), "Files".into());

        assert_eq!(
            r.get_by_window_label(&first.window_label).unwrap().req_id,
            first.req_id
        );
        assert_eq!(
            r.get_by_window_label(&other.window_label).unwrap().req_id,
            other.req_id
        );

        let (_, second) = r.open("window-1".into(), "ssh-sessions".into(), "SSH v2".into());
        assert!(
            r.get_by_window_label(&first.window_label).is_none(),
            "a displaced host's stale label names NOTHING"
        );
        assert_eq!(
            r.get_by_window_label(&second.window_label).unwrap().req_id,
            second.req_id
        );
    }

    #[test]
    fn hosts_of_parent_returns_only_that_parents_live_hosts() {
        let mut r = PanelHostRegistry::default();
        r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        r.open("window-1".into(), "file-explorer".into(), "Files".into());
        r.open("window-2".into(), "ssh-sessions".into(), "SSH".into());

        let mut ids: Vec<String> = r
            .hosts_of_parent("window-1")
            .into_iter()
            .map(|e| e.tool_window_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["file-explorer".to_string(), "ssh-sessions".to_string()]);
    }

    #[test]
    fn remove_is_exactly_once() {
        let mut r = PanelHostRegistry::default();
        r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        assert!(r.remove("window-1", "ssh-sessions").is_some());
        assert!(
            r.remove("window-1", "ssh-sessions").is_none(),
            "a second remove of the same key is a no-op, not a double-teardown"
        );
    }

    #[test]
    fn drain_parent_leaves_other_parents_entries_alone() {
        let mut r = PanelHostRegistry::default();
        r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        r.open("window-1".into(), "file-explorer".into(), "Files".into());
        let (_, keep) = r.open("window-2".into(), "ssh-sessions".into(), "SSH".into());

        let drained = r.drain_parent("window-1");
        assert_eq!(drained.len(), 2);
        assert!(r.get("window-1", "ssh-sessions").is_none());
        assert!(r.get("window-1", "file-explorer").is_none());
        assert_eq!(r.get("window-2", "ssh-sessions").unwrap().req_id, keep.req_id);

        // Draining an already-drained (or never-populated) parent is a no-op.
        assert!(r.drain_parent("window-1").is_empty());
    }

    #[test]
    fn set_visible_transitions_the_flag_and_reports_whether_the_entry_existed() {
        let mut r = PanelHostRegistry::default();
        r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        assert!(!r.get("window-1", "ssh-sessions").unwrap().visible);

        assert!(r.set_visible("window-1", "ssh-sessions", true));
        assert!(r.get("window-1", "ssh-sessions").unwrap().visible);

        assert!(r.set_visible("window-1", "ssh-sessions", false));
        assert!(!r.get("window-1", "ssh-sessions").unwrap().visible);

        assert!(
            !r.set_visible("window-1", "no-such-id", true),
            "mutating an unregistered key is a no-op, reported as false"
        );
    }

    #[test]
    fn entries_survive_hide_only_dock_and_drain_actually_remove_them() {
        // The one deliberate divergence from the chooser: hiding a panel host
        // does NOT remove its registry entry (`set_visible`, not `remove`).
        let mut r = PanelHostRegistry::default();
        r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        r.set_visible("window-1", "ssh-sessions", false);
        assert!(
            r.get("window-1", "ssh-sessions").is_some(),
            "hidden is not gone — summon must still find it"
        );
    }

    #[test]
    fn abort_via_window_label_resolves_the_entry_exactly_once() {
        // Registry-level mirror of `abort_panel_host`'s own lookup-then-remove
        // sequence (the boot's unknown-tool-window-id self-close path):
        // resolve the caller's own window_label to its (parent, id), then
        // remove by that key. A second call with the same window_label must
        // find nothing — the entry is gone, not just its window.
        let mut r = PanelHostRegistry::default();
        let (_, p) = r.open("window-1".into(), "ssh-sessions".into(), "SSH".into());

        let key = r
            .get_by_window_label(&p.window_label)
            .map(|e| (e.parent_label.clone(), e.tool_window_id.clone()));
        assert_eq!(key, Some(("window-1".to_string(), "ssh-sessions".to_string())));
        let removed = key.and_then(|(parent, id)| r.remove(&parent, &id));
        assert_eq!(
            removed.map(|e| e.req_id),
            Some(p.req_id),
            "the live entry resolves through its own window label"
        );

        // Second abort attempt, same window_label: the lookup now finds
        // nothing, so `abort_panel_host` returns Err rather than emitting or
        // destroying a second time.
        assert!(
            r.get_by_window_label(&p.window_label).is_none(),
            "a second abort of the same window is a no-op, not a double-teardown"
        );
    }

    #[test]
    fn aborted_event_name_and_payload_shape_are_pinned() {
        assert_eq!(PANEL_HOST_ABORTED_EVENT, "panel-host-aborted");
        let payload = PanelHostToolWindowEvent {
            tool_window_id: "ssh-sessions".to_string(),
        };
        let json = serde_json::to_value(&payload).expect("serialize");
        assert_eq!(json, serde_json::json!({ "toolWindowId": "ssh-sessions" }));
    }
}

// ---------------------------------------------------------------------------
// Caller validation
// ---------------------------------------------------------------------------

/// Which window labels may call `open_panel_host` / `panel_host_broadcast`:
/// main-app windows only. Pulled out as a pure function, the same way
/// `chooser_window::validate_chooser_caller` is, so the rejection rules are
/// unit-tested directly.
fn validate_panel_host_caller(label: &str) -> Result<(), String> {
    if label.starts_with("panelhost-") {
        return Err("a panel host window cannot open another panel host".to_string());
    }
    if label.starts_with("chooser-") {
        return Err("a chooser window cannot open a panel host".to_string());
    }
    if label == "settings" {
        return Err("the settings window cannot open a panel host".to_string());
    }
    Ok(())
}

/// Which window labels may call `dock_panel_host` / `abort_panel_host`: the
/// exact opposite restriction from `validate_panel_host_caller` above — here
/// the caller MUST be a panel host, since both commands are a host's own
/// self-teardown affordances, never something a parent or another window
/// invokes on its behalf. Only the structural (label-shape) half of "must be
/// a REGISTERED panelhost-*"; the registration half is the registry lookup
/// each command layers on top of this.
fn validate_panel_host_self_caller(label: &str) -> Result<(), String> {
    if !label.starts_with("panelhost-") {
        return Err("this command must be called by a panel host window".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod caller_validation_tests {
    use super::{validate_panel_host_caller, validate_panel_host_self_caller};

    #[test]
    fn rejects_panel_host_windows() {
        assert!(validate_panel_host_caller("panelhost-window-1-3").is_err());
        assert!(validate_panel_host_caller("panelhost-main-1").is_err());
    }

    #[test]
    fn rejects_chooser_windows() {
        assert!(validate_panel_host_caller("chooser-window-1-2").is_err());
    }

    #[test]
    fn rejects_the_settings_window() {
        assert!(validate_panel_host_caller("settings").is_err());
    }

    #[test]
    fn allows_ordinary_windows() {
        assert!(validate_panel_host_caller("main").is_ok());
        assert!(validate_panel_host_caller("window-1").is_ok());
    }

    #[test]
    fn self_caller_allows_only_panel_host_windows() {
        assert!(validate_panel_host_self_caller("panelhost-window-1-3").is_ok());
        assert!(validate_panel_host_self_caller("main").is_err());
        assert!(validate_panel_host_self_caller("window-1").is_err());
        assert!(validate_panel_host_self_caller("chooser-window-1-2").is_err());
        assert!(validate_panel_host_self_caller("settings").is_err());
    }
}

// ---------------------------------------------------------------------------
// Sizing / positioning
// ---------------------------------------------------------------------------

/// Clamp a persisted (or default-floor) dimension to `[floor, monitor work
/// area]` — identical rule to `chooser_window::clamp_dimension`.
fn clamp_dimension(value: f64, floor: f64, monitor_max: Option<f64>) -> f64 {
    let floored = value.max(floor);
    match monitor_max {
        Some(max) if max >= floor => floored.min(max),
        _ => floored,
    }
}

/// Clamp a persisted position so the window's bounds stay on-screen within
/// `work_area` (logical `(x, y, width, height)`). `None` (no monitor info)
/// passes the position through unclamped, same fallback as size.
fn clamp_position(x: f64, y: f64, w: f64, h: f64, work_area: Option<(f64, f64, f64, f64)>) -> (f64, f64) {
    match work_area {
        Some((wx, wy, ww, wh)) => {
            // `.max(wx)`/`.max(wy)`: a window wider/taller than the work area
            // (the min-size floor can force that on a tiny monitor) clamps to
            // the work area's origin rather than a negative-width range.
            let max_x = (wx + ww - w).max(wx);
            let max_y = (wy + wh - h).max(wy);
            (x.clamp(wx, max_x), y.clamp(wy, max_y))
        }
        None => (x, y),
    }
}

/// The parent's current monitor work area in logical px, as
/// `(x, y, width, height)`.
fn parent_work_area_logical<R: tauri::Runtime>(
    parent: &tauri::WebviewWindow<R>,
) -> Option<(f64, f64, f64, f64)> {
    match parent.current_monitor() {
        Ok(Some(monitor)) if monitor.scale_factor() > 0.0 => {
            let scale = monitor.scale_factor();
            let work_area = monitor.work_area();
            Some((
                work_area.position.x as f64 / scale,
                work_area.position.y as f64 / scale,
                work_area.size.width as f64 / scale,
                work_area.size.height as f64 / scale,
            ))
        }
        _ => None,
    }
}

/// The host's initial position when there is no persisted bounds for this
/// tool window: centered on the parent's current bounds — same approach as
/// `chooser_window::centered_position`.
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

// ---------------------------------------------------------------------------
// Window builder
// ---------------------------------------------------------------------------

/// Build (hidden) the panel host window for `(parent_label, tool_window_id)`.
/// Must run on the main thread — see `windows.rs:42-51` for the deadlock rule
/// this follows (same as `chooser_window::create_chooser_window`).
///
/// On failure the caller is responsible for removing the registry entry: an
/// entry with no window behind it is permanently unreachable (summon would
/// find a label that names nothing).
fn create_panel_host_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    parent_label: &str,
    panel_host_label: &str,
    tool_window_id: &str,
    title: &str,
) -> Result<(), String> {
    let parent = app
        .get_webview_window(parent_label)
        .ok_or_else(|| format!("parent window '{parent_label}' not found"))?;

    let user_cfg = config::load_user_config().unwrap_or_default();
    // Theme per settings-window rules (windows.rs:78-92: `appearance_to_theme`
    // from the same user config, no owner-window relationship — a panel host
    // is an ordinary secondary window, not a modal like the chooser).
    let theme = windows::appearance_to_theme(&user_cfg.colors.appearance_mode);
    let use_custom_titlebar = cfg!(target_os = "windows") || cfg!(target_os = "linux");

    let persisted_state = config::load_persistent_state().unwrap_or_default();
    let persisted_bounds = persisted_state
        .layout
        .tool_window_bounds
        .get(tool_window_id)
        .copied();

    let work_area = parent_work_area_logical(&parent);

    let (target_w, target_h) = match persisted_bounds {
        Some(b) => (
            clamp_dimension(b.width, PANEL_HOST_MIN_WIDTH, work_area.map(|(_, _, w, _)| w)),
            clamp_dimension(b.height, PANEL_HOST_MIN_HEIGHT, work_area.map(|(_, _, _, h)| h)),
        ),
        None => (PANEL_HOST_DEFAULT_WIDTH, PANEL_HOST_DEFAULT_HEIGHT),
    };

    let position = match persisted_bounds {
        Some(b) => Some(clamp_position(b.x, b.y, target_w, target_h, work_area)),
        None => centered_position(&parent, target_w, target_h),
    };

    // `index.html`, not a dedicated host page: the boot branch (a later task)
    // distinguishes a panel host from an ordinary window at runtime via
    // `get_panel_host_request()`, exactly-label-matched — see the design
    // spec's "panel host boot" section.
    let mut builder =
        WebviewWindowBuilder::new(app, panel_host_label, WebviewUrl::App("index.html".into()))
            .title(title)
            .inner_size(target_w, target_h)
            .min_inner_size(PANEL_HOST_MIN_WIDTH, PANEL_HOST_MIN_HEIGHT)
            .resizable(true)
            .visible(false)
            .decorations(!use_custom_titlebar)
            .theme(theme);

    if let Some((x, y)) = position {
        builder = builder.position(x, y);
    }

    let win = builder.build().map_err(|e| e.to_string())?;
    let _ = win.remove_menu();

    // Same rescue as every other hidden-until-ready window (windows.rs:144,
    // chooser_window.rs:661).
    crate::arm_window_show_fallback(app, win.label());
    Ok(())
}

/// Tear down the panel host window that `open_panel_host` is replacing, by
/// its stored (request-unique) label. `destroy()`, deliberately, not
/// `close()` — see `chooser_window::destroy_displaced_chooser`, whose
/// reasoning applies verbatim: `close()` would raise `CloseRequested`, and
/// this window is already resolved (removed from the registry by `open`
/// before this runs).
fn destroy_displaced_panel_host<R: tauri::Runtime>(app: &tauri::AppHandle<R>, window_label: &str) {
    if let Some(win) = app.get_webview_window(window_label) {
        let _ = win.destroy();
    }
}

// ---------------------------------------------------------------------------
// Bounds persistence (debounced)
// ---------------------------------------------------------------------------

/// How long to wait after the last `Moved`/`Resized` event before writing —
/// a user dragging or resizing fires many of these in a row, and a state.toml
/// write per pixel would both thrash disk and (via `run_on_main_thread`-free
/// config I/O here, thread-only) contend badly. 500ms mirrors the frontend's
/// own debounce shape (`createDebouncedSaver`,
/// `frontend/app/core/layout-service.js:23-40` — cancel-and-reschedule a
/// timer on every call, default 150ms there for a cheaper in-memory write;
/// this is a disk write of the whole state file, so it uses a longer delay).
const BOUNDS_DEBOUNCE_MS: u64 = 500;

/// Per-window-label "latest event wins" generation counter. A spawned save
/// checks its captured generation against this map after sleeping; a mismatch
/// means a newer `Moved`/`Resized` superseded it, so it skips the write
/// (the "simple last-write timestamp" debounce named in task-1-brief.md —
/// there is no prior spawned-delayed-save-with-cancellation in this codebase
/// to mirror 1:1 in Rust, so this follows the same spawn-a-thread-and-sleep
/// shape as `arm_window_show_fallback`, lib.rs:79-98).
static BOUNDS_SAVE_GENERATION: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Read `win`'s current outer position / inner size in logical px and
/// load-mutate-save them into `PersistentState.layout.tool_window_bounds`
/// under `tool_window_id` — the only API `termlab_core::config` offers
/// (`config/mod.rs:253,266`, same pattern as
/// `chooser_window::persist_chooser_size`). Silently does nothing if reading
/// the window fails (a closing window can race this).
///
/// **Keyed by `tool_window_id` alone, deliberately** — see the doc comment
/// on `LayoutConfig::tool_window_bounds` (`persistent.rs`) for the full
/// trade. In short: composite `(parent_label, tool_window_id)` keys were
/// rejected because parent labels are launch-order-assigned and not stable
/// across restarts, so they would only accumulate orphaned records. The
/// consequence here is that this write is a plain last-writer-wins
/// overwrite of one shared record — if the same tool window is popped out
/// from two different main windows, saving from either one clobbers what
/// the other remembers. The live windows themselves stay fully independent;
/// only the persisted, remembered bounds are shared.
fn persist_tool_window_bounds<R: tauri::Runtime>(
    win: &tauri::WebviewWindow<R>,
    tool_window_id: &str,
) {
    let (Ok(outer_pos), Ok(inner), Ok(scale)) =
        (win.outer_position(), win.inner_size(), win.scale_factor())
    else {
        return;
    };
    if scale <= 0.0 {
        return;
    }
    let record = WindowBoundsRecord {
        x: outer_pos.x as f64 / scale,
        y: outer_pos.y as f64 / scale,
        width: inner.width as f64 / scale,
        height: inner.height as f64 / scale,
    };
    let mut state = config::load_persistent_state().unwrap_or_default();
    state
        .layout
        .tool_window_bounds
        .insert(tool_window_id.to_string(), record);
    if let Err(e) = config::save_persistent_state(&state) {
        log::warn!("failed to persist panel host bounds for '{tool_window_id}': {e}");
    }
}

/// Schedule a debounced bounds save for `window_label` (a live panel host
/// registered under `tool_window_id`). Bumps the generation counter for this
/// label and spawns a thread that sleeps `BOUNDS_DEBOUNCE_MS`, then only
/// writes if no later call has bumped the counter again in the meantime.
fn schedule_bounds_persist<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    window_label: &str,
    tool_window_id: &str,
) {
    let generation = {
        let mut generations = BOUNDS_SAVE_GENERATION.lock();
        let next = generations.get(window_label).copied().unwrap_or(0) + 1;
        generations.insert(window_label.to_string(), next);
        next
    };

    let handle = app.clone();
    let label = window_label.to_string();
    let tool_window_id = tool_window_id.to_string();
    std::thread::Builder::new()
        .name(format!("panel-host-bounds-{label}"))
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(BOUNDS_DEBOUNCE_MS));
            let still_current = BOUNDS_SAVE_GENERATION.lock().get(&label).copied() == Some(generation);
            if !still_current {
                return; // superseded by a later Moved/Resized — that one will save
            }
            let Some(win) = handle.get_webview_window(&label) else {
                return; // window already gone — nothing to save
            };
            persist_tool_window_bounds(&win, &tool_window_id);
        })
        .ok();
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Open (or, if one is already live for this `(parent, tool_window_id)`,
/// cancel-and-recreate) a panel host. Rejects callers that are themselves a
/// panel host, a chooser, or the settings window
/// (`validate_panel_host_caller`).
#[tauri::command]
pub(crate) async fn open_panel_host(
    window: tauri::WebviewWindow,
    tool_window_id: String,
    title: String,
) -> Result<u64, String> {
    let parent_label = window.label().to_string();
    validate_panel_host_caller(&parent_label)?;

    let app = window.app_handle().clone();

    let (displaced, pending) = {
        let registry = app.state::<Mutex<PanelHostRegistry>>();
        registry
            .lock()
            .open(parent_label.clone(), tool_window_id.clone(), title.clone())
    };

    // Window creation must happen on the main thread — same deadlock rule as
    // open_new_window/open_settings_window (windows.rs:42-51) and
    // open_file_chooser (chooser_window.rs:840-846). The displaced window (if
    // any) is destroyed by its own stored label and the replacement is built
    // under a fresh one, so the build never races the (always-asynchronous)
    // destroy for a label.
    let handle = app.clone();
    let build_parent_label = parent_label.clone();
    let build_label = pending.window_label.clone();
    let build_tool_window_id = tool_window_id.clone();
    let build_title = title.clone();
    let displaced_label = displaced.as_ref().map(|d| d.window_label.clone());
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    handle
        .clone()
        .run_on_main_thread(move || {
            if let Some(old_label) = displaced_label.as_deref() {
                destroy_displaced_panel_host(&handle, old_label);
            }
            let result = create_panel_host_window(
                &handle,
                &build_parent_label,
                &build_label,
                &build_tool_window_id,
                &build_title,
            );
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;
    let build_result = rx.recv().map_err(|e| e.to_string())?;

    if let Err(e) = build_result {
        let registry = app.state::<Mutex<PanelHostRegistry>>();
        registry.lock().remove(&parent_label, &tool_window_id);
        return Err(e);
    }

    Ok(pending.req_id)
}

/// The pending request for the calling (panel host) window, resolved through
/// the registry by the caller's own window label — never by parsing a parent
/// out of it. A displaced window's stale label finds nothing here.
#[tauri::command]
pub(crate) fn get_panel_host_request(
    window: tauri::WebviewWindow,
    registry: tauri::State<'_, Mutex<PanelHostRegistry>>,
) -> Result<PanelHostRequest, String> {
    let label = window.label();
    if !label.starts_with("panelhost-") {
        return Err("not a panel host window".to_string());
    }
    registry
        .lock()
        .get_by_window_label(label)
        .map(|e| PanelHostRequest {
            req_id: e.req_id,
            tool_window_id: e.tool_window_id.clone(),
            parent_label: e.parent_label.clone(),
            title: e.title.clone(),
        })
        .ok_or_else(|| "no pending panel host request".to_string())
}

/// Show and focus the panel host window once its frontend reports first
/// paint (mirror of `chooser_window::chooser_ready` / `commands::app_ready`),
/// and mark the registry entry visible.
#[tauri::command]
pub(crate) fn panel_host_ready(
    window: tauri::WebviewWindow,
    registry: tauri::State<'_, Mutex<PanelHostRegistry>>,
) -> Result<(), String> {
    let label = window.label().to_string();
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;

    let key = registry
        .lock()
        .get_by_window_label(&label)
        .map(|e| (e.parent_label.clone(), e.tool_window_id.clone()));
    if let Some((parent, id)) = key {
        registry.lock().set_visible(&parent, &id, true);
    }
    Ok(())
}

/// Summon the caller's (parent's) panel host for `tool_window_id`: show +
/// focus if a live entry exists, else `Err("no host")` so the frontend falls
/// back to `open_panel_host` (task-1-brief.md's summon path).
#[tauri::command]
pub(crate) fn focus_panel_host(
    window: tauri::WebviewWindow,
    tool_window_id: String,
    registry: tauri::State<'_, Mutex<PanelHostRegistry>>,
) -> Result<(), String> {
    let parent_label = window.label().to_string();
    let window_label = registry
        .lock()
        .get(&parent_label, &tool_window_id)
        .map(|e| e.window_label.clone());
    let Some(window_label) = window_label else {
        return Err("no host".to_string());
    };

    let app = window.app_handle();
    let Some(host_win) = app.get_webview_window(&window_label) else {
        // Stale entry pointing at a window that is already gone — clean it up
        // rather than leaving a summon target that can never succeed.
        registry.lock().remove(&parent_label, &tool_window_id);
        return Err("no host".to_string());
    };
    let _ = host_win.show();
    let _ = host_win.set_focus();
    registry
        .lock()
        .set_visible(&parent_label, &tool_window_id, true);
    let _ = app.emit_to(
        parent_label.as_str(),
        PANEL_HOST_SHOWN_EVENT,
        &PanelHostToolWindowEvent { tool_window_id },
    );
    Ok(())
}

/// Hide the caller's (parent's) panel host for `tool_window_id`. The window
/// and its registry entry both survive — this is the command-driven twin of
/// the `CloseRequested` hide path below.
#[tauri::command]
pub(crate) fn hide_panel_host(
    window: tauri::WebviewWindow,
    tool_window_id: String,
    registry: tauri::State<'_, Mutex<PanelHostRegistry>>,
) -> Result<(), String> {
    let parent_label = window.label().to_string();
    let window_label = registry
        .lock()
        .get(&parent_label, &tool_window_id)
        .map(|e| e.window_label.clone());
    let Some(window_label) = window_label else {
        return Ok(()); // nothing live to hide
    };

    if let Some(host_win) = window.app_handle().get_webview_window(&window_label) {
        let _ = host_win.hide();
    }
    registry
        .lock()
        .set_visible(&parent_label, &tool_window_id, false);
    let _ = window.emit_to(
        parent_label.as_str(),
        PANEL_HOST_HIDDEN_EVENT,
        &PanelHostToolWindowEvent { tool_window_id },
    );
    Ok(())
}

/// Dock a panel host back into its parent's zone: called by the HOST window
/// itself (its own dock-back affordance), not the parent. Emits
/// `panel-host-docked` to the parent, removes the registry entry, then
/// DESTROYS (not closes — this window's own teardown, no CloseRequested
/// re-entry) the host window.
#[tauri::command]
pub(crate) fn dock_panel_host(
    window: tauri::WebviewWindow,
    tool_window_id: String,
) -> Result<(), String> {
    let caller_label = window.label().to_string();
    if !caller_label.starts_with("panelhost-") {
        return Err("dock_panel_host must be called by a panel host window".to_string());
    }

    let app = window.app_handle();
    let entry = {
        let registry = app.state::<Mutex<PanelHostRegistry>>();
        let guard = registry.lock();
        guard.get_by_window_label(&caller_label).cloned()
    };
    let Some(entry) = entry else {
        return Err("no panel host entry for this window".to_string());
    };
    if entry.tool_window_id != tool_window_id {
        return Err("tool_window_id does not match this panel host".to_string());
    }

    let _ = app.emit_to(
        entry.parent_label.as_str(),
        PANEL_HOST_DOCKED_EVENT,
        &PanelHostToolWindowEvent {
            tool_window_id: entry.tool_window_id.clone(),
        },
    );

    {
        let registry = app.state::<Mutex<PanelHostRegistry>>();
        registry
            .lock()
            .remove(&entry.parent_label, &entry.tool_window_id);
    }

    let _ = window.destroy();
    Ok(())
}

/// Self-close for a panel host that has nothing to host: the boot's
/// unknown-tool-window-id path. `open_panel_host` always registers an entry
/// before the window exists, so a plain `window.close()` from inside a host
/// whose id matched no panel would otherwise hit
/// `on_panel_host_close_requested`'s REGISTERED branch — intercepted and
/// hidden forever, not actually closed — and `dock_panel_host` is the wrong
/// tool too: it emits `panel-host-docked`, which tells the manager a panel
/// is coming back to remount, when in fact none was ever mounted here.
///
/// Callable only by the host window itself, and only while it is still
/// registered (`validate_panel_host_self_caller` plus the exact-label
/// registry lookup below — together, "caller label must be a registered
/// panelhost-*"). Removes the entry FIRST (remove-before-destroy: the same
/// law `dock_panel_host` and every other teardown path in this module
/// follow, which is what makes a second call see nothing and no-op rather
/// than double-emit or double-destroy), then emits `panel-host-aborted` to
/// the parent, then DESTROYS (not closes — this window's own teardown, no
/// `CloseRequested` re-entry) the host window.
#[tauri::command]
pub(crate) fn abort_panel_host(window: tauri::WebviewWindow) -> Result<(), String> {
    let caller_label = window.label().to_string();
    validate_panel_host_self_caller(&caller_label)?;

    let app = window.app_handle();
    let entry = {
        let registry = app.state::<Mutex<PanelHostRegistry>>();
        let guard = registry.lock();
        guard.get_by_window_label(&caller_label).cloned()
    };
    let Some(entry) = entry else {
        return Err("no panel host entry for this window".to_string());
    };

    {
        let registry = app.state::<Mutex<PanelHostRegistry>>();
        registry
            .lock()
            .remove(&entry.parent_label, &entry.tool_window_id);
    }

    let _ = app.emit_to(
        entry.parent_label.as_str(),
        PANEL_HOST_ABORTED_EVENT,
        &PanelHostToolWindowEvent {
            tool_window_id: entry.tool_window_id.clone(),
        },
    );

    let _ = window.destroy();
    Ok(())
}

/// Re-dispatch a parent-window-state event to every live host of the calling
/// parent, as `panel-host-event { event, payload }`. Parent-callers only —
/// same validation as `open_panel_host`.
#[tauri::command]
pub(crate) fn panel_host_broadcast(
    window: tauri::WebviewWindow,
    event: String,
    payload: serde_json::Value,
) -> Result<(), String> {
    let parent_label = window.label().to_string();
    validate_panel_host_caller(&parent_label)?;

    let app = window.app_handle();
    let hosts = {
        let registry = app.state::<Mutex<PanelHostRegistry>>();
        registry.lock().hosts_of_parent(&parent_label)
    };
    let out = PanelHostBroadcastEvent { event, payload };
    for host in hosts {
        let _ = app.emit_to(host.window_label.as_str(), PANEL_HOST_BROADCAST_EVENT, &out);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle hooks — called from lib.rs's `on_window_event`
// ---------------------------------------------------------------------------

/// `WindowEvent::CloseRequested` on a `panelhost-*` window.
///
/// Returns `true` when the caller must call `api.prevent_close()` (mirrors
/// the `close_guard::on_close_requested` return-a-bool-and-let-the-caller-
/// call-prevent_close shape already used in lib.rs's `on_window_event`).
///
/// A REGISTERED host (an entry exists) is the persistent case — the one
/// deliberate divergence from the chooser: prevent the close, hide the
/// window instead, mark the registry entry hidden (it survives), and emit
/// `panel-host-hidden` so the parent's rail icon can un-highlight. An
/// UNREGISTERED `panelhost-*` window (no entry) is allowed to close
/// normally — this returns `false` and does nothing else. That is the boot's
/// unknown-tool-window-id self-close path: a host whose id matched nothing
/// mounts no panel and closes itself before ever calling
/// `get_panel_host_request` successfully, i.e. before any entry exists for
/// its label to begin with, so treating "no entry" as "let it close" is
/// exactly what that path needs — no special-casing required here.
pub(crate) fn on_panel_host_close_requested<R: tauri::Runtime>(window: &tauri::Window<R>) -> bool {
    let label = window.label();
    if !label.starts_with("panelhost-") {
        return false;
    }
    let app = window.app_handle();
    let Some(registry) = app.try_state::<Mutex<PanelHostRegistry>>() else {
        return false;
    };

    let key = {
        let guard = registry.lock();
        guard
            .get_by_window_label(label)
            .map(|e| (e.parent_label.clone(), e.tool_window_id.clone()))
    };
    let Some((parent_label, tool_window_id)) = key else {
        return false; // unregistered — let the close proceed
    };

    registry
        .lock()
        .set_visible(&parent_label, &tool_window_id, false);
    let _ = window.hide();
    let _ = window.emit_to(
        parent_label.as_str(),
        PANEL_HOST_HIDDEN_EVENT,
        &PanelHostToolWindowEvent { tool_window_id },
    );
    true
}

/// `WindowEvent::Destroyed`: two unrelated cases, told apart by which side of
/// the parent/host relationship the destroyed window was on — same shape as
/// `chooser_window::on_window_destroyed`.
///
/// 1. The destroyed window was some panel host's PARENT: destroy every live
///    host of that parent and drain their registry entries (teardown paths
///    destroy, never close — destroying skips `CloseRequested`, which would
///    otherwise re-hide-and-survive a window that is supposed to be gone for
///    good).
/// 2. The destroyed window WAS a panel host (`panelhost-*`) that still had a
///    registry entry — an OS-level kill or crash that bypassed
///    `CloseRequested` entirely. Drop the stale entry so `focus_panel_host`
///    and `panel_host_broadcast` never target a dead window label again.
///    Re-entry-safe by the registry-removal-first law: every ordinary
///    teardown (dock-back, displacement, the parent-drain in branch 1) always
///    removes the entry BEFORE destroying the window, so by the time
///    `Destroyed` reaches this branch for one of those the entry is already
///    gone and this is a no-op.
pub(crate) fn on_window_destroyed<R: tauri::Runtime>(window: &tauri::Window<R>) {
    let app = window.app_handle();
    let label = window.label().to_string();
    let Some(registry) = app.try_state::<Mutex<PanelHostRegistry>>() else {
        return;
    };

    let orphaned_hosts = registry.lock().drain_parent(&label);
    for entry in orphaned_hosts {
        if let Some(win) = app.get_webview_window(&entry.window_label) {
            let _ = win.destroy();
        }
    }

    if label.starts_with("panelhost-") {
        let removed = {
            let mut guard = registry.lock();
            let key = guard
                .get_by_window_label(&label)
                .map(|e| (e.parent_label.clone(), e.tool_window_id.clone()));
            key.and_then(|(parent, id)| guard.remove(&parent, &id))
        };
        let _ = removed; // window is already gone; nothing further to do
    }
}

/// `WindowEvent::Moved` / `WindowEvent::Resized` on a `panelhost-*` window:
/// schedule a debounced bounds save for its `tool_window_id`. A no-op for any
/// other window, and for a `panelhost-*` window with no registry entry
/// (mid-build, or already torn down — nothing meaningful to key the save by).
pub(crate) fn on_panel_host_bounds_changed<R: tauri::Runtime>(window: &tauri::Window<R>) {
    let label = window.label();
    if !label.starts_with("panelhost-") {
        return;
    }
    let app = window.app_handle();
    let Some(registry) = app.try_state::<Mutex<PanelHostRegistry>>() else {
        return;
    };
    let tool_window_id = registry
        .lock()
        .get_by_window_label(label)
        .map(|e| e.tool_window_id.clone());
    let Some(tool_window_id) = tool_window_id else {
        return;
    };
    schedule_bounds_persist(app, label, &tool_window_id);
}
