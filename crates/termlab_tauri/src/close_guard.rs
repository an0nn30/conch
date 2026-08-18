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
//!    Such a window has no editors, so there is nothing to lose. This is the
//!    only structural escape from a wedged handshake on the Rust side — there
//!    is no "did the webview receive it?" check to be had, since `emit_to`
//!    returns `Ok` whether or not anything matched.
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

/// What the poll is for. Both kinds tear the app down over whatever is on
/// screen, so both have to ask first; only the final step differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum ExitKind {
    #[default]
    Quit,
    /// The updater's "Restart Now". `AppHandle::request_restart` sends
    /// `Message::RequestExit`, which becomes `RunEvent::ExitRequested` — and
    /// with no RunEvent callback on the builder, nothing prevents it and no
    /// window ever sees `CloseRequested`. It has to be polled like a quit.
    Restart,
}

#[derive(Default)]
struct GuardState {
    /// Windows whose frontend installed the close listener.
    armed: HashSet<String>,
    /// Windows cleared for exactly one close.
    confirmed: HashSet<String>,
    /// Windows still to be asked during a quit. `None` means no quit is in
    /// progress — setting it back to `None` is how a "no" vote cancels one.
    quit_queue: Option<VecDeque<String>>,
    /// What to do once the queue drains. Only meaningful while `quit_queue`
    /// is `Some`.
    quit_kind: ExitKind,
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

    /// Start a poll over `labels`. Returns false if one is already running, so
    /// a repeated Cmd+Q (or a Restart clicked during a quit) does not restart
    /// the questioning or stack prompts.
    fn begin_exit(&self, labels: Vec<String>, kind: ExitKind) -> bool {
        let mut state = self.0.lock();
        if state.quit_queue.is_some() {
            return false;
        }
        state.quit_queue = Some(VecDeque::from(labels));
        state.quit_kind = kind;
        state.quit_asking = None;
        true
    }

    /// The next window that still owes an answer, skipping unarmed ones (they
    /// have no frontend to ask and no editors to lose). `None` means either
    /// the queue has drained or no poll is running — [`take_finished_exit`]
    /// tells those two apart.
    fn next_to_ask(&self) -> Option<String> {
        let mut lock = self.0.lock();
        let state = &mut *lock;
        let queue = state.quit_queue.as_mut()?;
        while let Some(label) = queue.pop_front() {
            if state.armed.contains(&label) {
                return Some(label);
            }
        }
        None
    }

    fn record_asking(&self, label: &str) {
        self.0.lock().quit_asking = Some(label.to_string());
    }

    /// The action to take now that every window has consented — and the only
    /// way to get one. `None` while any window still owes an answer, and
    /// `None` when no poll is running at all, so neither a cancelled poll nor
    /// a half-finished one can reach an exit.
    fn take_finished_exit(&self) -> Option<ExitKind> {
        let mut state = self.0.lock();
        let drained = state.quit_queue.as_ref().is_some_and(|q| q.is_empty());
        if !drained {
            return None;
        }
        state.quit_queue = None;
        state.quit_asking = None;
        Some(state.quit_kind)
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
    // No "did the emit reach anyone?" check here, because there is no honest
    // one to make: `emit_to` serialises the payload and iterates the currently
    // registered webviews, so a label with no webview left simply matches
    // nothing and still returns `Ok`. The window whose event this is plainly
    // exists anyway. Arming (see the module docs) is what keeps a window that
    // cannot answer from being stopped in the first place.
    if let Err(error) = app.emit_to(&label, WINDOW_CLOSE_REQUESTED_EVENT, ()) {
        log::error!("close guard: could not ask window '{label}' about unsaved changes: {error}");
    }
    true
}

/// Start a quit. Asks each armed window in turn; exits once all have
/// consented. A quit already in progress is left alone, so hammering Cmd+Q
/// does not stack prompts.
pub(crate) fn request_quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    request_exit(app, ExitKind::Quit);
}

/// Start a restart. Same poll as a quit — a restart destroys unsaved buffers
/// exactly as thoroughly as a quit does, and the user answered "apply the
/// update?", not "discard your work?". Guarding here rather than at the
/// updater's toast means every caller of `restart_app` is covered, not just
/// that one button.
pub(crate) fn request_restart<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    request_exit(app, ExitKind::Restart);
}

fn request_exit<R: tauri::Runtime>(app: &tauri::AppHandle<R>, kind: ExitKind) {
    let labels = quit_order(app);
    let Some(guard) = app.try_state::<CloseGuard>() else {
        // No guard state managed at all (should not happen): there is nothing
        // to ask with, so do what the caller asked.
        finish_exit(app, kind);
        return;
    };
    if !guard.begin_exit(labels, kind) {
        return; // a poll is already running
    }
    advance_quit(app);
}

fn finish_exit<R: tauri::Runtime>(app: &tauri::AppHandle<R>, kind: ExitKind) {
    let _ = crate::editor_fs::editor_temp_sweep();
    match kind {
        ExitKind::Quit => app.exit(0),
        ExitKind::Restart => app.request_restart(),
    }
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

/// Drives the poll: ask the next window that owes an answer, or — only once
/// none do — carry out the exit. The decisions live on `CloseGuard` (and are
/// unit-tested there); this is just the part that needs an app handle.
fn advance_quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(guard) = app.try_state::<CloseGuard>() else {
        return;
    };
    loop {
        let Some(label) = guard.next_to_ask() else {
            // Either everyone consented, or the poll was cancelled while we
            // were asking. take_finished_exit is what tells those apart, and
            // it is the only source of an exit action.
            if let Some(kind) = guard.take_finished_exit() {
                finish_exit(app, kind);
            }
            return;
        };

        // The queue is built once, before any prompting starts, so a window in
        // it may have been closed while an earlier one was still being asked.
        // This is the check that actually works: `emit_to` returns `Ok` for a
        // label whose webview is gone, because its filter simply matches
        // nothing, so an emit result would never catch this.
        if app.get_webview_window(&label).is_none() {
            continue;
        }

        guard.record_asking(&label);
        let _ = app.emit_to(&label, APP_QUIT_REQUESTED_EVENT, ());
        return; // wait for quit_vote
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

    /// The whole poll, as `advance_quit` drives it: ask until nobody owes an
    /// answer, then take the exit. Returns the labels asked, in order, and the
    /// action that was reached (if any).
    fn drive_to_completion(guard: &CloseGuard, answer: bool) -> (Vec<String>, Option<ExitKind>) {
        let mut asked = Vec::new();
        loop {
            match guard.next_to_ask() {
                Some(label) => {
                    guard.record_asking(&label);
                    asked.push(label);
                    if !answer {
                        guard.cancel_quit();
                        return (asked, guard.take_finished_exit());
                    }
                }
                None => return (asked, guard.take_finished_exit()),
            }
        }
    }

    #[test]
    fn a_restart_asks_every_armed_window_before_restarting() {
        // The updater's "Restart Now" tears down unsaved buffers exactly as a
        // quit does, so it goes through the same poll.
        let guard = CloseGuard::default();
        guard.arm("main");
        guard.arm("window-1");
        assert!(guard.begin_exit(
            vec!["main".into(), "window-1".into()],
            ExitKind::Restart,
        ));

        // Nothing may be reachable while a window still owes an answer.
        assert_eq!(guard.next_to_ask().as_deref(), Some("main"));
        assert_eq!(
            guard.take_finished_exit(),
            None,
            "a restart must not be reachable with a window still unasked"
        );

        let (asked, action) = drive_to_completion(&guard, true);
        assert_eq!(asked, vec!["window-1".to_string()]);
        assert_eq!(action, Some(ExitKind::Restart), "and only then does it restart");
    }

    #[test]
    fn a_refused_restart_never_reaches_the_restart() {
        let guard = CloseGuard::default();
        guard.arm("main");
        assert!(guard.begin_exit(vec!["main".into()], ExitKind::Restart));

        let (asked, action) = drive_to_completion(&guard, false);
        assert_eq!(asked, vec!["main".to_string()]);
        assert_eq!(action, None, "one 'no' abandons the restart entirely");
        assert_eq!(
            guard.take_finished_exit(),
            None,
            "and it stays abandoned — a cancelled poll is not a drained one"
        );
    }

    #[test]
    fn a_poll_remembers_which_kind_of_exit_it_is_for() {
        // A restart that finished as a quit would exit without relaunching;
        // a quit that finished as a restart would relaunch an app the user
        // asked to close.
        for kind in [ExitKind::Quit, ExitKind::Restart] {
            let guard = CloseGuard::default();
            guard.arm("main");
            assert!(guard.begin_exit(vec!["main".into()], kind));
            let (_, action) = drive_to_completion(&guard, true);
            assert_eq!(action, Some(kind));
        }
    }

    #[test]
    fn a_second_exit_request_does_not_restart_the_poll() {
        let guard = CloseGuard::default();
        guard.arm("main");
        assert!(guard.begin_exit(vec!["main".into()], ExitKind::Quit));
        assert!(
            !guard.begin_exit(vec!["main".into()], ExitKind::Restart),
            "a Restart clicked during a quit must not requeue or change the kind"
        );
        let (_, action) = drive_to_completion(&guard, true);
        assert_eq!(action, Some(ExitKind::Quit));
    }

    #[test]
    fn a_poll_with_no_armed_windows_reaches_its_exit_immediately() {
        // Nothing to ask means nothing to lose — it must not stall.
        let guard = CloseGuard::default();
        assert!(guard.begin_exit(vec!["settings".into()], ExitKind::Restart));
        let (asked, action) = drive_to_completion(&guard, true);
        assert!(asked.is_empty(), "an unarmed window is skipped, not asked");
        assert_eq!(action, Some(ExitKind::Restart));
    }

    /// The unit tests above prove the poll cannot reach an exit early. They
    /// cannot prove that nothing *bypasses* the poll — and bypassing it is
    /// exactly how the updater's "Restart Now" came to discard unsaved
    /// scratches, and how `PredefinedMenuItem::quit` did before it. Neither
    /// was a bug in the guard; both were a second door.
    ///
    /// So: this module is the only place allowed to tear the app down. A new
    /// caller of `request_restart()` or `exit(0)` anywhere else in the crate
    /// fails here, with the file and line, rather than shipping.
    #[test]
    fn nothing_outside_this_module_may_end_the_app() {
        fn scan(dir: &std::path::Path, offenders: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read src dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    scan(&path, offenders);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                if path.file_name().and_then(|f| f.to_str()) == Some("close_guard.rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("read source file");
                for (n, line) in source.lines().enumerate() {
                    let code = line.trim_start();
                    // Doc comments and ordinary comments discuss both calls by
                    // name — including in this fix's own explanation.
                    if code.starts_with("//") {
                        continue;
                    }
                    if code.contains(".request_restart(") || code.contains(".exit(0)") {
                        offenders.push(format!("{}:{}: {}", path.display(), n + 1, code));
                    }
                }
            }
        }

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        scan(&src, &mut offenders);
        assert!(
            offenders.is_empty(),
            "these end the app without asking about unsaved editors; route them \
             through close_guard::request_quit / request_restart instead:\n  {}",
            offenders.join("\n  "),
        );
    }

    #[test]
    fn no_poll_means_no_exit() {
        let guard = CloseGuard::default();
        assert_eq!(
            guard.take_finished_exit(),
            None,
            "an exit action must never appear out of nowhere"
        );
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
