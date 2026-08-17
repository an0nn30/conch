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

  async function saveActiveEditor() {
    const pane = currentPane();
    if (!pane || pane.kind !== 'editor' || !pane.view) return;
    const contents = pane.view.state.doc.toString();
    try {
      await invoke('editor_write_file', { path: pane.filePath, contents });
      pane.view.termlabResetDirty();
      if (pane.remote) await uploadRemote(pane);
    } catch (error) {
      toastError('Save Failed', String(error));
    }
  }

  // Replaced with the real implementation in Task 8; a local-only save has
  // nothing to upload.
  async function uploadRemote(_pane) {}

  global.termlabEditorService = {
    openLocalFile,
    openScratch,
    saveActiveEditor,
    eachEditorPane,
    uploadRemote,
  };
})(window);
