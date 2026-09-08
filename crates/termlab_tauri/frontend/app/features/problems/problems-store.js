// The window's live projection of Rust's diagnostic store, plus the filters
// the Problems view and F8 share.
//
// Window-wide on purpose. The Problems tool window is registered lazily — its
// renderFn only runs when its zone activates — but F8/Shift-F8 must work
// whether or not the panel has ever been on screen, so the snapshot cannot
// live inside the view.
//
// Nothing here is authoritative. Every publication carries the whole snapshot,
// so accepting one is a replacement, never a merge, and a publication whose
// revision is not newer than the one already held is dropped outright. That is
// the whole staleness protocol: no per-URI bookkeeping, no reconciliation.
(function initTermLabProblemsStore(global) {
  'use strict';

  const subscribers = new Set();

  let bridge = null;
  let unsubscribeDiagnostics = null;
  let unsubscribeStatus = null;
  let hydrating = null;

  const state = {
    hydrated: false,
    revision: -1,
    items: [],
    // Keyed by session identity so a re-published status replaces rather than
    // duplicates. `revision` on the status is what orders two of them.
    sessions: new Map(),
    filters: {
      severities: {
        error: true, warning: true, information: true, hint: true,
      },
      text: '',
    },
    selectedId: null,
  };

  let cachedView = null;

  function model() {
    return global.termlabProblemsModel || null;
  }

  function activeBridge() {
    return bridge || global.termlabLspBridge || null;
  }

  function sessionList() {
    return Array.from(state.sessions.values());
  }

  function invalidate() {
    cachedView = null;
  }

  function view() {
    const builder = model();
    if (!builder) return { groups: [], flat: [], counts: {}, totals: {} };
    if (!cachedView) {
      cachedView = builder.build({
        items: state.items,
        sessions: sessionList(),
        filters: state.filters,
      });
    }
    return cachedView;
  }

  function notify() {
    for (const subscriber of Array.from(subscribers)) {
      try { subscriber(getState()); } catch (error) { console.error(error); }
    }
  }

  function changed() {
    invalidate();
    notify();
  }

  function getState() {
    const builder = model();
    const built = view();
    return {
      hydrated: state.hydrated,
      revision: state.revision,
      items: state.items,
      sessions: sessionList(),
      filters: state.filters,
      selectedId: state.selectedId,
      view: built,
      panelState: builder
        ? builder.panelState({
          hydrated: state.hydrated,
          sessions: sessionList(),
          itemCount: state.items.length,
        })
        : 'loading',
    };
  }

  // --- ingestion --------------------------------------------------------------

  function applySnapshot(snapshot) {
    if (!snapshot) return false;
    const revision = Number(snapshot.revision);
    if (!Number.isFinite(revision) || revision <= state.revision) return false;
    state.revision = revision;
    state.items = Array.isArray(snapshot.items) ? snapshot.items.slice() : [];
    return true;
  }

  function applyDiagnostics(update) {
    if (!update || !update.snapshot) return false;
    // The event's own revision is the store revision; the nested snapshot
    // carries the same number, but the event is what Rust ordered.
    const revision = Number(update.revision);
    if (!Number.isFinite(revision) || revision <= state.revision) return false;
    state.revision = revision;
    state.items = Array.isArray(update.snapshot.items) ? update.snapshot.items.slice() : [];
    changed();
    return true;
  }

  function sessionKey(status) {
    if (status.sessionId) return `session:${status.sessionId}`;
    return `adapter:${status.adapterId || ''}@${status.projectRootUri || ''}`;
  }

  function applyStatus(status) {
    // A status with no project root is a per-document state (plain text,
    // choosing a project, disabled) and says nothing about which projects the
    // window is watching.
    if (!status || !status.projectRootUri) return false;
    const key = sessionKey(status);
    const previous = state.sessions.get(key);
    if (previous && Number(status.revision) < Number(previous.revision)) return false;
    // `stopped` is the session-level terminal status Rust emits when a session
    // ends (app-wide, `documentId` null). It is the ONLY thing that can retire
    // a group whose session died while failed: that session publishes no
    // further per-document status, so its group would otherwise sit on screen
    // explaining a failure nothing is still trying to recover from.
    if (status.state === 'stopped') {
      if (!previous) return false;
      state.sessions.delete(key);
      changed();
      return true;
    }
    state.sessions.set(key, status);
    changed();
    return true;
  }

  function applyStatuses(statuses) {
    let any = false;
    for (const status of statuses || []) {
      // A hydration snapshot lists live sessions only; a `stopped` record in
      // one would be a contradiction, and seeding it would resurrect the group
      // the event just retired.
      if (!status || !status.projectRootUri || status.state === 'stopped') continue;
      const key = sessionKey(status);
      const previous = state.sessions.get(key);
      if (previous && Number(status.revision) < Number(previous.revision)) continue;
      state.sessions.set(key, status);
      any = true;
    }
    return any;
  }

  // --- filters and selection --------------------------------------------------

  function setSeverityEnabled(severity, enabled) {
    if (state.filters.severities[severity] === undefined) return;
    state.filters.severities[severity] = enabled !== false;
    changed();
  }

  function setTextFilter(text) {
    state.filters.text = String(text === null || text === undefined ? '' : text);
    changed();
  }

  function orderedItems() {
    return view().flat || [];
  }

  function select(id) {
    state.selectedId = id === null || id === undefined ? null : String(id);
    notify();
  }

  // --- lifecycle --------------------------------------------------------------

  function subscribeToBridge() {
    const active = activeBridge();
    if (!active) return;
    // A popped-out Problems window never runs manager-compose-runtime, so
    // nothing else in it has asked the bridge to start listening — and
    // without that, a host would render whatever it hydrated with and then
    // go permanently stale. `configure` is idempotent and only overwrites
    // the options it is given, so calling it from here is safe in a main
    // window where compose has already configured it properly.
    if (typeof active.configure === 'function') active.configure({});
    if (!unsubscribeDiagnostics && typeof active.subscribeDiagnostics === 'function') {
      unsubscribeDiagnostics = active.subscribeDiagnostics((payload) => { applyDiagnostics(payload); });
    }
    if (!unsubscribeStatus && typeof active.subscribeStatus === 'function') {
      unsubscribeStatus = active.subscribeStatus((payload) => { applyStatus(payload); });
    }
  }

  // Reads the two snapshot commands once. Failure is not fatal: the window
  // stays in its "loading" state and the next publication fills it in, which
  // is the same degradation every other language-service surface takes.
  function hydrate() {
    if (hydrating) return hydrating;
    const active = activeBridge();
    if (!active || typeof active.problemsSnapshot !== 'function') {
      return Promise.resolve(getState());
    }
    hydrating = Promise.all([
      Promise.resolve(active.problemsSnapshot(null)).catch(() => null),
      typeof active.statusSnapshot === 'function'
        ? Promise.resolve(active.statusSnapshot(null)).catch(() => null)
        : Promise.resolve(null),
    ]).then((results) => {
      applySnapshot(results[0]);
      applyStatuses(results[1]);
      state.hydrated = true;
      changed();
      return getState();
    });
    return hydrating;
  }

  function subscribe(listener) {
    if (typeof listener !== 'function') return function () {};
    subscribers.add(listener);
    return () => subscribers.delete(listener);
  }

  function configure(options) {
    const opts = options || {};
    if (opts.bridge) bridge = opts.bridge;
    subscribeToBridge();
    if (opts.hydrate !== false) hydrate();
  }

  function dispose() {
    if (typeof unsubscribeDiagnostics === 'function') unsubscribeDiagnostics();
    if (typeof unsubscribeStatus === 'function') unsubscribeStatus();
    unsubscribeDiagnostics = null;
    unsubscribeStatus = null;
  }

  global.termlabProblemsStore = {
    configure,
    dispose,
    hydrate,
    subscribe,
    getState,
    view,
    orderedItems,
    setSeverityEnabled,
    setTextFilter,
    getFilters: () => state.filters,
    select,
    getSelectedId: () => state.selectedId,
    applyDiagnostics,
    applyStatus,
  };
})(window);
