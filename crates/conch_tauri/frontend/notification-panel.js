// Bottom panel with tabbed interface — built-in Notifications tab + plugin tabs.

(function (exports) {
  'use strict';

  let tabsEl = null;
  let actionsEl = null;
  let contentEl = null;
  let activeTabId = 'notifications';
  const pluginTabs = new Map();

  function init() {
    tabsEl = document.getElementById('bottom-panel-tabs');
    actionsEl = document.getElementById('bottom-panel-actions');
    contentEl = document.getElementById('bottom-panel-content');

    addTab('notifications', 'Notifications');
    activateTab('notifications');

    const clearBtn = document.createElement('button');
    clearBtn.className = 'bottom-panel-action-btn';
    clearBtn.textContent = 'Clear';
    clearBtn.title = 'Clear notification history';
    clearBtn.addEventListener('click', () => {
      if (window.toast && window.toast.clearHistory) window.toast.clearHistory();
    });
    actionsEl.appendChild(clearBtn);

    if (window.toast && window.toast.onNotification) {
      window.toast.onNotification((record) => {
        if (activeTabId === 'notifications') renderNotifications();
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
    if (activeTabId === id) activateTab('notifications');
  }

  function activateTab(id) {
    activeTabId = id;
    for (const btn of tabsEl.querySelectorAll('.bottom-tab')) {
      btn.classList.toggle('active', btn.dataset.tabId === id);
    }
    if (actionsEl) {
      actionsEl.style.display = id === 'notifications' ? '' : 'none';
    }
    if (id === 'notifications') {
      renderNotifications();
    } else {
      const plugin = pluginTabs.get(id);
      if (plugin && plugin.renderFn) {
        contentEl.innerHTML = '';
        plugin.renderFn(contentEl);
      }
    }
  }

  function renderNotifications() {
    if (!contentEl) return;
    contentEl.innerHTML = '';

    const history = (window.toast && window.toast.getHistory) ? window.toast.getHistory() : [];
    if (history.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'notif-empty';
      empty.textContent = 'No notifications yet.';
      contentEl.appendChild(empty);
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
    contentEl.appendChild(frag);
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
  };
})(window);
