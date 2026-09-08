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
  // Pane -> { revision, signature } for the marks that pane is currently
  // showing. The revision is the staleness guard; the signature is what makes
  // a re-delivery of unchanged marks free (see applyToPane).
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
  // One implementation, in lsp-position.js: the clamping rules for a line past
  // the end of the document (CodeMirror throws rather than saturating) are the
  // kind of thing that must not exist in two versions.

  function positions() {
    return global.termlabLspPosition || null;
  }

  function spanOf(doc, range) {
    const helper = positions();
    return helper ? helper.spanOf(doc, range) : { from: 0, to: 0 };
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
      // A server may report end before start after a race; spanOf collapses
      // such a range rather than handing CodeMirror a negative length.
      const span = spanOf(doc, item.range);
      const diagnostic = {
        from: span.from,
        to: span.to,
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

  // Everything a rendered mark actually shows, in one comparable string. Not
  // the CodeMirror diagnostics themselves: each carries a fresh renderMessage
  // closure, so two identical publications are never deep-equal as objects.
  function signatureOf(diagnostics, items) {
    const parts = [];
    for (let i = 0; i < diagnostics.length; i += 1) {
      const rendered = diagnostics[i];
      const source = items[i];
      parts.push([
        rendered.from,
        rendered.to,
        rendered.severity,
        rendered.message,
        source.source || '',
        source.code === null || source.code === undefined ? '' : source.code,
      ].join('␟'));
    }
    return parts.join('␞');
  }

  function applyToPane(pane, update, revision) {
    const CM = cm();
    if (!CM || typeof CM.setDiagnostics !== 'function') return false;
    const applied = appliedRevisions.get(pane);
    if (applied !== undefined && revision <= applied.revision) return false;
    const filePath = String(pane.filePath);
    const items = (update.snapshot && update.snapshot.items) || [];
    const mine = items.filter((item) => item && uriToPath(item.uri) === filePath);
    // An event that names another file and carries nothing for this one is
    // not this pane's news: dispatching an identical empty set would cost a
    // transaction and, worse, claim this pane is up to date at that revision
    // when the next event for it may still be older.
    if (!mine.length && update.uri && uriToPath(update.uri) !== filePath) return false;
    const diagnostics = toCodeMirrorDiagnostics(pane.view.state.doc, mine);
    const signature = signatureOf(diagnostics, mine);
    // Rust publishes the WHOLE workspace snapshot every time, so a keystroke
    // in one file re-delivers every other open file's marks unchanged. Those
    // panes are up to date at the new revision — they just have nothing to
    // redraw, and a setDiagnostics transaction would rebuild the whole
    // decoration set to arrive back where it started.
    if (applied !== undefined && applied.signature === signature) {
      appliedRevisions.set(pane, { revision, signature });
      return false;
    }
    appliedRevisions.set(pane, { revision, signature });
    pane.view.dispatch(CM.setDiagnostics(pane.view.state, diagnostics));
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
    appliedRevision: (pane) => {
      const applied = appliedRevisions.get(pane);
      return applied ? applied.revision : undefined;
    },
  };
})(window);
