// Notifications tool window — right-bottom zone.
//
// Wraps the entry-rendering logic that still lives in app/ui/notification-panel.js
// (window.notificationPanel.renderInto), which also keeps owning the toast-history
// live-update hook and calls refresh() on this module whenever a new notification
// arrives. The toolbar's Clear button follows the tunnels-panel.js pattern (a
// tl-toolwindow__toolbar row of tl-icon-btn buttons above a scrollable content div)
// — it's the "Clear notification history" affordance that used to live in the old
// bottom-panel #bottom-panel-actions area, now restored here since that area no
// longer applies once notifications isn't a bottom-panel tab.

(function (global) {
  'use strict';
  function create() {
    let panelEl = null;
    let contentEl = null;
    function init(deps) {
      panelEl = deps.panelEl;

      panelEl.innerHTML = `
        <div class="tl-toolwindow__toolbar" id="notifications-toolbar">
          <button class="tl-icon-btn" id="notifications-clear" title="Clear notification history"></button>
        </div>
        <div class="notifications-content tl-scroll" id="notifications-content"></div>
      `;

      contentEl = panelEl.querySelector('#notifications-content');
      const clearBtn = panelEl.querySelector('#notifications-clear');

      if (global.tlIcon) {
        clearBtn.appendChild(global.tlIcon.create('close', { size: 16 }));
      }

      clearBtn.addEventListener('click', () => {
        if (global.toast && global.toast.clearHistory) global.toast.clearHistory();
        refresh();
      });

      refresh();
    }
    function refresh() {
      if (!contentEl) return;
      global.notificationPanel.renderInto(contentEl);
    }
    return { init, refresh };
  }
  global.notificationsPanel = create();
})(window);
