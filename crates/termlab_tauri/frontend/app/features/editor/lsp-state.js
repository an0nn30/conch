// Per-pane language-service metadata. The CodeMirror document remains the
// only frontend text authority; this store deliberately never copies text.
(function initTermLabLspState(global) {
  'use strict';

  const byPane = new WeakMap();
  const byDocument = new Map();
  const subscribers = new Set();

  function notify(pane, state) {
    for (const subscriber of subscribers) {
      try { subscriber(pane, state); } catch (error) { console.error(error); }
    }
  }

  function selectedRoot(status, candidates) {
    if (status && status.projectRootUri) {
      const match = (candidates || []).find((candidate) => candidate.rootUri === status.projectRootUri);
      return match ? match.canonicalPath : null;
    }
    return null;
  }

  function attach(pane, opened) {
    if (!pane || !opened || !opened.documentId) return null;
    clear(pane);
    const candidates = Array.isArray(opened.projectCandidates) ? opened.projectCandidates.slice() : [];
    const status = opened.status || null;
    const state = {
      documentId: String(opened.documentId),
      version: Number.isInteger(opened.version) ? opened.version : 1,
      projectCandidates: candidates,
      selectedRoot: selectedRoot(status, candidates),
      trust: status && status.state === 'untrusted' ? 'untrusted' : null,
      capabilities: status && status.capabilities ? { ...status.capabilities } : {},
      status,
      diagnosticsRevision: 0,
    };
    byPane.set(pane, state);
    byDocument.set(state.documentId, pane);
    notify(pane, state);
    return state;
  }

  function get(pane) {
    return (pane && byPane.get(pane)) || null;
  }

  function clear(pane) {
    const state = get(pane);
    if (!state) return;
    if (byDocument.get(state.documentId) === pane) byDocument.delete(state.documentId);
    byPane.delete(pane);
    notify(pane, null);
  }

  function setVersion(pane, version) {
    const state = get(pane);
    if (!state || !Number.isInteger(version)) return;
    state.version = version;
  }

  function updateStatus(status) {
    if (!status || !status.documentId) return;
    const pane = byDocument.get(String(status.documentId));
    const state = get(pane);
    if (!state) return;
    if (state.status && Number(status.revision) < Number(state.status.revision)) return;
    state.status = status;
    state.capabilities = status.capabilities ? { ...status.capabilities } : {};
    state.selectedRoot = selectedRoot(status, state.projectCandidates);
    state.trust = status.state === 'untrusted' ? 'untrusted'
      : (status.projectRootUri ? 'trusted' : null);
    notify(pane, state);
  }

  function updateDiagnostics(update) {
    if (!update || !Number.isInteger(update.revision)) return;
    if (update.documentId) {
      const pane = byDocument.get(String(update.documentId));
      const state = get(pane);
      if (state && update.revision >= state.diagnosticsRevision) {
        state.diagnosticsRevision = update.revision;
        notify(pane, state);
      }
      return;
    }
    for (const pane of byDocument.values()) {
      const state = get(pane);
      if (state && update.revision >= state.diagnosticsRevision) {
        state.diagnosticsRevision = update.revision;
        notify(pane, state);
      }
    }
  }

  function subscribe(listener) {
    if (typeof listener !== 'function') return function () {};
    subscribers.add(listener);
    return () => subscribers.delete(listener);
  }

  global.termlabLspState = {
    attach,
    get,
    clear,
    setVersion,
    updateStatus,
    updateDiagnostics,
    subscribe,
  };
})(window);
