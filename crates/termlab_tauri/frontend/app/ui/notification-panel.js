// Notification history rendering — the old tabbed bottom panel (with its
// addTab/removeTab/plugin-tab machinery) is gone; the bottom zone now hosts
// plugin panels as ordinary tool windows (see tool-window-runtime.js), and
// the built-in Notifications UI lives in its own right-bottom tool window
// (app/panels/notifications-panel.js). This module only keeps the toast-
// history live-update hook and the entry-rendering logic (exported as
// renderInto so the notifications tool window can reuse it).

(function (exports) {
  'use strict';

  function init() {
    if (window.toast && window.toast.onNotification) {
      window.toast.onNotification(() => {
        if (window.notificationsPanel) window.notificationsPanel.refresh();
      });
    }
  }

  // Renders the notification-history entries into an arbitrary container.
  // Used by the notifications tool window (app/panels/notifications-panel.js).
  function renderInto(containerEl) {
    if (!containerEl) return;
    containerEl.innerHTML = '';

    const history = (window.toast && window.toast.getHistory) ? window.toast.getHistory() : [];
    if (history.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'tl-empty-state notif-empty';
      empty.textContent = 'Nothing to show';
      containerEl.appendChild(empty);
      return;
    }

    const frag = document.createDocumentFragment();
    for (const entry of history) {
      const row = document.createElement('div');
      row.className = 'notif-entry';

      const time = document.createElement('span');
      time.className = 'notif-time';
      const d = entry.timestamp;
      time.textContent = String(d.getHours()).padStart(2, '0') + ':' +
        String(d.getMinutes()).padStart(2, '0') + ':' +
        String(d.getSeconds()).padStart(2, '0');
      row.appendChild(time);

      const dot = document.createElement('span');
      dot.className = 'notif-dot notif-dot-' + (entry.level || 'info');
      row.appendChild(dot);

      const text = document.createElement('span');
      text.className = 'notif-text';
      const title = document.createElement('span');
      title.className = 'notif-title';
      title.textContent = entry.title || '';
      text.appendChild(title);
      if (entry.body) {
        const body = document.createElement('span');
        body.className = 'notif-body';
        body.textContent = entry.body;
        text.appendChild(body);
      }
      row.appendChild(text);

      frag.appendChild(row);
    }
    containerEl.appendChild(frag);
  }

  exports.notificationPanel = {
    init,
    renderInto,
  };
})(window);
