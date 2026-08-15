// Notifications tool window — right-bottom zone.
//
// Thin wrapper around the entry-rendering logic that still lives in
// app/ui/notification-panel.js (window.notificationPanel.renderInto), which
// also keeps owning the toast-history live-update hook and calls refresh()
// on this module whenever a new notification arrives.

(function (global) {
  'use strict';
  function create() {
    let panelEl = null;
    function init(deps) {
      panelEl = deps.panelEl;
      panelEl.classList.add('tl-scroll');
      refresh();
    }
    function refresh() {
      if (!panelEl) return;
      global.notificationPanel.renderInto(panelEl);
    }
    return { init, refresh };
  }
  global.notificationsPanel = create();
})(window);
