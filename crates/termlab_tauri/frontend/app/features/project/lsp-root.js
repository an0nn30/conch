// Opening a project IS choosing the LSP root.
//
// When a document in a project window comes up `choosingProject` and its file
// lives under the project root, the project root is set as its context through
// the existing lsp_set_project_context path — so the per-file root-candidate
// chooser normally never appears inside a project window for files under the
// root.
//
// "Normally": the manager can still reject the adoption outright — a real
// case is a JSON file under a package.json subtree whose own candidate list
// (built from that file, not the project root) never actually includes the
// project root, so lsp_set_project_context comes back InvalidProjectRoot. On
// that rejection this pass-through backs off PERMANENTLY for that document —
// see the `failed` set below — rather than retrying, and the per-file
// chooser (features/editor/project-context.js) remains reachable as the
// fallback for that one document. A fresh open of the same path gets a fresh
// documentId and is not held back by an earlier rejection.
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

    // `answered`: documents this window has already sent an adoption call
    // for, in flight or succeeded — guards against the same status update
    // (or a same-tick duplicate) firing a second call. `failed`: documents
    // whose adoption came back rejected. Both block a resend, and a
    // rejection is never retried: lsp-state re-publishes session status on
    // every diagnostics revision for the life of the pane, so without a
    // permanent stop here a single rejected adoption would re-fire
    // lsp_set_project_context on every one of those revisions for as long as
    // the tab stays open.
    const answered = new Set();
    const failed = new Set();

    return state.subscribe((pane, paneState) => {
      if (!pane || pane.kind !== 'editor' || pane.remote || !pane.filePath) return;
      if (!shouldAdoptRoot(paneState, pane.filePath, mode)) return;
      const documentId = String(paneState.documentId);
      if (answered.has(documentId) || failed.has(documentId)) return;
      answered.add(documentId);
      Promise.resolve(bridge.setProjectContext(documentId, { kind: 'root', root: mode.root() }))
        .catch((error) => {
          failed.add(documentId);
          console.warn('project lsp root: could not set the project context', error);
        });
    });
  }

  global.termlabProjectLspRoot = { shouldAdoptRoot, install };
})(window);
