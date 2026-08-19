// The editor's file chooser, as the rest of the app asks for it: ⌘O and ⌘⇧S.
//
// The chooser itself is not here and is not in this window: it renders in a
// chooser window of its own (`features/editor/file-dialog-view.js`, hosted by
// `chooser.html`). What is left in this file is the QUESTION — who is asking,
// which pane a save is for, what to do with the answer, and the
// one-chooser-at-a-time rule that stops two callers sharing one answer.
//
// So `chooseFile` is a PROXY across the window boundary:
//
//   raise the scrim  ->  listen for `chooser-resolved`  ->  open_file_chooser
//                                                              |
//   resolve(choice)  <-  the settle latch  <-  the event Rust emits back
//
// Three things about that shape are load bearing:
//
//   * The scrim goes up BEFORE the invoke and comes down in the settle latch's
//     `finally`. The chooser window is window-modal to this one, and the scrim
//     (plus `inert` on the app root) is what makes that true for the webview
//     as well. A scrim that outlives its chooser is a locked app, so every
//     route out — a pick, a cancel, a listen that rejects, an invoke that
//     rejects — goes through the one latch.
//   * The listener attaches before the invoke: a resolution cannot outrun a
//     listener that already exists. But the invoke's RETURN (the req_id) and
//     the event both originate in Rust after the registry insert, and a fast
//     cancel lets the event win — so an event arriving before the req_id is
//     known is buffered and re-checked once it is.
//   * This window never asks for its own label. Rust derives the chooser's
//     parent from the CALLING window, which is this one by definition;
//     fetching a label to say so would only add a window in which a cancel can
//     strand the session.
//
// Layering:
//   - `termlabEditorService.openLocalFile` / `.openRemoteFile` / `.saveAs` are
//     the only open/save paths. This file adds none: the size cap, the
//     binary/extension blocklist, the same-file-focus behaviour and the pane
//     rebind all come free from them.
//   - No listing call, no filesystem invoke and no DOM of the chooser's own
//     appears below — those belong to the chooser window.
(function initTermLabFileDialog(global) {
  'use strict';

  // ---------------------------------------------------------------------------
  // Ambient lookups (resolved lazily — this script loads before main-runtime
  // publishes termlabServices, exactly like editor-service.js).
  // ---------------------------------------------------------------------------

  function tauriClient() {
    return (global.termlabServices && global.termlabServices.tauriClient) || null;
  }

  function invoke(command, args) {
    const client = tauriClient();
    if (!client || typeof client.invoke !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }
    return client.invoke(command, args);
  }

  // Deliberately the per-WINDOW listener: Rust emits `chooser-resolved` to the
  // parent window only, so a broadcast listener would also hear other windows'
  // choosers answering. (The reqId check below would drop them, but relying on
  // that is relying on ids never colliding across windows.)
  function listenOnCurrentWindow(eventName, handler) {
    const client = tauriClient();
    if (!client || typeof client.listenOnCurrentWindow !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }
    return client.listenOnCurrentWindow(eventName, handler);
  }

  function toastError(title, body) {
    if (global.toast && typeof global.toast.error === 'function') {
      global.toast.error(title, body);
      return;
    }
    console.error(`${title}: ${body}`);
  }

  // ---------------------------------------------------------------------------
  // The scrim
  // ---------------------------------------------------------------------------
  //
  // The chooser window is window-modal, but a webview does not stop taking
  // clicks just because another window is in front of it. The scrim is the
  // visual half of that modality and `inert` is the functional half — without
  // it, the app underneath stays keyboard-reachable and a second ⌘O-adjacent
  // action can run while the chooser is up.

  const SCRIM_CLASS = 'tl-chooser-scrim';
  const APP_ROOT_ID = 'app';

  function appRoot() {
    const doc = global.document;
    if (!doc || typeof doc.getElementById !== 'function') return null;
    return doc.getElementById(APP_ROOT_ID);
  }

  function raiseScrim() {
    const doc = global.document;
    if (!doc || !doc.body) return null;
    const scrim = doc.createElement('div');
    scrim.className = SCRIM_CLASS;
    doc.body.appendChild(scrim);
    const root = appRoot();
    if (root) {
      root.setAttribute('inert', '');
      root.setAttribute('aria-hidden', 'true');
    }
    return scrim;
  }

  // Idempotent: the latch calls this exactly once, but a scrim that is already
  // detached must not throw on the way out — a throw here is the locked app it
  // exists to prevent.
  function lowerScrim(scrim) {
    if (scrim && scrim.parentNode) scrim.parentNode.removeChild(scrim);
    const root = appRoot();
    if (root) {
      root.removeAttribute('inert');
      root.removeAttribute('aria-hidden');
    }
  }

  // ---------------------------------------------------------------------------
  // The chooser
  // ---------------------------------------------------------------------------

  // One chooser at a time. A second ⌘O while the chooser window is up should
  // surface the window that is already open, not spawn a second one.
  //
  // The session records which QUESTION that chooser is asking (`mode`) and, for
  // a save, which pane it is asking on behalf of (`pane`). Both are load
  // bearing, not bookkeeping: see the mode check in chooseFile and
  // cancelForPane below.
  let activeChoice = null;

  /**
   * Show the chooser. Resolves with
   *   { scope, path, entry }   — a file the user picked (in save mode `entry`
   *                              is the existing file being replaced, or null)
   *   null                     — cancelled (Cancel, Escape, the window's close
   *                              button, a teardown of the pane it was for, or
   *                              a failure to open the window at all)
   * It never rejects: a cancelled chooser is not an error.
   *
   * `scope` is the plain object the chooser window put in its answer
   * (`{id, kind, label, hostLabel, paneId, start}`), carried back over the
   * event as JSON. The two callers below read only `kind`, `paneId` and
   * `hostLabel` from it.
   *
   * options = { mode: 'open' | 'save', filename, selectFilename, pane }
   *   'save' adds the filename field (pre-filled with `filename`), the New
   *   Folder button and the existence check; the primary button reads Save.
   *   `selectFilename` focuses that field with its text selected — for a
   *   placeholder name (an untitled buffer's "Untitled-2") that is there to be
   *   typed over rather than edited.
   *   `pane` is the editor this save is FOR. It is never sent to the chooser
   *   window; it exists so cancelForPane can close a chooser whose subject has
   *   been destroyed out from under it.
   */
  function chooseFile(options) {
    const opts = options || {};
    const saveMode = opts.mode === 'save';
    const mode = saveMode ? 'save' : 'open';

    if (activeChoice) {
      // The SAME question asked twice — a second ⌘O while the open chooser is
      // up. Both callers want whichever file the user is about to pick, so
      // handing over the one chooser's answer is right. Fire-and-forget focus:
      // the window may be behind the main one, and a keystroke that appears to
      // do nothing is worse than the duplicate it is preventing.
      if (activeChoice.mode === mode) {
        invoke('focus_file_chooser').catch(() => {});
        return activeChoice.promise;
      }

      // A DIFFERENT question. ⌘O is a native menu accelerator: AppKit consumes
      // it before the webview, so no focus trap stops it reaching here while an
      // untitled buffer's first-save chooser is on screen. Sharing the promise
      // would make one Return mean two things at once — the untitled pane
      // rebinds to the chosen path AND openLocalFile opens a second tab on that
      // same path. Two editors on one file is exactly the state
      // focusExistingEditor exists to prevent, and it would happen silently.
      //
      // So: refuse. Null is the same answer a cancel gives, and both public
      // entry points already treat it as "nothing to do, say nothing" — the
      // chooser on screen is the one the user is answering.
      return Promise.resolve(null);
    }

    const failTitle = saveMode ? 'Cannot Save File' : 'Cannot Open File';

    let resolveChoice = null;
    const promise = new Promise((resolve) => { resolveChoice = resolve; });
    // `doCancel` is a hoisted function declaration further down this closure,
    // so the session is complete before the chooser window exists — which is
    // what lets cancelForPane fire at any moment, including while the window
    // is still being built.
    const session = { promise, mode, pane: opts.pane || null, settled: false, cancel: doCancel };
    activeChoice = session;

    // Before the invoke, always: the window must never appear over a parent
    // that is still live.
    const scrim = raiseScrim();

    let unlisten = null;
    let myReqId = null;
    // One event, held for the length of the race described at the top of this
    // file. One is enough: there is exactly one chooser per parent, so there
    // is exactly one answer that can arrive this early.
    let buffered = null;

    // The latch. A pick, a cancel, a window that would not build and a
    // transport that is not there all arrive here, and whichever is first is
    // the answer. Everything that must happen exactly once — releasing the
    // listener, lowering the scrim, releasing `activeChoice` — happens here
    // and nowhere else.
    const settle = (choice) => {
      if (session.settled) return;
      session.settled = true;
      try { if (unlisten) unlisten(); } finally { lowerScrim(scrim); }
      if (activeChoice === session) activeChoice = null;
      resolveChoice(choice || null);
    };

    function doCancel() {
      // No reqId: Rust force-resolves whatever is live for the calling window,
      // so this works even before the invoke has told us which chooser that is.
      invoke('cancel_file_chooser').catch(() => {}).finally(() => settle(null));
    }

    (async () => {
      try {
        unlisten = await listenOnCurrentWindow('chooser-resolved', (event) => {
          const p = event && event.payload;
          if (!p) return;
          // The reqId is not known yet — see the buffer note above. Anything
          // that arrives now is either this chooser's answer or nothing at
          // all, and the re-check below decides which.
          if (myReqId === null) {
            if (!buffered) buffered = p;
            return;
          }
          if (p.reqId !== myReqId) return;   // another chooser's answer, or a stale one
          settle(p.choice || null);
        });
        // Cancelled while the listener was being attached: nothing was opened,
        // so there is nothing to close — just release the listener.
        if (session.settled) {
          unlisten();
          return;
        }

        myReqId = await invoke('open_file_chooser', {
          mode,
          filename: opts.filename || null,
          selectFilename: !!opts.selectFilename,
        });

        if (session.settled) {
          // Cancelled while the window was being built. The cancel raced ahead
          // of the registry insert, so Rust's force-resolve found nothing —
          // send it again now that there IS something, or the user is left
          // with a chooser window nobody is listening to.
          invoke('cancel_file_chooser').catch(() => {});
          return;
        }
        if (buffered && buffered.reqId === myReqId) settle(buffered.choice || null);
      } catch (error) {
        // Every failure route lands here — including anything thrown by the
        // handler wiring itself. Leaving without settling would claim
        // `activeChoice` forever and refuse every later ⌘O in silence.
        toastError(failTitle, String(error));
        settle(null);
      }
    })();

    return promise;
  }

  // ---------------------------------------------------------------------------
  // Public entry points
  // ---------------------------------------------------------------------------

  /**
   * ⌘O. Shows the chooser and routes the pick through the editor service's
   * existing open paths — no new open path, so the size cap, the extension
   * blocklist, the binary sniff and the focus-the-tab-you-already-have
   * behaviour all apply unchanged.
   */
  async function openForOpen() {
    const choice = await chooseFile();
    if (!choice) return null;
    const editor = global.termlabEditorService;
    if (!editor) {
      toastError('Editor Unavailable', 'The editor service is not loaded.');
      return null;
    }
    if (choice.scope.kind === 'local') {
      await editor.openLocalFile(choice.path);
      return choice;
    }
    await editor.openRemoteFile({
      paneId: choice.scope.paneId,
      remotePath: choice.path,
      hostLabel: choice.scope.hostLabel,
      size: choice.entry ? choice.entry.size : 0,
    });
    return choice;
  }

  function basename(p) {
    const segments = String(p || '').split('/').filter(Boolean);
    return segments.length ? segments[segments.length - 1] : '';
  }

  /**
   * ⌘⇧S. Shows the same chooser in save mode and routes the chosen target
   * through `termlabEditorService.saveAs`, which owns the write, the upload
   * and the pane rebind — this file never touches pane identity.
   *
   * Resolves with the chooser's result, or null when it was cancelled or the
   * save failed. It does not reject: `saveAs` has already told the user what
   * went wrong (and left the pane on its old binding), so a second report
   * here would only say it twice.
   */
  async function openForSave(pane) {
    if (!pane || pane.kind !== 'editor') return null;

    // A chooser is already up. chooseFile's `activeChoice` short-circuit would
    // hand this caller THAT chooser's answer — and a path the user picked for
    // one pane is never where another pane's buffer belongs: both tabs would
    // rebind to one file and the second write would bury the first, with
    // nothing on screen to say so. Refuse instead. The chooser they are
    // looking at is the one they are answering, and savePane reads a null here
    // as a quiet not-saved (no toast, no close).
    //
    // Same-pane re-entry does not reach this: editor-service parks a
    // per-pane entry for the duration of the await and joins it. This is the
    // backstop for every other route, including a window-close sweep that
    // walks a second dirty pane while the first pane's chooser is up.
    if (activeChoice) return null;
    // The name the USER knows this file by. For a remote pane that is the
    // remote basename — pane.filePath is the local temp file it was
    // downloaded into, which they have never seen.
    // An untitled buffer has no path to take a basename from. Its tab label
    // ("Untitled-2") is the only name it has, so that is what the field is
    // seeded with — selected, because it is a placeholder to type over rather
    // than a name to extend.
    const untitled = !pane.filePath && !pane.remote;
    const labels = global.termlabEditorTabLabel;
    let currentName;
    if (pane.remote) {
      currentName = basename(pane.remote.remotePath);
    } else if (untitled && labels && typeof labels.editorTabLabel === 'function') {
      currentName = labels.editorTabLabel(pane).label;
    } else {
      currentName = basename(pane.filePath);
    }

    const choice = await chooseFile({
      mode: 'save',
      filename: currentName,
      selectFilename: untitled,
      // Recorded so a teardown of this pane can close the chooser. See
      // cancelForPane.
      pane,
    });
    if (!choice) return null;

    const editor = global.termlabEditorService;
    if (!editor || typeof editor.saveAs !== 'function') {
      toastError('Editor Unavailable', 'The editor service is not loaded.');
      return null;
    }

    const target = choice.scope.kind === 'local'
      ? { scope: 'local', path: choice.path }
      : {
        scope: 'remote',
        paneId: choice.scope.paneId,
        // The CLEAN identity string, never the button's disambiguated
        // caption: editor_temp_path hashes this into the file's temp path,
        // so a " (pane N)" suffix here would split one remote file across
        // two tabs.
        hostLabel: choice.scope.hostLabel,
        remotePath: choice.path,
      };

    try {
      await editor.saveAs(pane, target);
    } catch (_) {
      return null;
    }
    return choice;
  }

  /**
   * Close the save chooser currently open FOR `pane`, resolving it null, and
   * report whether there was one. A no-op for any other pane, for an open-mode
   * chooser (which has no subject), and when nothing is open.
   *
   * Called from the two pane teardown paths — tab-manager.js's closeTab and
   * pane-manager.js's split close — via the editor service. A chooser outlives
   * its subject easily: ⌘W over an open chooser puts the Unsaved Changes
   * prompt up in THIS window, and "Don't Save" destroys the pane while the
   * chooser window stays up, still bound to it. What is left is a window
   * asking where to put a buffer that no longer exists — and, because it is
   * still `activeChoice`, it blocks the chooser every OTHER pane would open
   * until someone answers it. Answering does not even write: saveAs no-ops on
   * a pane whose view has been nulled, so the save silently reports success
   * having done nothing.
   */
  function cancelForPane(pane) {
    if (!pane || !activeChoice || activeChoice.pane !== pane) return false;
    activeChoice.cancel();
    return true;
  }

  global.termlabFileDialog = {
    openForOpen,
    openForSave,
    cancelForPane,
    // Exposed for scripts/tests/test_file_dialog_proxy.mjs (and the entry-point
    // checks in test_file_dialog.mjs). The chooser's own behaviour is tested
    // against the view, which this file no longer touches.
    _chooseFile: chooseFile,
  };
})(window);
