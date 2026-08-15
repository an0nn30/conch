// Bottom panel with tabbed interface — plugin tabs only.
//
// The built-in Notifications tab that used to live here has moved to its own
// right-bottom tool window (see app/panels/notifications-panel.js). This module
// still owns the entry-rendering logic (exported as renderInto so the new tool
// window can reuse it) and the toast-history live-update hook, plus all of the
// plugin-tab machinery (addPluginTab/removePluginTab/updatePluginTab) that
// plugin-widgets.js relies on for bottom-location plugin panels.

(function (exports) {
  'use strict';

  let tabsEl = null;
  let actionsEl = null;
  let contentEl = null;
  let activeTabId = null;
  const pluginTabs = new Map();

  function init() {
    tabsEl = document.getElementById('bottom-panel-tabs');
    actionsEl = document.getElementById('bottom-panel-actions');
    contentEl = document.getElementById('bottom-panel-content');

    if (window.toast && window.toast.onNotification) {
      window.toast.onNotification(() => {
        if (window.notificationsPanel) window.notificationsPanel.refresh();
      });
    }
  }

  function addTab(id, label) {
    const btn = document.createElement('button');
    btn.className = 'bottom-tab';
    btn.textContent = label;
    btn.dataset.tabId = id;
    btn.addEventListener('click', () => activateTab(id));
    tabsEl.appendChild(btn);
  }

  function removeTab(id) {
    const btn = tabsEl.querySelector('[data-tab-id="' + id + '"]');
    if (btn) btn.remove();
    pluginTabs.delete(id);
    if (activeTabId === id) {
      const remaining = tabsEl.querySelector('.bottom-tab');
      if (remaining && remaining.dataset.tabId) {
        activateTab(remaining.dataset.tabId);
      } else {
        activeTabId = null;
        if (contentEl) contentEl.innerHTML = '';
      }
    }
  }

  function activateTab(id) {
    activeTabId = id;
    for (const btn of tabsEl.querySelectorAll('.bottom-tab')) {
      btn.classList.toggle('active', btn.dataset.tabId === id);
    }
    const plugin = pluginTabs.get(id);
    if (plugin && plugin.renderFn && contentEl) {
      contentEl.innerHTML = '';
      plugin.renderFn(contentEl);
    }
  }

  // Renders the notification-history entries into an arbitrary container.
  // Used by the notifications tool window (app/panels/notifications-panel.js);
  // this module no longer owns a bottom-panel tab for notifications itself.
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

  function addPluginTab(id, name, renderFn) {
    if (pluginTabs.has(id)) return;
    pluginTabs.set(id, { name, renderFn });
    addTab(id, name);
  }

  function removePluginTab(id) {
    removeTab(id);
  }

  function updatePluginTab(id, renderFn) {
    const plugin = pluginTabs.get(id);
    if (plugin) {
      plugin.renderFn = renderFn;
      if (activeTabId === id) activateTab(id);
    }
  }

  exports.notificationPanel = {
    init,
    activateTab,
    addPluginTab,
    removePluginTab,
    updatePluginTab,
    renderInto,
  };
})(window);
