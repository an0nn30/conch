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

  // ---------------------------------------------------------------------
  // FORWARD: parent -> every live host of that parent (panel_host_broadcast)
  // ---------------------------------------------------------------------

  const BRIDGE_EVENTS = Object.freeze(['active-pane-changed']);

  // Named accessor for the one event Task 5 wires end to end, so call sites
  // outside this module (the parent's publish calls, the host's dispatch
  // table) never have to retype the string literal above.
  const ACTIVE_PANE_CHANGED_EVENT = BRIDGE_EVENTS[0];

  // ---------------------------------------------------------------------
  // REVERSE: one host -> its own parent (panel_host_action)
  //
  // The set of actions a popped-out HOST may ask its PARENT to perform on
  // its behalf. A host mounts the exact same panel code a docked zone would
  // (panel-host-runtime.js's mountRegistration), but owns none of the
  // parent's singletons — no editor, no tab manager, nothing
  // manager-compose-runtime.js publishes, because that module only ever
  // composes for a real main window. An action a docked panel would just
  // perform locally (e.g. opening a double-clicked file in the editor) has
  // to cross back over IPC instead, through here.
  // ---------------------------------------------------------------------

  const HOST_ACTION_EVENTS = Object.freeze(['open-in-editor']);

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

    // Same discipline as publish() above, mirrored for the reverse
    // direction: a synchronous throw on an unlisted action name, else
    // fire-and-forget through `panel_host_action`
    // (crates/termlab_tauri/src/panel_host.rs) — a plain Tauri command with
    // no return value a caller acts on, so there is nothing useful to await
    // here either.
    function publishAction(event, payload) {
      if (!HOST_ACTION_EVENTS.includes(event)) {
        throw new Error(`panel-host-bridge: unlisted action "${event}" (must be one of ${HOST_ACTION_EVENTS.join(', ')})`);
      }
      if (typeof invoke !== 'function') return;
      invoke('panel_host_action', { event, payload }).catch(() => {});
    }

    return { publish, publishAction };
  }

  global.termlabPanelHostBridge = {
    BRIDGE_EVENTS,
    ACTIVE_PANE_CHANGED_EVENT,
    HOST_ACTION_EVENTS,
    create,
  };
})(window);
