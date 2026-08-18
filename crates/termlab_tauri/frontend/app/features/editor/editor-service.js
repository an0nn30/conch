// Open and save orchestration for the editor.
//
// The one place that knows a file has a location as well as contents: it
// reads through the Rust guards, hands the text to a tab, and writes it back.
(function initTermLabEditorService(global) {
  'use strict';

  function invoke(command, args) {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    if (!client || typeof client.invoke !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }
    return client.invoke(command, args);
  }

  function toastError(title, body) {
    if (global.toast && typeof global.toast.error === 'function') {
      global.toast.error(title, body);
      return;
    }
    console.error(`${title}: ${body}`);
  }

  // beforeBuildCommand only fires under `cargo tauri build`/`dev`, so a plain
  // `cargo run` yields an index.html pointing at a bundle that was never
  // generated. Say so instead of failing as an editor that does nothing.
  function bundleMissing() {
    if (global.CM6) return false;
    toastError(
      'Editor unavailable',
      'The editor bundle is missing. Run "npm run build:vendor" in crates/termlab_tauri/frontend.',
    );
    return true;
  }

  // The composed tab and pane managers live inside main-runtime's closure.
  // `global.termlabTabManager` is the FACTORY ({create}) and there is no
  // `global.paneManager` at all, so both of the obvious-looking accessors
  // would be undefined here. manager-compose-runtime.js publishes the real
  // entry points; resolve them lazily because this script loads first.
  function paneAccess() {
    return global.__termlabPaneAccess || null;
  }

  function createEditorTab(options) {
    if (typeof global.__termlabCreateEditorTab !== 'function') {
      throw new Error('editor tabs are unavailable (app not composed yet)');
    }
    return global.__termlabCreateEditorTab(options);
  }

  function currentPane() {
    const access = paneAccess();
    return access ? access.currentPane() : null;
  }

  function eachEditorPane(fn) {
    const access = paneAccess();
    if (!access) return;
    const panes = access.allPanes();
    if (!panes || typeof panes.values !== 'function') return;
    for (const pane of panes.values()) {
      if (pane && pane.kind === 'editor') fn(pane);
    }
  }

  // Opening a file that is already open focuses its tab instead of making a
  // second view of the same bytes — two editors on one path would each hold a
  // doc and the last save would silently win.
  function focusExistingEditor(filePath) {
    let found = null;
    eachEditorPane((pane) => {
      if (!found && pane.filePath === filePath) found = pane;
    });
    if (!found) return false;
    const access = paneAccess();
    access.activateTab(found.tabId);
    access.setFocusedPane(found.paneId);
    return true;
  }

  async function openLocalFile(filePath) {
    if (bundleMissing()) return;
    if (focusExistingEditor(filePath)) return;
    try {
      const contents = await invoke('editor_read_file', { path: filePath });
      createEditorTab({ filePath, contents, remote: null });
    } catch (error) {
      toastError('Cannot Open File', String(error));
    }
  }

  async function openScratch() {
    if (bundleMissing()) return;
    try {
      const [dir, existing] = await Promise.all([
        invoke('editor_scratch_dir'),
        invoke('editor_scratch_list'),
      ]);
      const name = global.termlabEditorScratch.nextScratchName(existing);
      const filePath = `${dir}/${name}`;
      await invoke('editor_write_file', { path: filePath, contents: '' });
      createEditorTab({ filePath, contents: '', remote: null });
    } catch (error) {
      toastError('Cannot Create Scratch', String(error));
    }
  }

  // One write. Rejects on failure so callers can decide what a failure means;
  // saveActiveEditor turns it into a toast, the close guards turn it into a
  // refusal to close.
  async function writeOnce(pane) {
    const contents = pane.view.state.doc.toString();
    await invoke('editor_write_file', { path: pane.filePath, contents });
    // `dirty` is a plain boolean, not a diff against the document, and the
    // update listener stops firing once it is set. So a keystroke that lands
    // between the snapshot above and this line is on screen but not in the
    // file — clearing the flag unconditionally would mark it saved and, once
    // the close guards read pane.dirty, discard it without a prompt. Only
    // reset when the buffer still matches the bytes that were written.
    if (pane.view && pane.view.state.doc.toString() === contents) {
      pane.view.termlabResetDirty();
    }
    if (pane.remote) await uploadRemote(pane);
  }

  // The write currently in flight for a pane, if any. Two overlapping writes
  // for the same file can resolve out of order: the newer one lands first and
  // clears `dirty`, then the older one overwrites the file with stale bytes
  // and there is nothing left to say so. The close guards below read
  // `pane.dirty`, so that lie is what makes them close a tab over unsaved
  // text. A pane therefore never has more than one write outstanding.
  const savesInFlight = new WeakMap();

  // Save a specific pane. saveActiveEditor covers the keyboard path; the close
  // guards need to save panes that are not focused.
  async function savePane(pane, options) {
    if (!pane || pane.kind !== 'editor' || !pane.view) return;

    const pending = savesInFlight.get(pane);
    if (pending) {
      // Join the write already running instead of starting a second one. It
      // rejects for us too if it fails, so the caller still learns about it.
      await pending;
      // That write may have snapshotted the document before our caller's text
      // existed, in which case the pane is still dirty and owes a write. One
      // retry covers it; retrying without a bound would spin forever against
      // someone who keeps typing.
      if (pane.dirty && !(options && options.noRetry)) {
        await savePane(pane, { noRetry: true });
      }
      return;
    }

    // .finally is attached before the promise is stored, so it settles only
    // after the map entry is gone — a joiner that wakes on it and retries can
    // never find this same entry still there and re-join itself.
    const inFlight = writeOnce(pane).finally(() => {
      savesInFlight.delete(pane);
    });
    savesInFlight.set(pane, inFlight);
    await inFlight;
  }

  async function saveActiveEditor() {
    const pane = currentPane();
    if (!pane || pane.kind !== 'editor' || !pane.view) return;
    try {
      await savePane(pane);
    } catch (error) {
      toastError('Save Failed', String(error));
    }
  }

  // Ask about each of `panes` in turn. Returns false — abort the whole close,
  // not just this one tab — the moment anything is less than certain: the user
  // cancelled, a save failed, the dialog service is missing, or a save
  // reported success but left the pane dirty anyway. Callers treat a false
  // return as "do not close".
  async function confirmDirtyPanes(panes) {
    const dirty = (panes || []).filter((pane) => pane && pane.kind === 'editor' && pane.dirty);
    if (dirty.length === 0) return true;

    const dialogs = global.termlabDialogService;
    if (!dialogs || typeof dialogs.confirmSave !== 'function') {
      // No way to ask means no consent. Refusing to close is recoverable;
      // closing is not.
      toastError('Unsaved Changes', 'Cannot confirm unsaved changes, so nothing was closed.');
      return false;
    }

    for (const pane of dirty) {
      const name = String(pane.filePath || 'untitled').split('/').pop();
      const choice = await dialogs.confirmSave(name);
      if (choice === 'cancel') return false;
      if (choice !== 'save') continue;
      try {
        await savePane(pane);
      } catch (error) {
        toastError('Save Failed', String(error));
        return false; // a failed save must not be treated as consent to lose it
      }
      if (pane.dirty) {
        // The write resolved but the buffer moved on. Do not close over it.
        toastError('Save Failed', `"${name}" still has unsaved changes.`);
        return false;
      }
    }
    return true;
  }

  // Walk every dirty editor in this window and ask. Returns false the moment
  // the user cancels, which aborts the whole close rather than the one tab.
  async function confirmAllDirty() {
    const dirty = [];
    eachEditorPane((pane) => { if (pane.dirty) dirty.push(pane); });
    return confirmDirtyPanes(dirty);
  }

  // Replaced with the real implementation in Task 8; a local-only save has
  // nothing to upload.
  async function uploadRemote(_pane) {}

  global.termlabEditorService = {
    openLocalFile,
    openScratch,
    saveActiveEditor,
    savePane,
    confirmDirtyPanes,
    confirmAllDirty,
    eachEditorPane,
    uploadRemote,
  };
})(window);
