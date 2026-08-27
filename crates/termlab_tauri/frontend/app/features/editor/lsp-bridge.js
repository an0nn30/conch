// The sole frontend boundary for document ownership and language-service IPC.
(function initTermLabLspBridge(global) {
  'use strict';

  let windowLabel = null;
  let paneAccess = null;
  let reservationFailureHandler = null;
  let listening = false;
  let listenerEpoch = 0;
  const unlisteners = [];
  const reservationFailureRetries = new Map();

  function client() {
    return global.termlabServices && global.termlabServices.tauriClient;
  }

  function invoke(command, args) {
    const tauri = client();
    if (!tauri || typeof tauri.invoke !== 'function') {
      return Promise.reject(new Error('language service unavailable'));
    }
    return tauri.invoke(command, args);
  }

  function normalizeError(error, operation) {
    return {
      state: 'failed',
      operation: String(operation || 'language service'),
      message: 'Language features are unavailable; editing continues.',
      detail: error instanceof Error ? error.message : String(error),
    };
  }

  function findPane(paneId) {
    if (!paneAccess || typeof paneAccess.allPanes !== 'function') return null;
    const panes = paneAccess.allPanes();
    if (!panes || typeof panes.values !== 'function') return null;
    for (const pane of panes.values()) {
      if (pane && String(pane.paneId) === String(paneId)) return pane;
    }
    return null;
  }

  async function focusOwner(payload) {
    if (!payload || !payload.paneId) return false;
    if (payload.windowLabel && windowLabel && payload.windowLabel !== windowLabel) return false;
    const pane = findPane(payload.paneId);
    if (!pane) return false;
    const tauri = client();
    if (tauri && tauri.currentWindow && typeof tauri.currentWindow.setFocus === 'function') {
      await tauri.currentWindow.setFocus().catch(() => {});
    }
    if (typeof paneAccess.activateTab === 'function') paneAccess.activateTab(pane.tabId);
    if (typeof paneAccess.setFocusedPane === 'function') paneAccess.setFocusedPane(pane.paneId);
    if (pane.view && typeof pane.view.focus === 'function') pane.view.focus();
    return true;
  }

  function handleOwnershipEvent(payload) {
    if (payload && payload.reservationFailed) {
      if (reservationFailureHandler && payload.canonicalPath) {
        const canonicalPath = String(payload.canonicalPath);
        if (!reservationFailureRetries.has(canonicalPath)) {
          let retry = null;
          retry = Promise.resolve(reservationFailureHandler(canonicalPath))
            .catch(() => {})
            .finally(() => {
              if (reservationFailureRetries.get(canonicalPath) === retry) {
                reservationFailureRetries.delete(canonicalPath);
              }
            });
          reservationFailureRetries.set(canonicalPath, retry);
        }
      } else {
        console.warn('Document reservation failed before its waiting open could continue.');
      }
      return false;
    }
    return focusOwner(payload);
  }

  function refreshPane(status) {
    const state = global.termlabLspState;
    if (state && typeof state.updateStatus === 'function') state.updateStatus(status);
  }

  function startListeners() {
    if (listening) return;
    const tauri = client();
    if (!tauri) return;
    const listen = typeof tauri.listenOnCurrentWindow === 'function'
      ? tauri.listenOnCurrentWindow.bind(tauri)
      : (typeof tauri.listen === 'function' ? tauri.listen.bind(tauri) : null);
    if (!listen) return;
    listening = true;
    const epoch = ++listenerEpoch;
    const own = (promise) => {
      Promise.resolve(promise).then((unlisten) => {
        if (typeof unlisten !== 'function') return;
        if (!listening || epoch !== listenerEpoch) {
          unlisten();
          return;
        }
        unlisteners.push(unlisten);
      }).catch(() => {});
    };
    own(listen('lsp-session-status', (event) => refreshPane(event && event.payload)));
    own(listen('lsp-diagnostics-updated', (event) => {
      const state = global.termlabLspState;
      if (state && typeof state.updateDiagnostics === 'function') {
        state.updateDiagnostics(event && event.payload);
      }
    }));
    own(listen('editor-document-owner-focused', (event) => handleOwnershipEvent(event && event.payload)));
  }

  function configure(options) {
    const opts = options || {};
    if (opts.windowLabel) windowLabel = String(opts.windowLabel);
    if (opts.paneAccess) paneAccess = opts.paneAccess;
    if (typeof opts.onReservationFailed === 'function') {
      reservationFailureHandler = opts.onReservationFailed;
    }
    startListeners();
  }

  function dispose() {
    listenerEpoch += 1;
    listening = false;
    reservationFailureRetries.clear();
    while (unlisteners.length) {
      try { unlisteners.pop()(); } catch (_) {}
    }
  }

  const bridge = {
    configure,
    dispose,
    normalizeError,
    focusOwner,
    reserveDocument: (path) => invoke('editor_reserve_document', { path, windowLabel }),
    releaseDocument: (reservationId) => invoke('editor_release_document', { reservationId }),
    transferDocument: (documentId, targetReservationId, paneId) => invoke('editor_transfer_document', {
      documentId, targetReservationId, windowLabel, paneId: String(paneId),
    }),
    openDocument: (reservationId, paneId, contents, languageId) => invoke('lsp_open_document', {
      reservationId, paneId: String(paneId), contents, languageId,
    }),
    applyChanges: (documentId, batch) => invoke('lsp_apply_changes', { documentId, batch }),
    resyncDocument: (documentId, version, contents) => invoke('lsp_resync_document', {
      documentId, version, contents,
    }),
    didSave: (documentId) => invoke('lsp_did_save', { documentId }),
    closeDocument: (documentId) => invoke('lsp_close_document', { documentId }),
    closeDocuments: (documentIds) => invoke('lsp_close_documents', { documentIds }),
    projectCandidates: (path, languageId) => invoke('lsp_project_candidates', { path, languageId }),
    setProjectContext: (documentId, context) => invoke('lsp_set_project_context', { documentId, context }),
    setProjectTrust: (root, adapterId, decision) => invoke('lsp_set_project_trust', { root, adapterId, decision }),
    completion: (documentId, position, trigger) => invoke('lsp_completion', { documentId, position, trigger }),
    // `completionItem/resolve`. The item id carries its own document, version
    // and generation, so there is no position; `documentId` routes it to the
    // session that handed the item out.
    resolveCompletionItem: (documentId, itemId) => invoke('lsp_resolve_completion_item', {
      documentId, itemId,
    }),
    hover: (documentId, position) => invoke('lsp_hover', { documentId, position }),
    signatureHelp: (documentId, position, trigger) => invoke('lsp_signature_help', { documentId, position, trigger }),
    definition: (documentId, position) => invoke('lsp_definition', { documentId, position }),
    problemsSnapshot: (root) => invoke('lsp_problems_snapshot', { root: root || null }),
    statusSnapshot: (documentId) => invoke('lsp_status_snapshot', { documentId: documentId || null }),
    restartSession: (adapterId, root) => invoke('lsp_restart_session', { adapterId, root }),
    sessionLogs: (adapterId, root) => invoke('lsp_session_logs', { adapterId, root }),
    trustedProjects: () => invoke('lsp_trusted_projects'),
    revokeProjectTrust: (root, adapterId) => invoke('lsp_revoke_project_trust', { root, adapterId: adapterId || null }),
  };

  global.termlabLspBridge = bridge;
  if (typeof global.addEventListener === 'function') {
    global.addEventListener('unload', dispose, { once: true });
  }
})(window);
