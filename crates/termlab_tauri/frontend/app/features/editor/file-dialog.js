// The editor's file chooser, as the rest of the app asks for it: ⌘O and ⌘⇧S.
//
// The chooser's DOM and every listing call it makes live in
// `features/editor/file-dialog-view.js`. What is left here is the QUESTION —
// who is asking, which pane a save is for, what to do with the answer, and the
// one-dialog-at-a-time rule that stops two callers sharing one answer.
//
// Layering:
//   - `features/editor/file-dialog-view.js` renders the chooser and answers
//     once through `onResolve`. It never asks which window it is in; the
//     label that decides which SSH sessions are addressable is handed to it
//     from here, because THIS window is the one whose panes those are.
//   - `features/files/data-service.js` owns every invoke the chooser needs.
//     No raw invoke names appear below.
//   - `termlabEditorService.openLocalFile` / `.openRemoteFile` are the only
//     open paths. This file adds none: the size cap, the binary/extension
//     blocklist and the same-file-focus behaviour all come free from them.
(function initTermLabFileDialog(global) {
  'use strict';

  // ---------------------------------------------------------------------------
  // Ambient lookups (resolved lazily — this script loads before main-runtime
  // publishes termlabServices, exactly like editor-service.js).
  // ---------------------------------------------------------------------------

  function invoke(command, args) {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    if (!client || typeof client.invoke !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }
    return client.invoke(command, args);
  }

  function filesData() {
    return global.termlabFilesFeatureDataService || null;
  }

  function fileModel() {
    return global.termlabFileDialogModel || null;
  }

  function chooserView() {
    return global.termlabFileDialogView || null;
  }

  function toastError(title, body) {
    if (global.toast && typeof global.toast.error === 'function') {
      global.toast.error(title, body);
      return;
    }
    console.error(`${title}: ${body}`);
  }

  // ---------------------------------------------------------------------------
  // The chooser
  // ---------------------------------------------------------------------------

  // One dialog at a time. A second ⌘O while the chooser is up should surface
  // the dialog that is already open, not stack a second modal over it.
  //
  // The session records which QUESTION that dialog is asking (`mode`) and, for
  // a save, which pane it is asking on behalf of (`pane`). Both are load
  // bearing, not bookkeeping: see the mode check in chooseFile and
  // cancelForPane below.
  let activeChoice = null;

  /**
   * Show the chooser. Resolves with
   *   { scope, path, entry }   — a file the user picked (in save mode `entry`
   *                              is the existing file being replaced, or null)
   *   null                     — cancelled (Cancel, Escape, backdrop, or a
   *                              failure to even build the scope list)
   * It never rejects: a cancelled chooser is not an error.
   *
   * options = { mode: 'open' | 'save', filename, selectFilename, pane }
   *   'save' adds the filename field (pre-filled with `filename`), the New
   *   Folder button and the existence check; the primary button reads Save.
   *   `selectFilename` focuses that field with its text selected — for a
   *   placeholder name (an untitled buffer's "Untitled-2") that is there to be
   *   typed over rather than edited.
   *   `pane` is the editor this save is FOR. It is never read for the dialog
   *   itself; it exists so cancelForPane can close a chooser whose subject has
   *   been destroyed out from under it.
   */
  function chooseFile(options) {
    const opts = options || {};
    const saveMode = opts.mode === 'save';
    const mode = saveMode ? 'save' : 'open';

    if (activeChoice) {
      // The SAME question asked twice — a second ⌘O while the open chooser is
      // up. Both callers want whichever file the user is about to pick, so
      // handing over the one dialog's answer is right (and is what keeps a
      // second ⌘O from stacking a modal over the first).
      if (activeChoice.mode === mode) return activeChoice.promise;

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
      // dialog on screen is the one the user is answering.
      return Promise.resolve(null);
    }

    const failTitle = saveMode ? 'Cannot Save File' : 'Cannot Open File';

    if (!fileModel()) {
      toastError(failTitle, 'The file dialog model is unavailable.');
      return Promise.resolve(null);
    }
    const view = chooserView();
    if (!view || typeof view.build !== 'function') {
      toastError(failTitle, 'The file chooser is unavailable.');
      return Promise.resolve(null);
    }
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
      toastError(failTitle, 'The dialog stack is unavailable.');
      return Promise.resolve(null);
    }

    let resolveChoice = null;
    const promise = new Promise((resolve) => { resolveChoice = resolve; });
    // `finish` is a hoisted function declaration further down this closure, so
    // the arrow resolves it at call time — the session is complete before the
    // dialog exists, which is what lets cancelForPane fire at any moment.
    const session = { promise, mode, pane: opts.pane || null, cancel: () => finish(null) };
    activeChoice = session;

    // The latch. Escape and the backdrop reach here through tl-dialog's
    // onClose, and so do the view's own Cancel/Escape/pick through onResolve;
    // whichever arrives first is the answer, and handle.close()'s own onClose
    // cannot overwrite it (same shape as dialog-service.js's confirmSave).
    let done = false;
    let handle = null;
    function finish(result) {
      if (done) return;
      done = true;
      activeChoice = null;
      resolveChoice(result || null);
      if (handle) handle.close(result ? 'open' : 'cancel');
    }

    // The label the sessions are keyed by. THIS window is the parent of the
    // chooser it opens, so its own label is the right answer — and asking for
    // it here rather than inside the view is what lets the same view render in
    // a chooser window of its own, where its own label would be the wrong one.
    const data = filesData();
    const parentLabel = data && typeof data.getCurrentWindowLabel === 'function'
      ? data.getCurrentWindowLabel(invoke).catch(() => null)
      : Promise.resolve(null);

    parentLabel.then((parentWindowLabel) => {
      if (done) return;                  // cancelForPane got here first
      // TEMPORARY (this whole host goes away when the chooser moves into its
      // own window): the view is mounted in a tl-dialog body, and the footer
      // it renders for itself is re-homed below into the panel's footer row.
      let mounted = null;
      let bodyRoot = null;
      handle = global.tlDialog.open({
        title: saveMode ? 'Save File As' : 'Open File',
        ariaLabel: saveMode ? 'Save file as' : 'Open file',
        size: 'lg',
        body: (bodyEl) => {
          bodyRoot = bodyEl;
          mounted = view.build(bodyEl, {
            data,
            mode,
            filename: opts.filename,
            selectFilename: opts.selectFilename,
            parentWindowLabel,
            onResolve: (result) => finish(result),
          });
        },
        onOpen: (panel) => {
          // The panel exists only now, and the view's footer is currently the
          // last child of the dialog BODY. Move it into a footer row of the
          // panel so this host keeps the shape every other tl-dialog has.
          const footer = bodyRoot && typeof bodyRoot.querySelector === 'function'
            ? bodyRoot.querySelector('.tl-filedlg__footer')
            : null;
          if (panel && footer && typeof panel.appendChild === 'function') {
            const row = document.createElement('div');
            row.className = 'tl-dialog__footer';
            if (footer.parentNode) footer.parentNode.removeChild(footer);
            row.appendChild(footer);
            panel.appendChild(row);
          }
          if (mounted && typeof mounted.focusInitial === 'function') mounted.focusInitial();
        },
        onClose: () => finish(null),
      });
    });

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

    // A chooser is already on screen. chooseFile's `activeChoice`
    // short-circuit would hand this caller THAT dialog's answer — and a path
    // the user picked for one pane is never where another pane's buffer
    // belongs: both tabs would rebind to one file and the second write would
    // bury the first, with nothing on screen to say so. Refuse instead. The
    // dialog they are looking at is the one they are answering, and savePane
    // reads a null here as a quiet not-saved (no toast, no close).
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
      // Recorded so a teardown of this pane can close the dialog. See
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
   * chooser (which has no subject), and when nothing is on screen.
   *
   * Called from the two pane teardown paths — tab-manager.js's closeTab and
   * pane-manager.js's split close — via the editor service. A chooser outlives
   * its subject easily: ⌘W over an open chooser stacks the Unsaved Changes
   * prompt on top of it, and "Don't Save" destroys the pane while the chooser
   * stays up, still bound to it. What is left is a modal asking where to put a
   * buffer that no longer exists — and, because it is still `activeChoice`, it
   * blocks the chooser every OTHER pane would open until someone answers it.
   * Answering does not even write: saveAs no-ops on a pane whose view has been
   * nulled, so the save silently reports success having done nothing.
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
    // Exposed for scripts/tests/test_file_dialog.mjs. The scope derivations
    // moved out with the view and are exported by termlabFileDialogView; what
    // is left here is the one entry point those tests drive directly.
    _chooseFile: chooseFile,
  };
})(window);
