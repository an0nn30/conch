// The chooser WINDOW's boot runtime — the thin module chooser.html loads
// last. Asks Rust for the request this window was built to answer
// (Task 1's `get_chooser_request`, resolved by exact window-label match —
// see chooser_window.rs's `get_by_window_label`), mounts Task 2's
// `file-dialog-view.js` as the window's whole content, and forwards its one
// answer back through `resolve_file_chooser`.
//
// This file never reads its own window label, and never should: window
// labels are unique per request (`chooser-<parent>-<reqId>`, chooser_window.
// rs's `ChooserRegistry::open`), so there is nothing useful to parse out of
// it, and the one place a label WOULD matter — which parent's SSH sessions
// this chooser may offer — arrives pre-resolved as `request.parentLabel`.
// Handing the view anything else here is the exact bug `file-dialog-view.js`
// guards against in its own header comment.
//
// It also never registers a close-guard handshake. `close_guard::
// on_close_requested` still fires for this window's CloseRequested (it is
// wired into every window's WindowEvent handler in lib.rs), but it is a
// no-op for an unarmed window — CloseGuard.take_permission returns true
// whenever the label was never armed (close_guard.rs's `take_permission`:
// "An unarmed window is always allowed through") — so the close proceeds,
// and `chooser_window::on_chooser_close_requested` (wired right after it)
// is the hook that actually owns this window's close semantics: registry
// cleanup, the `chooser-resolved` emit, and size persistence.
(function initChooserWindowRuntime(global) {
  'use strict';

  // The no-`close_current_window`-command fallback: no such command exists
  // in commands.rs (grepped at HEAD), so the window closes itself the same
  // way settings.js and titlebar.js already do (settings.js:310,
  // ui/titlebar.js:428). `close()` raises CloseRequested, which
  // `on_chooser_close_requested` answers — but only for a window whose
  // label the registry recognizes. On the no-pending-request path taken
  // here there is no registry entry for this window at all (the request
  // never arrived, or was never made), so that hook's own lookup finds
  // nothing and returns immediately: a harmless no-op, not a second
  // resolution of anything.
  function closeThisWindow() {
    const tauri = global.__TAURI__;
    if (!tauri || !tauri.window || typeof tauri.window.getCurrentWindow !== 'function') return;
    const win = tauri.window.getCurrentWindow();
    if (win && typeof win.close === 'function') win.close();
  }

  async function boot() {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    const invoke = client.invoke.bind(client);

    // "A chooser with no question must not linger" (design spec, "Window &
    // lifecycle"): a rejection here means Rust has no pending request for
    // THIS window's label — the parent's chooser was displaced out from
    // under this window before it could even ask, or it was opened with
    // nothing behind it. Either way the view never builds; there is no
    // request to build it FROM.
    let request;
    try {
      request = await invoke('get_chooser_request');
    } catch (e) {
      closeThisWindow();
      return;
    }

    const root = document.getElementById('chooser-root');
    const view = global.termlabFileDialogView.build(root, {
      data: global.termlabFilesFeatureDataService,
      mode: request.mode,
      filename: request.filename,
      selectFilename: request.selectFilename,
      // The PARENT's label, exactly as Rust resolved it — never this
      // window's own. See the file header.
      parentWindowLabel: request.parentLabel,
      onResolve: (choice) => {
        invoke('resolve_file_chooser', { reqId: request.reqId, choice }).catch(() => {});
      },
    });

    // Show and focus only once the view is actually on screen — mirrors
    // `app_ready` (commands.rs:248-251); chooser_ready is this window's
    // version of it (chooser_window.rs's `chooser_ready`: show() then
    // set_focus()).
    await invoke('chooser_ready');

    // Deferred to the next frame, matching how the view's OWN save-mode
    // focus (`selectNameField`, file-dialog-view.js) already defers itself
    // when a `requestAnimationFrame` is available: `focusInitial`'s other
    // branch — the plain `list.focus()` used by every open-mode chooser and
    // every save-mode chooser without a placeholder name — is NOT deferred
    // internally, because the tl-dialog bridge host used to override it a
    // frame later from its own queued rAF (file-dialog-view.js:1063-1068).
    // This window has no such override, so calling focusInitial()
    // synchronously right after `chooser_ready` would ask a webview that
    // has not necessarily finished first paint (show()/set_focus() just
    // landed) to move focus. Wrapping the call here — once, at the host
    // level — gives BOTH branches the same one-frame grace consistently,
    // rather than special-casing the view for a host it does not know
    // about.
    if (typeof global.requestAnimationFrame === 'function') {
      global.requestAnimationFrame(() => view.focusInitial());
    } else {
      view.focusInitial();
    }
  }

  boot();
})(window);
