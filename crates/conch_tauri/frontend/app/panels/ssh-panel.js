// SSH Panel — server tree, quick connect, connection form, session management.

(function (exports) {
  'use strict';

  let invoke = null;
  let listen = null;
  let createSshTabFn = null;
  let panelEl = null;
  let panelWrapEl = null;
  let resizeHandleEl = null;
  let layoutService = null;
  const sshDataService = exports.conchSshFeatureDataService || {};
  const sshStore = exports.conchSshStore || {};
  const sshActions = exports.conchSshActions || {};
  const sshView = exports.conchSshView || {};
  const sshContextMenuFeature = exports.conchSshContextMenu || {};
  const sshAuthPromptsFeature = exports.conchSshAuthPrompts || {};
  const sshDialogsFeature = exports.conchSshDialogs || {};
  const sshDependencyPromptFeature = exports.conchSshDependencyPrompt || {};
  const sshConnectionFormFeature = exports.conchSshConnectionForm || {};
  let serverListEl = null;
  let quickConnectEl = null;
  let sessionListEl = null;
  let tunnelsSectionEl = null;
  let fitActiveTabFn = null;
  let refocusTerminalFn = null;

  // State
  let serverData = { folders: [], ungrouped: [], ssh_config: [] };
  let panelWasHiddenBeforeQuickConnect = false;
  let searchQuery = '';
  let searchSelectedIndex = 0;

  function setOverlayDialogAttributes(overlay, label) {
    if (!overlay) return;
    overlay.setAttribute('role', 'dialog');
    overlay.setAttribute('aria-modal', 'true');
    overlay.setAttribute('aria-label', String(label || 'Dialog'));
  }

  function registerOverlayKeys(overlay, name, onKeyDown) {
    const keyboardRouter = window.conchKeyboardRouter;
    if (keyboardRouter && typeof keyboardRouter.register === 'function') {
      return keyboardRouter.register({
        name: name || 'ssh-overlay',
        priority: 220,
        isActive: () => !!(overlay && overlay.isConnected),
        onKeyDown: (event) => {
          if (!overlay || !overlay.isConnected) return false;
          return onKeyDown(event) === true;
        },
      });
    }

    console.warn('ssh-panel: keyboard router unavailable, skipping overlay handler registration:', name || 'ssh-overlay');
    return () => {};
  }

  function invalidateCommandPaletteCache(reason) {
    if (typeof window.__conchInvalidateCommandPaletteCache === 'function') {
      window.__conchInvalidateCommandPaletteCache(reason || 'ssh-panel');
    }
  }

  function init(opts) {
    invoke = opts.invoke;
    listen = opts.listen;
    createSshTabFn = opts.createSshTab;
    fitActiveTabFn = opts.fitActiveTab;
    panelEl = opts.panelEl;
    panelWrapEl = opts.panelWrapEl;
    resizeHandleEl = opts.resizeHandleEl;
    layoutService = opts.layoutService
      || (window.conchServices && window.conchServices.layoutService)
      || null;
    refocusTerminalFn = opts.refocusTerminal;

    if (!panelEl) {
      console.warn('sshPanel.init called without a panel element');
      return;
    }

    panelEl.innerHTML = `
      <div class="ssh-panel-header">
        <span class="ssh-panel-title">Sessions</span>
        <div class="ssh-panel-actions">
          <div style="position:relative">
            <button class="ssh-icon-btn" id="ssh-add-new" title="New..."><svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" style="vertical-align:-2px"><path d="M 4 7 h 8 v 2 H 4 Z M 7 4 h 2 v 8 H 7 Z"/></svg></button>
          </div>
          <button class="ssh-icon-btn" id="ssh-refresh" title="Refresh"><svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" style="vertical-align:-2px"><path d="m 7.972 0 v 2 c -3.314 0 -6 2.686 -6 6 0 3.314 2.686 6 6 6 3.28 0 5.94 -2.633 5.994 -5.9 0.004 -0.033 0.006 -0.066 0.006 -0.1 0 -0.006 -0.004 -0.011 -0.004 -0.018 h -1.992 c 0 0.006 -0.004 0.011 -0.004 0.018 0 2.209 -1.791 4 -4 4 -2.209 0 -4 -1.791 -4 -4 0 -2.209 1.791 -4 4 -4 v 2 l 3.494 -3.018 z"/></svg></button>
        </div>
      </div>
      <div class="ssh-quick-connect">
        <input type="text" id="ssh-quick-connect-input"
               placeholder="Quick connect (user@host:port)"
               spellcheck="false" autocomplete="off" />
      </div>
      <div class="ssh-panel-body" id="ssh-panel-body">
        <div class="ssh-active-sessions" id="ssh-active-sessions"></div>
        <div class="ssh-tunnels-section" id="ssh-tunnels-section"></div>
        <div class="ssh-server-list" id="ssh-server-list"></div>
      </div>
    `;

    serverListEl = panelEl.querySelector('#ssh-server-list');
    quickConnectEl = panelEl.querySelector('#ssh-quick-connect-input');
    sessionListEl = panelEl.querySelector('#ssh-active-sessions');
    tunnelsSectionEl = panelEl.querySelector('#ssh-tunnels-section');

    // Quick connect input — filters server list + arrow key navigation
    quickConnectEl.addEventListener('input', () => {
      searchQuery = quickConnectEl.value.trim().toLowerCase();
      searchSelectedIndex = 0;
      renderServerList();
    });

    quickConnectEl.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        const query = quickConnectEl.value.trim();
        if (!query) return;

        const matches = getFilteredServers(query.toLowerCase());
        const idx = searchSelectedIndex;

        quickConnectEl.value = '';
        searchQuery = '';
        searchSelectedIndex = 0;
        quickConnectEl.blur();
        renderServerList();

        if (matches.length > 0) {
          const selected = matches[Math.min(idx, matches.length - 1)];
          createSshTabFn({ serverId: selected.id });
        } else {
          // No match — treat as user@host:port quick connect
          createSshTabFn({ spec: query });
        }
        return;
      }

      if (e.key === 'ArrowDown') {
        e.preventDefault();
        const matches = getFilteredServers(searchQuery);
        if (matches.length > 0) {
          searchSelectedIndex = Math.min(searchSelectedIndex + 1, matches.length - 1);
          renderServerList();
        }
        return;
      }

      if (e.key === 'ArrowUp') {
        e.preventDefault();
        searchSelectedIndex = Math.max(searchSelectedIndex - 1, 0);
        renderServerList();
        return;
      }

      if (e.key === 'Escape') {
        quickConnectEl.value = '';
        searchQuery = '';
        searchSelectedIndex = 0;
        renderServerList();
        quickConnectEl.blur();
        if (panelWasHiddenBeforeQuickConnect) {
          hidePanel();
          panelWasHiddenBeforeQuickConnect = false;
        }
        if (refocusTerminalFn) refocusTerminalFn();
      }
    });

    // Buttons
    panelEl.querySelector('#ssh-add-new').addEventListener('click', (e) => {
      e.stopPropagation();
      showNewMenu(panelEl.querySelector('#ssh-add-new'));
    });
    panelEl.querySelector('#ssh-refresh').addEventListener('click', refreshAll);

    // Auth prompts
    listen('ssh-host-key-prompt', handleHostKeyPrompt);
    listen('ssh-password-prompt', handlePasswordPrompt);

    // Vault auto-save prompt
    listen('vault-auto-save-prompt', handleVaultAutoSavePrompt);

    // Resize drag + state restore
    initResize();
    restoreLayout();

    refreshAll();
  }

  function hasPanelDom() {
    return !!(panelEl && serverListEl && sessionListEl && tunnelsSectionEl);
  }

  // ---------------------------------------------------------------------------
  // Panel visibility
  // ---------------------------------------------------------------------------

  function isHidden() {
    if (window.toolWindowManager) return !window.toolWindowManager.isVisible('ssh-sessions');
    if (!panelWrapEl) return true;
    return panelWrapEl.classList.contains('hidden');
  }

  function showPanel() {
    if (window.toolWindowManager) { window.toolWindowManager.activate('ssh-sessions'); return; }
    panelWrapEl.classList.remove('hidden');
    if (fitActiveTabFn) fitActiveTabFn();
    saveLayoutState();
  }

  function hidePanel() {
    if (window.toolWindowManager) { window.toolWindowManager.deactivate('ssh-sessions'); return; }
    panelWrapEl.classList.add('hidden');
    if (fitActiveTabFn) fitActiveTabFn();
    saveLayoutState();
  }

  function togglePanel() {
    if (window.toolWindowManager) { window.toolWindowManager.toggle('ssh-sessions'); return; }
    if (isHidden()) showPanel(); else hidePanel();
  }

  function focusQuickConnect() {
    panelWasHiddenBeforeQuickConnect = isHidden();
    if (isHidden()) showPanel();
    if (!quickConnectEl) return;
    quickConnectEl.focus();
    quickConnectEl.select();
  }

  function showNewMenu(anchorBtn) {
    const rect = anchorBtn.getBoundingClientRect();
    const fakeEvent = { clientX: rect.left, clientY: rect.bottom + 4 };
    showContextMenu(fakeEvent, [
      { label: 'New Connection', action: () => showConnectionForm() },
      { label: 'New Folder', action: () => showAddFolderDialog() },
      { label: 'New Tunnel', action: () => { if (window.tunnelManager) window.tunnelManager.show(); } },
    ]);
  }

  // ---------------------------------------------------------------------------
  // Resize drag
  // ---------------------------------------------------------------------------

  function initResize() {
    if (!resizeHandleEl) return;

    let dragging = false;
    let startX = 0;
    let startWidth = 0;

    // Prevent native drag-and-drop from hijacking the resize gesture.
    resizeHandleEl.addEventListener('dragstart', (e) => e.preventDefault());
    resizeHandleEl.style.touchAction = 'none';

    resizeHandleEl.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      resizeHandleEl.setPointerCapture(e.pointerId);
      dragging = true;
      startX = e.clientX;
      startWidth = panelEl.offsetWidth;
      resizeHandleEl.classList.add('dragging');
      document.body.style.cursor = 'col-resize';
      document.body.style.userSelect = 'none';
    });

    resizeHandleEl.addEventListener('pointermove', (e) => {
      if (!dragging) return;
      // Panel is on the right, so dragging left = bigger panel
      const delta = startX - e.clientX;
      const newWidth = Math.max(180, Math.min(500, startWidth + delta));
      panelEl.style.width = newWidth + 'px';
      if (fitActiveTabFn) fitActiveTabFn();
    });

    resizeHandleEl.addEventListener('pointerup', (e) => {
      if (!dragging) return;
      resizeHandleEl.releasePointerCapture(e.pointerId);
      dragging = false;
      resizeHandleEl.classList.remove('dragging');
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      saveLayoutState();
    });
  }

  // ---------------------------------------------------------------------------
  // State persistence
  // ---------------------------------------------------------------------------

  let saveTimeout = null;

  function saveLayoutState() {
    // Debounce saves
    if (saveTimeout) clearTimeout(saveTimeout);
    saveTimeout = setTimeout(() => {
      if (!invoke) return;
      const patch = {
        ssh_panel_width: panelEl.offsetWidth,
        ssh_panel_visible: !isHidden(),
      };
      if (layoutService && typeof layoutService.savePartialLayout === 'function') {
        layoutService.savePartialLayout(patch);
      } else {
        invoke('save_window_layout', { layout: patch }).catch(() => {});
      }
    }, 300);
  }

  async function restoreLayout() {
    // When TWM is active, sidebar width and visibility are managed centrally.
    if (window.toolWindowManager) return;
    try {
      const saved = layoutService && typeof layoutService.getSavedLayout === 'function'
        ? await layoutService.getSavedLayout()
        : await invoke('get_saved_layout');
      if (saved.ssh_panel_width > 100) {
        panelEl.style.width = saved.ssh_panel_width + 'px';
      }
      if (saved.ssh_panel_visible === false) {
        panelWrapEl.classList.add('hidden');
      } else {
        panelWrapEl.classList.remove('hidden');
      }
      if (fitActiveTabFn) setTimeout(fitActiveTabFn, 100);
    } catch (e) {
      console.error('Failed to restore layout:', e);
    }
  }

  // ---------------------------------------------------------------------------
  // Data
  // ---------------------------------------------------------------------------

  async function refreshAll() {
    try {
      if (!sshDataService || typeof sshDataService.getServers !== 'function') {
        throw new Error('SSH data service unavailable: getServers');
      }
      serverData = await sshDataService.getServers(invoke);
    } catch (e) {
      console.error('Failed to load servers:', e);
      serverData = { folders: [], ungrouped: [], ssh_config: [] };
    }
    invalidateCommandPaletteCache('ssh-refresh-all');
    if (!hasPanelDom()) return;
    renderServerList();
    await refreshSessions();
    await refreshTunnels();
  }

  async function exportConfig() {
    // Load current data for the selection form.
    let data;
    let tunnels;
    try {
      if (!sshDataService || typeof sshDataService.getServers !== 'function' || typeof sshDataService.getTunnels !== 'function') {
        throw new Error('SSH data service unavailable: getServers/getTunnels');
      }
      data = await sshDataService.getServers(invoke);
      tunnels = await sshDataService.getTunnels(invoke);
    } catch (e) {
      if (window.toast) window.toast.error('Export Failed', String(e));
      return;
    }

    removeOverlay();
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    setOverlayDialogAttributes(overlay, 'Export connections');

    // Build checkbox list HTML.
    let serversHtml = '';
    for (const folder of data.folders) {
      serversHtml += `<div class="ssh-export-group">${esc(folder.name)}</div>`;
      for (const s of folder.entries) {
        serversHtml += `<label class="ssh-export-item"><input type="checkbox" value="${esc(s.id)}" data-type="server" checked />${esc(s.label)} <span class="ssh-export-dim">(${esc(s.user)}@${esc(s.host)}:${s.port})</span></label>`;
      }
    }
    if (data.ungrouped.length) {
      serversHtml += `<div class="ssh-export-group">Ungrouped</div>`;
      for (const s of data.ungrouped) {
        serversHtml += `<label class="ssh-export-item"><input type="checkbox" value="${esc(s.id)}" data-type="server" checked />${esc(s.label)} <span class="ssh-export-dim">(${esc(s.user)}@${esc(s.host)}:${s.port})</span></label>`;
      }
    }
    if (data.ssh_config && data.ssh_config.length) {
      serversHtml += `<div class="ssh-export-group">~/.ssh/config</div>`;
      for (const s of data.ssh_config) {
        serversHtml += `<label class="ssh-export-item"><input type="checkbox" value="${esc(s.id)}" data-type="server" />${esc(s.label)} <span class="ssh-export-dim">(${esc(s.user)}@${esc(s.host)}:${s.port})</span></label>`;
      }
    }

    let tunnelsHtml = '';
    for (const t of tunnels) {
      tunnelsHtml += `<label class="ssh-export-item"><input type="checkbox" value="${esc(t.id)}" data-type="tunnel" checked />${esc(t.label)} <span class="ssh-export-dim">(L${t.local_port} → ${esc(t.remote_host)}:${t.remote_port})</span></label>`;
    }

    const hasServers = data.folders.some(f => f.entries.length) || data.ungrouped.length || (data.ssh_config && data.ssh_config.length);
    const hasTunnels = tunnels.length > 0;

    overlay.innerHTML = `
      <div class="ssh-form" style="min-width:400px;max-height:80vh;display:flex;flex-direction:column;">
        <div class="ssh-form-title">Export Connections</div>
        <div class="ssh-form-body" style="overflow-y:auto;flex:1;">
          <div style="margin-bottom:8px;">
            <label style="cursor:pointer;"><input type="checkbox" id="exp-select-all" checked /> Select All</label>
          </div>
          ${hasServers ? '<div class="ssh-export-section">Servers</div>' + serversHtml : ''}
          ${hasTunnels ? '<div class="ssh-export-section"' + (hasServers ? ' style="margin-top:12px;"' : '') + '>Tunnels</div>' + tunnelsHtml : ''}
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="exp-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="exp-export">Export</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);

    // Select All toggle
    const selectAll = overlay.querySelector('#exp-select-all');
    const allBoxes = () => overlay.querySelectorAll('input[data-type]');
    selectAll.addEventListener('change', () => {
      allBoxes().forEach(cb => cb.checked = selectAll.checked);
    });

    let closed = false;
    const closeExportDialog = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      removeOverlay();
    };
    const unregisterKeys = registerOverlayKeys(overlay, 'ssh-export-dialog', (event) => {
      if (event.key !== 'Escape') return false;
      closeExportDialog();
      return true;
    });
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) closeExportDialog(); });
    overlay.querySelector('#exp-cancel').addEventListener('click', closeExportDialog);

    // Build a lookup of all servers by their session key (user@host:port).
    const allServers = [];
    for (const f of data.folders) for (const s of f.entries) allServers.push(s);
    for (const s of data.ungrouped) allServers.push(s);
    if (data.ssh_config) for (const s of data.ssh_config) allServers.push(s);

    function serverSessionKey(s) { return s.user + '@' + s.host + ':' + s.port; }
    function findServerForTunnel(t) {
      return allServers.find(s => serverSessionKey(s) === t.session_key);
    }

    overlay.querySelector('#exp-export').addEventListener('click', async () => {
      let serverIds = [...overlay.querySelectorAll('input[data-type="server"]:checked')].map(cb => cb.value);
      const tunnelIds = [...overlay.querySelectorAll('input[data-type="tunnel"]:checked')].map(cb => cb.value);

      if (serverIds.length === 0 && tunnelIds.length === 0) {
        if (window.toast) window.toast.error('Export', 'Nothing selected');
        return;
      }

      const selectedServerIds = new Set(serverIds);

      // Check if selected items depend on servers not in the export.
      const selectedTunnels = tunnels.filter(t => tunnelIds.includes(t.id));
      const missingDependencies = [];
      for (const t of selectedTunnels) {
        const server = findServerForTunnel(t);
        if (server && !selectedServerIds.has(server.id)) {
          missingDependencies.push({
            reason: 'tunnel',
            sourceId: t.id,
            sourceLabel: t.label,
            server,
          });
        }
      }

      const selectedServers = allServers.filter((s) => selectedServerIds.has(s.id));
      for (const s of selectedServers) {
        if (!s.proxy_jump) continue;
        const depServer = findServerForProxyJump(s.proxy_jump, allServers);
        if (depServer && !selectedServerIds.has(depServer.id)) {
          missingDependencies.push({
            reason: 'proxy_jump',
            sourceId: s.id,
            sourceLabel: s.label,
            server: depServer,
          });
        }
      }

      const dedupedDependencies = dedupeDependencyServers(missingDependencies);
      if (dedupedDependencies.length > 0) {
        const shouldInclude = await showDependencyPrompt(dedupedDependencies);
        if (shouldInclude === null) return; // cancelled
        if (shouldInclude) {
          for (const dep of dedupedDependencies) {
            if (!selectedServerIds.has(dep.server.id)) {
              selectedServerIds.add(dep.server.id);
              serverIds.push(dep.server.id);
            }
          }
        }
      }

      closeExportDialog();
      try {
        if (!sshDataService || typeof sshDataService.exportSelection !== 'function') {
          throw new Error('SSH data service unavailable: exportSelection');
        }
        await sshDataService.exportSelection(invoke, serverIds, tunnelIds);
        if (window.toast) window.toast.info('Export', `Exported ${serverIds.length} server(s), ${tunnelIds.length} tunnel(s)`);
      } catch (e) {
        if (String(e) === 'Export cancelled') return;
        console.error('Export failed:', e);
        if (window.toast) window.toast.error('Export Failed', String(e));
      }
    });
  }


  function showDependencyPrompt(missingDependencies) {
    if (sshDependencyPromptFeature && typeof sshDependencyPromptFeature.showDependencyPrompt === 'function') {
      const delegated = sshDependencyPromptFeature.showDependencyPrompt(missingDependencies, {
        esc,
        setOverlayDialogAttributes,
        registerOverlayKeys,
      });
      if (delegated) return delegated;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Dependency prompt module is unavailable.');
    }
    return Promise.resolve(null);
  }

  async function importConfig() {
    try {
      if (!sshDataService || typeof sshDataService.importConfig !== 'function') {
        throw new Error('SSH data service unavailable: importConfig');
      }
      const msg = await sshDataService.importConfig(invoke);
      await refreshAll();
      if (window.toast) window.toast.info('Import', msg);
    } catch (e) {
      if (String(e) === 'Import cancelled') return;
      console.error('Import failed:', e);
      if (window.toast) window.toast.error('Import Failed', String(e));
    }
  }

  async function refreshSessions() {
    try {
      if (!sshDataService || typeof sshDataService.getSessions !== 'function') {
        throw new Error('SSH data service unavailable: getSessions');
      }
      const sessions = await sshDataService.getSessions(invoke);
      renderSessions(sessions);
    } catch (e) {
      console.error('Failed to load sessions:', e);
    }
  }

  // ---------------------------------------------------------------------------
  // Server filtering
  // ---------------------------------------------------------------------------

  function getAllServers() {
    if (!sshStore || typeof sshStore.getAllServers !== 'function') {
      console.error('ssh-store missing getAllServers');
      return [];
    }
    return sshStore.getAllServers(serverData);
  }

  function serverMatchesQuery(server, query) {
    if (!sshStore || typeof sshStore.serverMatchesQuery !== 'function') {
      console.error('ssh-store missing serverMatchesQuery');
      return true;
    }
    return sshStore.serverMatchesQuery(server, query);
  }

  function getFilteredServers(query) {
    if (!sshStore || typeof sshStore.getFilteredServers !== 'function') {
      console.error('ssh-store missing getFilteredServers');
      return [];
    }
    return sshStore.getFilteredServers(serverData, query);
  }

  function buildProxyJumpOptions(excludedServerId) {
    if (!sshStore || typeof sshStore.buildProxyJumpOptions !== 'function') {
      console.error('ssh-store missing buildProxyJumpOptions');
      return [];
    }
    return sshStore.buildProxyJumpOptions(serverData, excludedServerId);
  }

  function renderProxyJumpOptions(options) {
    if (!sshStore || typeof sshStore.renderProxyJumpOptions !== 'function') {
      console.error('ssh-store missing renderProxyJumpOptions');
      return '';
    }
    return sshStore.renderProxyJumpOptions(options, { esc, attr });
  }

  function parseProxyJump(value) {
    if (!sshStore || typeof sshStore.parseProxyJump !== 'function') {
      console.error('ssh-store missing parseProxyJump');
      return null;
    }
    return sshStore.parseProxyJump(value);
  }

  function normalizeProxyJump(value) {
    if (!sshStore || typeof sshStore.normalizeProxyJump !== 'function') {
      console.error('ssh-store missing normalizeProxyJump');
      return null;
    }
    return sshStore.normalizeProxyJump(value);
  }

  function makeProxyJumpSpec(server) {
    if (!sshStore || typeof sshStore.makeProxyJumpSpec !== 'function') {
      console.error('ssh-store missing makeProxyJumpSpec');
      return '';
    }
    return sshStore.makeProxyJumpSpec(server);
  }

  function findServerForProxyJump(proxyJumpValue, servers) {
    if (!sshStore || typeof sshStore.findServerForProxyJump !== 'function') {
      console.error('ssh-store missing findServerForProxyJump');
      return null;
    }
    return sshStore.findServerForProxyJump(proxyJumpValue, servers);
  }

  function dedupeDependencyServers(missingDependencies) {
    if (!sshStore || typeof sshStore.dedupeDependencyServers !== 'function') {
      console.error('ssh-store missing dedupeDependencyServers');
      return Array.isArray(missingDependencies) ? missingDependencies : [];
    }
    return sshStore.dedupeDependencyServers(missingDependencies);
  }

  // ---------------------------------------------------------------------------
  // Server tree rendering
  // ---------------------------------------------------------------------------

  function renderServerList() {
    if (!sshView || typeof sshView.renderServerList !== 'function') {
      console.error('ssh-view missing renderServerList');
      return;
    }
    sshView.renderServerList({
      serverListEl,
      serverData,
      searchQuery,
      searchSelectedIndex,
      getFilteredServers,
      esc,
      onFolderToggle: (folder, expanded) => {
        if (sshActions && typeof sshActions.setFolderExpanded === 'function') {
          sshActions.setFolderExpanded(invoke, folder.id, expanded).catch(() => {});
        }
        folder.expanded = expanded;
        renderServerList();
      },
      onFolderContextMenu: (event, folder) => showFolderContextMenu(event, folder),
      onServerContextMenu: (event, server, folderId) => showServerContextMenu(event, server, folderId),
      onServerDblClick: (server) => createSshTabFn({ serverId: server.id }),
    });
  }

  function renderSessions(sessions) {
    if (!sshView || typeof sshView.renderSessions !== 'function') {
      console.error('ssh-view missing renderSessions');
      return;
    }
    sshView.renderSessions(sessionListEl, sessions, { esc });
  }

  // ---------------------------------------------------------------------------
  // Tunnels section in sidebar
  // ---------------------------------------------------------------------------

  async function refreshTunnels() {
    let tunnels = [];
    try {
      if (!sshDataService || typeof sshDataService.getTunnels !== 'function') {
        throw new Error('SSH data service unavailable: getTunnels');
      }
      tunnels = await sshDataService.getTunnels(invoke);
    } catch (e) {
      console.error('Failed to load tunnels:', e);
    }
    invalidateCommandPaletteCache('ssh-refresh-tunnels');
    renderTunnels(tunnels);
  }

  function renderTunnels(tunnels) {
    if (!sshView || typeof sshView.renderTunnels !== 'function') {
      console.error('ssh-view missing renderTunnels');
      return;
    }
    sshView.renderTunnels(tunnelsSectionEl, tunnels, {
      esc,
      toast: window.toast,
      onStartTunnel: async (tunnel) => {
        if (!sshActions || typeof sshActions.startTunnel !== 'function') {
          throw new Error('SSH actions unavailable: startTunnel');
        }
        await sshActions.startTunnel(invoke, tunnel.id);
      },
      onStopTunnel: async (tunnel) => {
        if (!sshActions || typeof sshActions.stopTunnel !== 'function') {
          throw new Error('SSH actions unavailable: stopTunnel');
        }
        await sshActions.stopTunnel(invoke, tunnel.id);
      },
      onRefreshTunnels: refreshTunnels,
      onTunnelContextMenu: (event, tunnel, status) => showTunnelContextMenu(event, tunnel, status),
    });
  }

  function showTunnelContextMenu(e, tunnel, status) {
    const items = [];
    if (status === 'active' || status === 'connecting') {
      items.push({ label: 'Stop', action: async () => {
        if (!sshActions || typeof sshActions.stopTunnel !== 'function') return;
        try { await sshActions.stopTunnel(invoke, tunnel.id); } catch (err) { console.error(err); }
        setTimeout(refreshTunnels, 300);
      }});
    } else {
      items.push({ label: 'Start', action: async () => {
        if (!sshActions || typeof sshActions.startTunnel !== 'function') return;
        try { await sshActions.startTunnel(invoke, tunnel.id); } catch (err) { window.toast.error('Tunnel Error', String(err)); }
        setTimeout(refreshTunnels, 500);
      }});
    }
    items.push({ label: 'Edit', action: () => {
      if (window.tunnelManager) window.tunnelManager.showEdit(tunnel);
    }});
    items.push({ type: 'separator' });
    items.push({ label: 'Delete', danger: true, action: async () => {
      if (!sshActions || typeof sshActions.deleteTunnel !== 'function') return;
      try { await sshActions.deleteTunnel(invoke, tunnel.id); } catch (err) { console.error(err); }
      refreshTunnels();
    }});

    showContextMenu(e, items);
  }

  // ---------------------------------------------------------------------------
  // Connection form (modal overlay)
  // ---------------------------------------------------------------------------

  function showConnectionForm(existing, defaultFolderId) {
    if (sshConnectionFormFeature && typeof sshConnectionFormFeature.showConnectionForm === 'function') {
      const handled = sshConnectionFormFeature.showConnectionForm(existing, defaultFolderId, {
        removeOverlay,
        setOverlayDialogAttributes,
        registerOverlayKeys,
        serverData,
        buildProxyJumpOptions,
        renderProxyJumpOptions,
        normalizeProxyJump,
        attr,
        esc,
        invoke,
        refreshAll,
        createSshTab: createSshTabFn,
        toast: window.toast,
      });
      if (handled) return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Connection form module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // Folder dialog (inline prompt-style)
  // ---------------------------------------------------------------------------

  function showAddFolderDialog() {
    if (sshDialogsFeature && typeof sshDialogsFeature.showAddFolderDialog === 'function') {
      const handled = sshDialogsFeature.showAddFolderDialog({
        removeOverlay,
        setOverlayDialogAttributes,
        registerOverlayKeys,
        invoke,
        refreshAll,
        toast: window.toast,
      });
      if (handled) return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Folder dialog module is unavailable.');
    }
  }

  function showRenameFolderDialog(folder) {
    if (sshDialogsFeature && typeof sshDialogsFeature.showRenameFolderDialog === 'function') {
      const handled = sshDialogsFeature.showRenameFolderDialog(folder, {
        removeOverlay,
        setOverlayDialogAttributes,
        registerOverlayKeys,
        invoke,
        refreshAll,
        toast: window.toast,
        attr,
      });
      if (handled) return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Rename-folder dialog module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // Context menus
  // ---------------------------------------------------------------------------

  function showServerContextMenu(e, server, folderId) {
    showContextMenu(e, [
      { label: 'Connect', action: () => createSshTabFn({ serverId: server.id }) },
      { label: 'Edit', action: () => showConnectionForm(server, folderId) },
      { label: 'Duplicate', action: () => {
        if (!sshActions || typeof sshActions.duplicateServer !== 'function') return;
        sshActions.duplicateServer(invoke, server.id).then(() => refreshAll()).catch(() => {});
      }},
      { type: 'separator' },
      { label: 'Delete', danger: true, action: () => {
        showDeleteConfirmDialog(`Delete "${server.label}"?`, () => {
          if (!sshActions || typeof sshActions.deleteServer !== 'function') return;
          sshActions.deleteServer(invoke, server.id).then(() => refreshAll()).catch(() => {});
        });
      }},
    ]);
  }

  function showFolderContextMenu(e, folder) {
    showContextMenu(e, [
      { label: 'Add Server Here', action: () => showConnectionForm(null, folder.id) },
      { label: 'Rename', action: () => showRenameFolderDialog(folder) },
      { type: 'separator' },
      { label: 'Delete Folder', danger: true, action: () => {
        showDeleteConfirmDialog(`Delete folder "${folder.name}" and all servers in it?`, () => {
          if (!sshActions || typeof sshActions.deleteFolder !== 'function') return;
          sshActions.deleteFolder(invoke, folder.id).then(() => refreshAll()).catch(() => {});
        });
      }},
    ]);
  }

  function showDeleteConfirmDialog(message, onConfirm) {
    if (sshDialogsFeature && typeof sshDialogsFeature.showDeleteConfirmDialog === 'function') {
      const handled = sshDialogsFeature.showDeleteConfirmDialog(message, onConfirm, {
        removeOverlay,
        setOverlayDialogAttributes,
        registerOverlayKeys,
        esc,
      });
      if (handled) return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Delete-confirm dialog module is unavailable.');
    }
  }

  function showContextMenu(e, items) {
    if (sshContextMenuFeature && typeof sshContextMenuFeature.showContextMenu === 'function') {
      sshContextMenuFeature.showContextMenu(e, items, {
        onOpen: () => {},
      });
      return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Context-menu module is unavailable.');
    }
  }

  function removeContextMenu() {
    if (sshContextMenuFeature && typeof sshContextMenuFeature.removeContextMenu === 'function') {
      sshContextMenuFeature.removeContextMenu();
      return;
    }
  }

  function removeOverlay() {
    document.querySelectorAll('.ssh-overlay').forEach((el) => el.remove());
  }

  // ---------------------------------------------------------------------------
  // Auth prompts
  // ---------------------------------------------------------------------------

  function handleHostKeyPrompt(event) {
    if (sshAuthPromptsFeature && typeof sshAuthPromptsFeature.showHostKeyPrompt === 'function') {
      const handled = sshAuthPromptsFeature.showHostKeyPrompt(event, {
        invoke,
        esc,
        setOverlayDialogAttributes,
        registerOverlayKeys,
      });
      if (handled) return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Host-key prompt module is unavailable.');
    }
  }

  function handlePasswordPrompt(event) {
    if (sshAuthPromptsFeature && typeof sshAuthPromptsFeature.showPasswordPrompt === 'function') {
      const handled = sshAuthPromptsFeature.showPasswordPrompt(event, {
        invoke,
        esc,
        setOverlayDialogAttributes,
        registerOverlayKeys,
      });
      if (handled) return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Password prompt module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // Vault auto-save prompt
  // ---------------------------------------------------------------------------

  function handleVaultAutoSavePrompt(event) {
    const { server_id, server_label, host, username, auth_method } = event.payload;

    // Only show for password auth — key auth doesn't need saving.
    if (auth_method !== 'password') return;

    // Only show if vault module is available.
    if (!window.vault) return;

    window.toast.info(
      'Save to Vault?',
      `Save credentials for ${username}@${host} to the credential vault?`,
      {
        duration: 10000,
        action: {
          label: 'Save',
          callback: () => {
            window.vault.ensureUnlocked(() => {
              window.vault.showAccountForm({
                display_name: server_label || `${username}@${host}`,
                username: username,
                auth_type: 'password',
              });
            });
          },
        },
      }
    );
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  const esc = window.utils.esc;
  const attr = window.utils.attr;

  function getServerData() { return serverData; }

  exports.sshPanel = { init, refreshAll, refreshSessions, togglePanel, focusQuickConnect, isHidden, getServerData, exportConfig, importConfig };
})(window);
