// Opening a project IS choosing the LSP root.
//
// When a document in a project window comes up `choosingProject` and its file
// lives under the project root, the project root is set as its context through
// the existing lsp_set_project_context path — so the per-file root-candidate
// chooser never appears inside a project window for files under the root.
//
// Files OUTSIDE the root (a `gd` into std, or a cargo-registry source) are
// deliberately left alone: they keep the loose-file behaviour — a plain
// editable tab, no prompts, no attach.
(function initTermLabProjectLspRoot(global) {
  'use strict';

  function shouldAdoptRoot(paneState, filePath, mode) {
    if (!paneState || !paneState.documentId) return false;
    const status = paneState.status;
    if (!status || status.state !== 'choosingProject') return false;
    if (!mode || typeof mode.isActive !== 'function' || !mode.isActive()) return false;
    return mode.isUnderRoot(filePath);
  }

  function install(options) {
    const opts = options || {};
    const state = opts.state || global.termlabLspState || null;
    const bridge = opts.bridge || global.termlabLspBridge || null;
    const mode = opts.mode || global.termlabProjectMode || null;
    if (!state || typeof state.subscribe !== 'function') return function () {};
    if (!bridge || typeof bridge.setProjectContext !== 'function') return function () {};

    // Once per document: the manager republishes status on every revision, and
    // re-sending the same choice would restart the session in a loop.
    const answered = new Set();

    return state.subscribe((pane, paneState) => {
      if (!pane || pane.kind !== 'editor' || pane.remote || !pane.filePath) return;
      if (!shouldAdoptRoot(paneState, pane.filePath, mode)) return;
      const documentId = String(paneState.documentId);
      if (answered.has(documentId)) return;
      answered.add(documentId);
      Promise.resolve(bridge.setProjectContext(documentId, { kind: 'root', root: mode.root() }))
        .catch((error) => {
          answered.delete(documentId);
          console.warn('project lsp root: could not set the project context', error);
        });
    });
  }

  global.termlabProjectLspRoot = { shouldAdoptRoot, install };
})(window);
