// Server diagnostics, projected onto the CodeMirror views that own the files.
//
// Rust owns the authoritative diagnostic store. This module holds NO list of
// its own: every publication carries the whole snapshot, so rendering is
// "filter the snapshot to this pane's URI and hand CodeMirror the result".
// The only per-view state kept here is the revision last applied, which is
// what makes an out-of-order event a no-op instead of a resurrection of
// diagnostics the server has already withdrawn.
//
// It reaches the backend through exactly one seam — lsp-bridge's diagnostics
// fan-out, which owns the single Tauri subscription — and it registers no
// keyboard handlers at all: F8/Shift-F8 traverse the workspace-wide Problems
// snapshot (features/problems/), not one document's marks.
(function initTermLabLspDiagnostics(global) {
  'use strict';

  // LSP names four severities; CodeMirror's lint knows four too, but calls
  // one of them something else. An unrecognized severity renders as an error
  // rather than vanishing: a diagnostic nobody can see is worse than one
  // drawn too loudly.
  const SEVERITY = {
    error: 'error',
    warning: 'warning',
    information: 'info',
    hint: 'hint',
  };

  let allPanesHook = null;
  let unsubscribe = null;
  // Pane -> the revision whose marks that pane is currently showing.
  const appliedRevisions = new WeakMap();

  function cm() {
    return global.CM6 || null;
  }

  function uriToPath(uri) {
    const uris = global.termlabLspUri;
    return uris && typeof uris.uriToPath === 'function' ? uris.uriToPath(uri) : String(uri || '');
  }

  // --- position conversion ----------------------------------------------------
  //
  // LSP positions from Rust are already UTF-16 code units within a line, which
  // is the same unit a CodeMirror offset counts in, so this is addition and
  // clamping and nothing more. Both clamps matter: a server that has raced
  // ahead of the document can name a line past the end, and CodeMirror throws
  // on an out-of-range line rather than saturating.

  function offsetAt(doc, position) {
    const line = Number(position && position.line);
    const character = Number(position && position.character);
    const wanted = Number.isFinite(line) ? line + 1 : 1;
    const clampedLine = Math.min(Math.max(wanted, 1), doc.lines);
    const entry = doc.line(clampedLine);
    const column = Number.isFinite(character) && character > 0 ? character : 0;
    return Math.min(entry.from + column, entry.to);
  }

  // --- tooltip ----------------------------------------------------------------

  function metaText(item) {
    const source = item.source ? String(item.source) : '';
    const code = item.code === null || item.code === undefined ? '' : String(item.code);
    if (source && code) return `${source}(${code})`;
    return source || code;
  }

  // Server text is inserted as textContent, never as markup — the same rule
  // the completion info panel follows, for the same reason.
  function renderMessage(item) {
    const root = global.document.createElement('div');
    root.className = 'tl-diagnostic';
    const message = global.document.createElement('div');
    message.className = 'tl-diagnostic__message';
    message.textContent = String(item.message || '');
    root.appendChild(message);
    const meta = metaText(item);
    if (meta) {
      const node = global.document.createElement('div');
      node.className = 'tl-diagnostic__meta';
      node.textContent = meta;
      root.appendChild(node);
    }
    return root;
  }

  // --- translation ------------------------------------------------------------

  function toCodeMirrorDiagnostics(doc, items) {
    const out = [];
    for (const item of items || []) {
      if (!item || !item.range) continue;
      const from = offsetAt(doc, item.range.start);
      // A server may report end before start after a race; a negative-length
      // range is not something CodeMirror can place.
      const to = Math.max(offsetAt(doc, item.range.end), from);
      const diagnostic = {
        from,
        to,
        severity: SEVERITY[item.severity] || 'error',
        message: String(item.message || ''),
        renderMessage: () => renderMessage(item),
      };
      if (item.source) diagnostic.source = String(item.source);
      out.push(diagnostic);
    }
    return out;
  }

  // --- routing ----------------------------------------------------------------

  function editorPanes() {
    const panes = typeof allPanesHook === 'function' ? allPanesHook() : null;
    if (!panes || typeof panes.values !== 'function') return [];
    const out = [];
    for (const pane of panes.values()) {
      // Remote buffers never enter the ownership registry in Rust and never
      // attach to a session, so they can never be a diagnostic's owner here.
      if (pane && pane.kind === 'editor' && pane.view && !pane.remote && pane.filePath) {
        out.push(pane);
      }
    }
    return out;
  }

  function applyToPane(pane, update, revision) {
    const CM = cm();
    if (!CM || typeof CM.setDiagnostics !== 'function') return false;
    const applied = appliedRevisions.get(pane);
    if (applied !== undefined && revision <= applied) return false;
    const filePath = String(pane.filePath);
    const items = (update.snapshot && update.snapshot.items) || [];
    const mine = items.filter((item) => item && uriToPath(item.uri) === filePath);
    // An event that names another file and carries nothing for this one is
    // not this pane's news: dispatching an identical empty set would cost a
    // transaction and, worse, claim this pane is up to date at that revision
    // when the next event for it may still be older.
    if (!mine.length && update.uri && uriToPath(update.uri) !== filePath) return false;
    appliedRevisions.set(pane, revision);
    pane.view.dispatch(CM.setDiagnostics(
      pane.view.state,
      toCodeMirrorDiagnostics(pane.view.state.doc, mine),
    ));
    return true;
  }

  function applyUpdate(update) {
    if (!update || !update.snapshot) return 0;
    const revision = Number(update.revision);
    if (!Number.isFinite(revision)) return 0;
    let applied = 0;
    for (const pane of editorPanes()) {
      try {
        if (applyToPane(pane, update, revision)) applied += 1;
      } catch (error) {
        console.error('Diagnostics could not be rendered for a pane', error);
      }
    }
    return applied;
  }

  // --- extensions -------------------------------------------------------------
  //
  // `linter(null, …)` is the documented way to mount the lint machinery
  // without a local lint source: this app never lints, it only renders what
  // Rust normalized. `setDiagnostics` would append the same extensions on
  // first use anyway — mounting them up front is what lets the gutter exist
  // before the first publication, and gives the tooltip a place to be
  // configured.

  function extensions() {
    const CM = cm();
    if (!CM || typeof CM.linter !== 'function') return [];
    const list = [CM.linter(null, { delay: 0 })];
    if (typeof CM.lintGutter === 'function') list.push(CM.lintGutter());
    return list;
  }

  // --- lifecycle --------------------------------------------------------------

  function subscribe() {
    if (unsubscribe) return;
    const bridge = global.termlabLspBridge;
    if (!bridge || typeof bridge.subscribeDiagnostics !== 'function') return;
    unsubscribe = bridge.subscribeDiagnostics((payload) => { applyUpdate(payload); });
  }

  function configure(options) {
    const opts = options || {};
    if (typeof opts.allPanes === 'function') allPanesHook = opts.allPanes;
    subscribe();
  }

  function dispose() {
    if (typeof unsubscribe === 'function') unsubscribe();
    unsubscribe = null;
  }

  global.termlabLspDiagnostics = {
    configure,
    dispose,
    extensions,
    applyUpdate,
    toCodeMirrorDiagnostics,
    severityFor: (severity) => SEVERITY[severity] || 'error',
    appliedRevision: (pane) => appliedRevisions.get(pane),
  };
})(window);
