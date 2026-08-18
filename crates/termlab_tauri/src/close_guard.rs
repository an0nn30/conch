//! Unsaved-changes handshake between the Rust side and the webview.
//!
//! The webview owns the answer to "is anything unsaved?" — `dirty` lives on
//! CodeMirror panes, not here — but the events that destroy that state
//! (`WindowEvent::CloseRequested`, the Quit menu item) arrive on the Rust
//! side. So Rust stops the close, asks, and acts on the answer.
//!
//! Three rules keep the handshake from wedging:
//!
//! 1. **Armed windows only.** A window is guarded only after its frontend
//!    calls [`window_close_guard_arm`], which it does once its listener is
//!    installed. A window that never gets that far — the settings window,
//!    which loads a different document, or a main window whose scripts failed
//!    — keeps the ordinary close behaviour instead of becoming unclosable.
//!    Such a window has no editors, so there is nothing to lose.
//!
//! 2. **Permission is consumed.** A confirmation authorises exactly one
//!    close. The second `CloseRequested` (the one raised by the frontend's own
//!    `window.close()`) spends it and passes through, so the prompt cannot
//!    loop; anything after that asks again.
//!
//! 3. **Quit is a poll, not a broadcast.** Quit asks one window at a time and
//!    only exits once every armed window has consented, so a second window's
//!    unsaved buffer is not destroyed by a quit the user started in the first.
//!    A single "no" abandons the whole quit and leaves every window standing.

use std::collections::{HashSet, VecDeque};

use parking_lot::Mutex;
use tauri::{Emitter, Manager};

/// Emitted to a window whose close was just prevented. The frontend answers
/// with [`confirm_window_close`].
pub(crate) const WINDOW_CLOSE_REQUESTED_EVENT: &str = "window-close-requested";
/// Emitted to one window at a time during a quit. The frontend answers with
/// [`quit_vote`].
pub(crate) const APP_QUIT_REQUESTED_EVENT: &str = "app-quit-requested";

#[derive(Default)]
struct GuardState {
    /// Windows whose frontend installed the close listener.
    armed: HashSet<String>,
    /// Windows cleared for exactly one close.
    confirmed: HashSet<String>,
    /// Windows still to be asked during a quit. `None` means no quit is in
    /// progress — setting it back to `None` is how a "no" vote cancels one.
    quit_queue: Option<VecDeque<String>>,
    /// The window a quit request was last emitted to, so a stale vote from
    /// some other window cannot advance the poll.
    quit_asking: Option<String>,
}

#[derive(Default)]
pub(crate) struct CloseGuard(Mutex<GuardState>);

impl CloseGuard {
    fn arm(&self, label: &str) {
        self.0.lock().armed.insert(label.to_string());
    }

    fn confirm(&self, label: &str) {
        self.0.lock().confirmed.insert(label.to_string());
    }

    /// Whether this close may proceed, consuming the permission if one is
    /// held. An unarmed window is always allowed through (rule 1).
    fn take_permission(&self, label: &str) -> bool {
        let mut state = self.0.lock();
        if state.confirmed.remove(label) {
            return true;
        }
        !state.armed.contains(label)
    }

    /// Drop everything remembered about a destroyed window, so a label can
    /// never carry stale permission into a later window. Returns true if that
    /// window was the one a quit was waiting on.
    fn forget(&self, label: &str) -> bool {
        let mut state = self.0.lock();
        state.armed.remove(label);
        state.confirmed.remove(label);
        if state.quit_asking.as_deref() == Some(label) {
            state.quit_asking = None;
            return state.quit_queue.is_some();
        }
        false
    }

    fn cancel_quit(&self) {
        let mut state = self.0.lock();
        state.quit_queue = None;
        state.quit_asking = None;
    }
}

/// Called by the frontend once its `window-close-requested` listener is live.
#[tauri::command]
pub(crate) fn window_close_guard_arm(window: tauri::WebviewWindow, guard: tauri::State<'_, CloseGuard>) {
    guard.arm(window.label());
}

/// The frontend's answer to `window-close-requested`. `allow: false` needs no
/// action — the close was already prevented — but is still sent so the
/// handshake has an explicit end.
#[tauri::command]
pub(crate) fn confirm_window_close(
    window: tauri::WebviewWindow,
    guard: tauri::State<'_, CloseGuard>,
    allow: bool,
) {
    if !allow {
        return;
    }
    guard.confirm(window.label());
    let _ = window.close();
}

/// The frontend's answer to `app-quit-requested`.
#[tauri::command]
pub(crate) fn quit_vote(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    allow: bool,
) {
    {
        let guard = app.state::<CloseGuard>();
        let state = guard.0.lock();
        // Ignore a vote from a window we are not currently asking: a late
        // answer to an abandoned quit must not restart or advance a new one.
        if state.quit_asking.as_deref() != Some(window.label()) {
            return;
        }
    }
    if allow {
        advance_quit(&app);
    } else {
        app.state::<CloseGuard>().cancel_quit();
    }
}

/// Handle `WindowEvent::Destroyed`. Forgets the label, and keeps a quit
/// moving if the window it was waiting on went away without voting —
/// otherwise the poll would stall and Cmd+Q would silently stop working for
/// the rest of the session.
pub(crate) fn on_window_destroyed<R: tauri::Runtime>(window: &tauri::Window<R>) {
    let app = window.app_handle();
    let Some(guard) = app.try_state::<CloseGuard>() else {
        return;
    };
    if guard.forget(window.label()) {
        advance_quit(app);
    }
}

/// Handle `WindowEvent::CloseRequested`. Returns `true` when the caller should
/// prevent the close; the request has then been emitted to the window.
pub(crate) fn on_close_requested<R: tauri::Runtime>(window: &tauri::Window<R>) -> bool {
    let label = window.label().to_string();
    let app = window.app_handle();
    // No guard state managed (should not happen) means no way to ask, and a
    // window that cannot be closed is worse than one that closes.
    let Some(guard) = app.try_state::<CloseGuard>() else {
        return false;
    };
    if guard.take_permission(&label) {
        return false;
    }
    if app.emit_to(&label, WINDOW_CLOSE_REQUESTED_EVENT, ()).is_err() {
        // The window cannot be reached, so it can never answer. Let it close
        // rather than stranding it.
        return false;
    }
    true
}

/// Start a quit. Asks each armed window in turn; exits once all have
/// consented. A quit already in progress is left alone, so hammering Cmd+Q
/// does not stack prompts.
pub(crate) fn request_quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let labels = quit_order(app);
    {
        let Some(guard) = app.try_state::<CloseGuard>() else {
            app.exit(0);
            return;
        };
        let mut state = guard.0.lock();
        if state.quit_queue.is_some() {
            return;
        }
        state.quit_queue = Some(VecDeque::from(labels));
    }
    advance_quit(app);
}

/// Window labels to poll, the focused one first so the user is asked about
/// what they are actually looking at before anything else.
fn quit_order<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<String> {
    let mut focused = None;
    let mut rest = Vec::new();
    for (label, window) in app.webview_windows() {
        if focused.is_none() && window.is_focused().unwrap_or(false) {
            focused = Some(label);
        } else {
            rest.push(label);
        }
    }
    rest.sort();
    let mut labels = Vec::with_capacity(rest.len() + 1);
    labels.extend(focused);
    labels.extend(rest);
    labels
}

fn advance_quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    loop {
        let next = {
            let Some(guard) = app.try_state::<CloseGuard>() else {
                app.exit(0);
                return;
            };
            let mut lock = guard.0.lock();
            let state = &mut *lock;
            let Some(queue) = state.quit_queue.as_mut() else {
                return; // cancelled while we were asking
            };
            match queue.pop_front() {
                // Unarmed windows have no frontend to ask and no editors to
                // lose; skip straight past them.
                Some(label) if state.armed.contains(&label) => {
                    state.quit_asking = Some(label.clone());
                    Some(label)
                }
                Some(_) => continue,
                None => None,
            }
        };

        let Some(label) = next else {
            // Everyone consented.
            if let Some(guard) = app.try_state::<CloseGuard>() {
                guard.cancel_quit();
            }
            let _ = crate::editor_fs::editor_temp_sweep();
            app.exit(0);
            return;
        };

        if app.emit_to(&label, APP_QUIT_REQUESTED_EVENT, ()).is_ok() {
            return; // wait for quit_vote
        }
        // The window is gone or unreachable; it cannot be holding an editor
        // we can still save. Move on.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unarmed_window_closes_without_asking() {
        let guard = CloseGuard::default();
        assert!(guard.take_permission("settings"));
    }

    #[test]
    fn armed_window_is_refused_until_confirmed() {
        let guard = CloseGuard::default();
        guard.arm("main");
        assert!(!guard.take_permission("main"), "first attempt must be stopped");
        guard.confirm("main");
        assert!(guard.take_permission("main"), "the confirmed close goes through");
        assert!(
            !guard.take_permission("main"),
            "permission is spent, so a later close asks again"
        );
    }

    #[test]
    fn forget_clears_permission_for_the_label() {
        let guard = CloseGuard::default();
        guard.arm("window-1");
        guard.confirm("window-1");
        assert!(!guard.forget("window-1"), "no quit was in progress");
        // Re-armed under the same label: the old confirmation must not carry
        // over into the new window's first close.
        guard.arm("window-1");
        assert!(!guard.take_permission("window-1"));
    }

    #[test]
    fn losing_the_window_a_quit_is_waiting_on_asks_for_a_nudge() {
        let guard = CloseGuard::default();
        guard.arm("main");
        {
            let mut state = guard.0.lock();
            state.quit_queue = Some(VecDeque::from(vec!["window-1".to_string()]));
            state.quit_asking = Some("main".to_string());
        }
        assert!(
            guard.forget("main"),
            "the caller must be told to advance, or the quit stalls forever"
        );
        assert!(!guard.forget("window-1"), "and only for the window being asked");
    }

    #[test]
    fn cancel_quit_clears_the_poll() {
        let guard = CloseGuard::default();
        guard.0.lock().quit_queue = Some(VecDeque::from(vec!["main".to_string()]));
        guard.0.lock().quit_asking = Some("main".to_string());
        guard.cancel_quit();
        assert!(guard.0.lock().quit_queue.is_none());
        assert!(guard.0.lock().quit_asking.is_none());
    }
}
