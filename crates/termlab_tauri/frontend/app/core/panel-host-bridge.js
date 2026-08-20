// The parent-state event bridge: the CLOSED list of window-state events a
// parent window may broadcast to its own live panel-host windows, and the
// one function that ever calls `panel_host_broadcast`.
//
// `BRIDGE_EVENTS` is THE single source of truth for the event name(s) this
// bridge carries. Both halves that need to agree on it — this module's own
// `publish` validation, and the host-side re-dispatch table in
// app/panel-host-runtime.js — read it from here rather than each spelling
// the event name out as a second literal that could drift.
//
// `publish` is fire-and-forget by design: `panel_host_broadcast` is a plain
// Tauri command with no return value a caller acts on
// (crates/termlab_tauri/src/panel_host.rs), and it answers a parent with
// zero live hosts by iterating an empty `hosts_of_parent` Vec — a cheap
// no-op, not an error — so publishing from a window that has nothing popped
// out needs no guard here.
(function initTermLabPanelHostBridge(global) {
  'use strict';

  const BRIDGE_EVENTS = Object.freeze(['active-pane-changed']);

  // Named accessor for the one event Task 5 wires end to end, so call sites
  // outside this module (the parent's publish calls, the host's dispatch
  // table) never have to retype the string literal above.
  const ACTIVE_PANE_CHANGED_EVENT = BRIDGE_EVENTS[0];

  function create(deps) {
    const invoke = deps && deps.invoke;

    // Throws SYNCHRONOUSLY on an unlisted event name — a programmer error
    // (a call site that has not been added to BRIDGE_EVENTS), never
    // something to swallow into a rejected promise a fire-and-forget caller
    // was never going to check.
    function publish(event, payload) {
      if (!BRIDGE_EVENTS.includes(event)) {
        throw new Error(`panel-host-bridge: unlisted event "${event}" (must be one of ${BRIDGE_EVENTS.join(', ')})`);
      }
      if (typeof invoke !== 'function') return;
      invoke('panel_host_broadcast', { event, payload }).catch(() => {});
    }

    return { publish };
  }

  global.termlabPanelHostBridge = {
    BRIDGE_EVENTS,
    ACTIVE_PANE_CHANGED_EVENT,
    create,
  };
})(window);
