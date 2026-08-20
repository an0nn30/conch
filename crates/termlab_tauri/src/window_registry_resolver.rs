//! Shared caller-window → session-owning-window resolution, over BOTH of the
//! secondary-window registries (`chooser_window::ChooserRegistry` and
//! `panel_host::PanelHostRegistry`).
//!
//! A window-scoped session command (SFTP today; anything else that keys
//! behavior off `window.label()`) must not trust the calling window's own
//! label at face value: a chooser window and a popped-out panel host window
//! both act ON BEHALF OF a parent main window, and sessions in `RemoteState`
//! are keyed by the PARENT's label, not the secondary window's own. Each
//! registry already exposes a pure, Tauri-handle-free
//! `session_label_for_caller` method that does this mapping for its own
//! window family (identity for anything it does not recognize, including a
//! stale label of its own prefix whose entry is gone); this module is only
//! the composition of the two, plus the one AppHandle-level lookup every
//! call site actually needs.
//!
//! A caller label matches at most one registry's prefix (`chooser-*` vs
//! `panelhost-*` are disjoint, and a plain main-window label matches
//! neither), so the two `session_label_for_caller` calls below never
//! disagree in practice — but the composition still runs chooser first, then
//! panel-host, exactly as specified.

use parking_lot::Mutex;
use tauri::Manager;

use crate::chooser_window::ChooserRegistry;
use crate::panel_host::PanelHostRegistry;

/// The pure composition: chooser registry first, then panel-host registry,
/// else identity. Either registry argument may be absent (no managed state,
/// or simply not consulted) — an absent registry behaves exactly like one
/// with no matching entry, i.e. it defers to the next step.
fn resolve_caller_label(
    chooser: Option<&ChooserRegistry>,
    panel_host: Option<&PanelHostRegistry>,
    caller_label: &str,
) -> String {
    if let Some(chooser) = chooser {
        let resolved = chooser.session_label_for_caller(caller_label);
        if resolved != caller_label {
            return resolved;
        }
    }
    if let Some(panel_host) = panel_host {
        let resolved = panel_host.session_label_for_caller(caller_label);
        if resolved != caller_label {
            return resolved;
        }
    }
    caller_label.to_string()
}

/// The window label whose sessions `caller_label` may use, resolved through
/// whichever secondary-window registry (if any) recognizes it — the single
/// entry point every session-scoped command layer (the SFTP commands today)
/// calls instead of trusting `window.label()` directly.
///
/// `try_state` rather than `state`: this must not panic if only one registry
/// (or neither, e.g. a minimal test harness) is managed — an unmanaged
/// registry is simply not consulted, same as one with no matching entry.
pub(crate) fn effective_session_window_label<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    caller_label: &str,
) -> String {
    let chooser_state = app.try_state::<Mutex<ChooserRegistry>>();
    let panel_host_state = app.try_state::<Mutex<PanelHostRegistry>>();
    let chooser_guard = chooser_state.as_ref().map(|s| s.lock());
    let panel_host_guard = panel_host_state.as_ref().map(|s| s.lock());
    resolve_caller_label(
        chooser_guard.as_deref(),
        panel_host_guard.as_deref(),
        caller_label,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, otherwise-unused `ChooserRequest` — every field but the
    /// mode is irrelevant to the registry-level matrix below, which only
    /// exercises `open`/`session_label_for_caller`.
    fn chooser_request() -> crate::chooser_window::ChooserRequest {
        crate::chooser_window::ChooserRequest {
            req_id: 0,
            mode: "open".to_string(),
            filename: None,
            select_filename: false,
            parent_label: String::new(),
        }
    }

    #[test]
    fn chooser_caller_resolves_to_its_parent() {
        let mut chooser = ChooserRegistry::default();
        let pending = chooser.open("window-1".into(), chooser_request());
        assert_eq!(
            resolve_caller_label(Some(&chooser), None, &pending.window_label),
            "window-1"
        );
    }

    #[test]
    fn panel_host_caller_resolves_to_its_parent() {
        let mut panel_host = PanelHostRegistry::default();
        let (_, entry) = panel_host.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        assert_eq!(
            resolve_caller_label(None, Some(&panel_host), &entry.window_label),
            "window-1"
        );
    }

    #[test]
    fn a_plain_main_window_label_resolves_to_itself() {
        let chooser = ChooserRegistry::default();
        let panel_host = PanelHostRegistry::default();
        assert_eq!(
            resolve_caller_label(Some(&chooser), Some(&panel_host), "main"),
            "main"
        );
        // And with neither registry available at all (e.g. unmanaged state).
        assert_eq!(resolve_caller_label(None, None, "main"), "main");
    }

    #[test]
    fn a_stale_chooser_label_resolves_to_itself_not_the_panel_hosts_answer() {
        let mut chooser = ChooserRegistry::default();
        let pending = chooser.open("window-1".into(), chooser_request());
        let stale = pending.window_label.clone();
        chooser.take_pending("window-1");

        let panel_host = PanelHostRegistry::default();
        assert_eq!(
            resolve_caller_label(Some(&chooser), Some(&panel_host), &stale),
            stale,
            "a stale chooser-* label must not be treated as an unrecognized \
             label the panel-host registry gets a turn at — it stays itself"
        );
    }

    #[test]
    fn a_stale_panel_host_label_resolves_to_itself() {
        let mut panel_host = PanelHostRegistry::default();
        let (_, entry) = panel_host.open("window-1".into(), "ssh-sessions".into(), "SSH".into());
        let stale = entry.window_label.clone();
        panel_host.remove("window-1", "ssh-sessions");

        let chooser = ChooserRegistry::default();
        assert_eq!(
            resolve_caller_label(Some(&chooser), Some(&panel_host), &stale),
            stale
        );
    }

    #[test]
    fn a_live_panel_host_caller_is_untouched_by_an_unrelated_live_chooser() {
        // Both registries populated at once, with UNRELATED live entries —
        // the panel-host caller's own label must resolve through the
        // panel-host registry, not get shadowed or confused by the chooser
        // registry having entries of its own.
        let mut chooser = ChooserRegistry::default();
        chooser.open("chooser-parent".into(), chooser_request());

        let mut panel_host = PanelHostRegistry::default();
        let (_, entry) =
            panel_host.open("panel-host-parent".into(), "ssh-sessions".into(), "SSH".into());

        assert_eq!(
            resolve_caller_label(Some(&chooser), Some(&panel_host), &entry.window_label),
            "panel-host-parent"
        );
    }

    // ------------------------------------------------------------------
    // AppHandle-level smoke test
    // ------------------------------------------------------------------
    //
    // `tauri::test::mock_app()` (the `test` feature, dev-dependency only)
    // builds a real `App<MockRuntime>` with no webview windows — exactly the
    // "managed state without a full app" the brief asks for. This exercises
    // the actual `state`/`try_state` plumbing `effective_session_window_label`
    // uses, on top of the pure-function matrix above.
    #[test]
    fn app_handle_smoke_resolves_through_managed_registries() {
        let app = tauri::test::mock_app();
        app.manage(Mutex::new(ChooserRegistry::default()));
        app.manage(Mutex::new(PanelHostRegistry::default()));

        let handle = app.handle();

        let chooser_label = {
            let state = handle.state::<Mutex<ChooserRegistry>>();
            let mut guard = state.lock();
            guard.open("window-1".into(), chooser_request()).window_label
        };
        assert_eq!(
            effective_session_window_label(handle, &chooser_label),
            "window-1"
        );

        let panel_host_label = {
            let state = handle.state::<Mutex<PanelHostRegistry>>();
            let mut guard = state.lock();
            guard
                .open("window-2".into(), "ssh-sessions".into(), "SSH".into())
                .1
                .window_label
        };
        assert_eq!(
            effective_session_window_label(handle, &panel_host_label),
            "window-2"
        );

        assert_eq!(effective_session_window_label(handle, "window-3"), "window-3");
    }

    #[test]
    fn app_handle_smoke_falls_back_to_identity_with_no_managed_registries() {
        // Neither registry managed at all — try_state must return None, not
        // panic, and the resolver still answers identity.
        let app = tauri::test::mock_app();
        let handle = app.handle();
        assert_eq!(effective_session_window_label(handle, "main"), "main");
    }
}
