// Tunnels Panel — standalone tool window listing SSH tunnels (start/stop/edit/delete).
//
// Reuses app/features/ssh/{view,data-service,actions,context-menu}.js and
// app/panels/tunnel-manager.js. This tool window previously lived as a section
// inside ssh-panel.js's sidebar; that wiring (removed in an earlier design-system
// task) is mirrored here, minus the sidebar-specific chrome.

(function (exports) {
  'use strict';

  const sshDataService = exports.termlabSshFeatureDataService || {};
  const sshActions = exports.termlabSshActions || {};
  const sshView = exports.termlabSshView || {};
  const sshContextMenuFeature = exports.termlabSshContextMenu || {};

  const esc = (exports.utils && exports.utils.esc) || ((value) => String(value == null ? '' : value));

  let invoke = null;
  let panelEl = null;
  let listEl = null;
  let refreshTimer = null;

  function init(opts) {
    // opts.listen (listenOnCurrentWindow) is accepted for consistency with the other
    // tool-window init() calls, but unused: no backend event exists for tunnel status
    // changes (see note below), so there is nothing to subscribe to yet.
    invoke = (opts && opts.invoke) || null;
    panelEl = opts && opts.panelEl;

    if (!panelEl) {
      console.warn('tunnelsPanel.init called without a panel element');
      return;
    }

    panelEl.innerHTML = `
      <div class="tl-toolwindow__toolbar" id="tunnels-toolbar">
        <button class="tl-icon-btn" id="tunnel-add" title="New Tunnel"></button>
        <button class="tl-icon-btn" id="tunnel-manage" title="Edit Tunnels"></button>
        <button class="tl-icon-btn" id="tunnel-refresh" title="Refresh"></button>
      </div>
      <div class="tunnels-list tl-scroll" id="tunnels-list"></div>
    `;

    listEl = panelEl.querySelector('#tunnels-list');
    const addBtn = panelEl.querySelector('#tunnel-add');
    const manageBtn = panelEl.querySelector('#tunnel-manage');
    const refreshBtn = panelEl.querySelector('#tunnel-refresh');

    if (exports.tlIcon) {
      addBtn.appendChild(exports.tlIcon.create('add', { size: 16 }));
      manageBtn.appendChild(exports.tlIcon.create('edit', { size: 16 }));
      refreshBtn.appendChild(exports.tlIcon.create('refresh', { size: 16 }));
    }

    // tunnel-manager.js only exports { init, show, showEdit, showError } — there is
    // no showNewTunnelForm entry point, so "New Tunnel" opens the same manager
    // dialog as "Edit Tunnels"; the dialog has its own add-tunnel affordance.
    addBtn.addEventListener('click', () => {
      if (exports.tunnelManager) exports.tunnelManager.show();
    });
    manageBtn.addEventListener('click', () => {
      if (exports.tunnelManager) exports.tunnelManager.show();
    });
    refreshBtn.addEventListener('click', refresh);

    // No backend event for tunnel status changes exists (checked crates/termlab_tauri/src
    // for `emit.*tunnel` — none found), so this relies on the polling interval below plus
    // the post-action refreshes triggered from each tunnel node's own handlers.

    refresh();
    if (refreshTimer) clearInterval(refreshTimer);
    refreshTimer = setInterval(refresh, 15000);
  }

  async function refresh() {
    if (!panelEl || !listEl || !invoke) return;
    if (typeof sshDataService.getTunnels !== 'function') {
      console.error('tunnelsPanel: SSH data service unavailable: getTunnels');
      return;
    }

    let tunnels = [];
    try {
      tunnels = await sshDataService.getTunnels(invoke);
    } catch (e) {
      console.error('tunnelsPanel: failed to load tunnels:', e);
      return;
    }

    listEl.innerHTML = '';

    if (!tunnels.length) {
      const empty = document.createElement('div');
      empty.className = 'tl-empty-state';
      empty.textContent = 'Nothing to show';
      listEl.appendChild(empty);
      return;
    }

    if (typeof sshView.createTunnelNode !== 'function') {
      console.error('tunnelsPanel: ssh-view missing createTunnelNode');
      return;
    }

    const frag = document.createDocumentFragment();
    for (const tunnel of tunnels) {
      frag.appendChild(sshView.createTunnelNode(tunnel, {
        esc,
        toast: exports.toast,
        onStartTunnel: async (t) => {
          if (typeof sshActions.startTunnel !== 'function') {
            throw new Error('SSH actions unavailable: startTunnel');
          }
          await sshActions.startTunnel(invoke, t.id);
        },
        onStopTunnel: async (t) => {
          if (typeof sshActions.stopTunnel !== 'function') {
            throw new Error('SSH actions unavailable: stopTunnel');
          }
          await sshActions.stopTunnel(invoke, t.id);
        },
        onRefreshTunnels: refresh,
        onTunnelContextMenu: (event, t, status) => showTunnelContextMenu(event, t, status),
      }));
    }
    listEl.appendChild(frag);
  }

  function showTunnelContextMenu(e, tunnel, status) {
    if (typeof sshContextMenuFeature.showContextMenu !== 'function') {
      console.error('tunnelsPanel: ssh context-menu feature unavailable');
      return;
    }

    const items = [];
    if (status === 'active' || status === 'connecting') {
      items.push({
        label: 'Stop',
        action: async () => {
          if (typeof sshActions.stopTunnel !== 'function') return;
          try { await sshActions.stopTunnel(invoke, tunnel.id); } catch (err) { console.error(err); }
          setTimeout(refresh, 300);
        },
      });
    } else {
      items.push({
        label: 'Start',
        action: async () => {
          if (typeof sshActions.startTunnel !== 'function') return;
          try {
            await sshActions.startTunnel(invoke, tunnel.id);
          } catch (err) {
            if (exports.toast) exports.toast.error('Tunnel Error', String(err));
          }
          setTimeout(refresh, 500);
        },
      });
    }
    items.push({
      label: 'Edit',
      action: () => {
        if (exports.tunnelManager) exports.tunnelManager.showEdit(tunnel);
      },
    });
    items.push({ type: 'separator' });
    items.push({
      label: 'Delete',
      danger: true,
      action: async () => {
        if (typeof sshActions.deleteTunnel !== 'function') return;
        try { await sshActions.deleteTunnel(invoke, tunnel.id); } catch (err) { console.error(err); }
        refresh();
      },
    });

    sshContextMenuFeature.showContextMenu(e, items);
  }

  exports.tunnelsPanel = { init, refresh };
})(window);
