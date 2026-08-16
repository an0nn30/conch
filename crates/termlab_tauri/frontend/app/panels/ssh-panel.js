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
  const sshDataService = exports.termlabSshFeatureDataService || {};
  const sshStore = exports.termlabSshStore || {};
  const sshActions = exports.termlabSshActions || {};
  const sshView = exports.termlabSshView || {};
  const sshAuthPromptsFeature = exports.termlabSshAuthPrompts || {};
  const sshDialogsFeature = exports.termlabSshDialogs || {};
  const sshDependencyPromptFeature = exports.termlabSshDependencyPrompt || {};
  const sshConnectionFormFeature = exports.termlabSshConnectionForm || {};
  let serverListEl = null;
  let quickConnectEl = null;
  let sessionListEl = null;
  let editHostBtn = null;
  let removeHostBtn = null;
  let configToggleBtn = null;
  let fitActiveTabFn = null;
  let refocusTerminalFn = null;

  // State
  let serverData = { folders: [], ungrouped: [], ssh_config: [] };
  let panelWasHiddenBeforeQuickConnect = false;
  let searchQuery = '';
  let searchSelectedIndex = 0;
  let selectedServer = null;
  // Preserves the pre-Phase-2 behavior of always listing ~/.ssh/config hosts by
  // default; the toggle button lets the user *hide* them instead of show them.
  let showSshConfigHosts = true;
  // The tl-dialog handle for whichever dialog this panel most recently
  // opened (connection form, add/rename-folder, delete-confirm, export).
  // removeOverlay() below closes only this — never a blanket DOM query —
  // so it can't reach into keygen/files-panel/plugin/vault/tunnel dialogs.
  let activeDialogHandle = null;

  function invalidateCommandPaletteCache(reason) {
    if (typeof window.__termlabInvalidateCommandPaletteCache === 'function') {
      window.__termlabInvalidateCommandPaletteCache(reason || 'ssh-panel');
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
      || (window.termlabServices && window.termlabServices.layoutService)
      || null;
    refocusTerminalFn = opts.refocusTerminal;

    if (!panelEl) {
      console.warn('sshPanel.init called without a panel element');
      return;
    }

    panelEl.innerHTML = `
      <div class="tl-toolwindow__toolbar" id="hosts-toolbar">
        <button class="tl-icon-btn" id="ssh-add-new" title="New Host"></button>
        <button class="tl-icon-btn" id="ssh-edit-host" title="Edit Host"></button>
        <button class="tl-icon-btn" id="ssh-remove-host" title="Delete Host"></button>
        <button class="tl-icon-btn" id="ssh-refresh" title="Refresh"></button>
        <button class="tl-icon-btn" id="ssh-config-toggle" title="Show ~/.ssh/config hosts"></button>
        <input id="ssh-quick-connect-input" class="tl-input ssh-quick-connect-input"
               placeholder="Quick connect (user@host:port)"
               spellcheck="false" autocomplete="off" />
      </div>
      <div class="ssh-panel-body tl-scroll" id="ssh-panel-body">
        <div class="ssh-active-sessions" id="ssh-active-sessions"></div>
        <div class="ssh-server-list" id="ssh-server-list"></div>
      </div>
    `;

    serverListEl = panelEl.querySelector('#ssh-server-list');
    quickConnectEl = panelEl.querySelector('#ssh-quick-connect-input');
    sessionListEl = panelEl.querySelector('#ssh-active-sessions');
    editHostBtn = panelEl.querySelector('#ssh-edit-host');
    removeHostBtn = panelEl.querySelector('#ssh-remove-host');
    configToggleBtn = panelEl.querySelector('#ssh-config-toggle');

    if (window.tlIcon) {
      panelEl.querySelector('#ssh-add-new').appendChild(window.tlIcon.create('add', { size: 16 }));
      editHostBtn.appendChild(window.tlIcon.create('edit', { size: 16 }));
      removeHostBtn.appendChild(window.tlIcon.create('remove', { size: 16 }));
      panelEl.querySelector('#ssh-refresh').appendChild(window.tlIcon.create('refresh', { size: 16 }));
      configToggleBtn.appendChild(window.tlIcon.create('web', { size: 16 }));
    }

    editHostBtn.disabled = true;
    removeHostBtn.disabled = true;
    updateConfigToggleUI();

    // Selection — event delegation so it survives server-list re-renders.
    serverListEl.addEventListener('click', handleServerListClick);

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
    editHostBtn.addEventListener('click', () => {
      if (!selectedServer) return;
      showConnectionForm(selectedServer);
    });
    removeHostBtn.addEventListener('click', () => {
      if (!selectedServer) return;
      const server = selectedServer;
      showDeleteConfirmDialog(`Delete "${server.label}"?`, () => {
        if (!sshActions || typeof sshActions.deleteServer !== 'function') return;
        sshActions.deleteServer(invoke, server.id).then(() => {
          selectServer(null);
          refreshAll();
        }).catch(() => {});
      });
    });
    configToggleBtn.addEventListener('click', () => {
      showSshConfigHosts = !showSshConfigHosts;
      updateConfigToggleUI();
      renderServerList();
    });

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
    return !!(panelEl && serverListEl && sessionListEl);
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

  function updateConfigToggleUI() {
    if (!configToggleBtn) return;
    configToggleBtn.classList.toggle('active', showSshConfigHosts);
    configToggleBtn.title = showSshConfigHosts
      ? 'Hide ~/.ssh/config hosts'
      : 'Show ~/.ssh/config hosts';
  }

  function showNewMenu(anchorBtn) {
    const rect = anchorBtn.getBoundingClientRect();
    const fakeEvent = { clientX: rect.left, clientY: rect.bottom + 4 };
    showContextMenu(fakeEvent, [
      { icon: 'add', label: 'New Connection', action: () => showConnectionForm() },
      { icon: 'newFolder', label: 'New Folder', action: () => showAddFolderDialog() },
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
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') {
      if (window.toast) window.toast.error('Export Failed', 'Dialog shell unavailable.');
      return;
    }

    // Build checkbox list HTML.
    let serversHtml = '';
    for (const folder of data.folders) {
      serversHtml += `<div class="ssh-export-group">${esc(folder.name)}</div>`;
      for (const s of folder.entries) {
        serversHtml += `<label class="tl-check"><input type="checkbox" value="${esc(s.id)}" data-type="server" checked />${esc(s.label)} <span class="ssh-export-dim">(${esc(s.user)}@${esc(s.host)}:${s.port})</span></label>`;
      }
    }
    if (data.ungrouped.length) {
      serversHtml += `<div class="ssh-export-group">Ungrouped</div>`;
      for (const s of data.ungrouped) {
        serversHtml += `<label class="tl-check"><input type="checkbox" value="${esc(s.id)}" data-type="server" checked />${esc(s.label)} <span class="ssh-export-dim">(${esc(s.user)}@${esc(s.host)}:${s.port})</span></label>`;
      }
    }
    if (data.ssh_config && data.ssh_config.length) {
      serversHtml += `<div class="ssh-export-group">~/.ssh/config</div>`;
      for (const s of data.ssh_config) {
        serversHtml += `<label class="tl-check"><input type="checkbox" value="${esc(s.id)}" data-type="server" />${esc(s.label)} <span class="ssh-export-dim">(${esc(s.user)}@${esc(s.host)}:${s.port})</span></label>`;
      }
    }

    let tunnelsHtml = '';
    for (const t of tunnels) {
      tunnelsHtml += `<label class="tl-check"><input type="checkbox" value="${esc(t.id)}" data-type="tunnel" checked />${esc(t.label)} <span class="ssh-export-dim">(L${t.local_port} → ${esc(t.remote_host)}:${t.remote_port})</span></label>`;
    }

    const hasServers = data.folders.some(f => f.entries.length) || data.ungrouped.length || (data.ssh_config && data.ssh_config.length);
    const hasTunnels = tunnels.length > 0;

    // Build a lookup of all servers by their session key (user@host:port).
    const allServers = [];
    for (const f of data.folders) for (const s of f.entries) allServers.push(s);
    for (const s of data.ungrouped) allServers.push(s);
    if (data.ssh_config) for (const s of data.ssh_config) allServers.push(s);

    function serverSessionKey(s) { return s.user + '@' + s.host + ':' + s.port; }
    function findServerForTunnel(t) {
      return allServers.find(s => serverSessionKey(s) === t.session_key);
    }

    let handle = null;
    let closed = false;
    const closeExportDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'Export Connections',
      ariaLabel: 'Export connections',
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div style="margin-bottom:8px;">
            <label class="tl-check"><input type="checkbox" id="exp-select-all" checked /> Select All</label>
          </div>
          ${hasServers ? '<div class="ssh-export-section">Servers</div>' + serversHtml : ''}
          ${hasTunnels ? '<div class="ssh-export-section"' + (hasServers ? ' style="margin-top:12px;"' : '') + '>Tunnels</div>' + tunnelsHtml : ''}
        `;

        const selectAll = bodyEl.querySelector('#exp-select-all');
        const allBoxes = () => bodyEl.querySelectorAll('input[data-type]');
        selectAll.addEventListener('change', () => {
          allBoxes().forEach(cb => cb.checked = selectAll.checked);
        });
      },
      buttons: [
        { label: 'Cancel', onSelect: closeExportDialog },
        { label: 'Export', primary: true, onSelect: async () => {
          const bodyEl = handle.el;
          let serverIds = [...bodyEl.querySelectorAll('input[data-type="server"]:checked')].map(cb => cb.value);
          const tunnelIds = [...bodyEl.querySelectorAll('input[data-type="tunnel"]:checked')].map(cb => cb.value);

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
        } },
      ],
      onClose: closeExportDialog,
    });
    activeDialogHandle = handle;
  }


  function showDependencyPrompt(missingDependencies) {
    if (sshDependencyPromptFeature && typeof sshDependencyPromptFeature.showDependencyPrompt === 'function') {
      const delegated = sshDependencyPromptFeature.showDependencyPrompt(missingDependencies, {
        esc,
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
    const dataForView = showSshConfigHosts ? serverData : { ...serverData, ssh_config: [] };
    sshView.renderServerList({
      serverListEl,
      serverData: dataForView,
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
    reapplySelection();
  }

  // ---------------------------------------------------------------------------
  // Selection state
  // ---------------------------------------------------------------------------

  function serverKey(s) { return `${s.user}@${s.host}:${s.port}`; }

  // Server tree nodes (see features/ssh/view.js#createServerNode) don't carry a
  // data-id attribute, but each node's title is the unique "user@host:port"
  // session key (same key format used by exportConfig's serverSessionKey /
  // findServerForTunnel above) — used here to resolve a clicked DOM node back
  // to its server record without modifying the view module.
  function findServerByNodeTitle(title) {
    const servers = getAllServers();
    return servers.find((s) => serverKey(s) === title) || null;
  }

  function handleServerListClick(event) {
    const node = event.target.closest('.ssh-server-node');
    serverListEl.querySelectorAll('.ssh-server-node.selected').forEach((el) => el.classList.remove('selected'));
    if (!node) {
      selectServer(null);
      return;
    }
    node.classList.add('selected');
    selectServer(findServerByNodeTitle(node.title));
  }

  function selectServer(server) {
    selectedServer = server || null;
    updateSelectionButtons();
  }

  function updateSelectionButtons() {
    const disabled = !selectedServer;
    if (editHostBtn) editHostBtn.disabled = disabled;
    if (removeHostBtn) removeHostBtn.disabled = disabled;
  }

  // Re-resolves the current selection against freshly-rendered DOM/data. Server
  // objects are wholesale-replaced on every refreshAll() (new objects from
  // remote_get_servers), so a stale `selectedServer` reference would let the
  // toolbar Edit button reopen outdated field values. Called after every
  // renderServerList() (data refresh, folder toggle, config-hosts toggle,
  // search) so it also re-applies the `.selected` class the DOM rebuild wipes.
  function reapplySelection() {
    if (!selectedServer) {
      updateSelectionButtons();
      return;
    }
    const fresh = getAllServers().find((s) => s.id === selectedServer.id) || null;
    selectedServer = fresh;
    updateSelectionButtons();
    if (!fresh || !serverListEl) return;
    const key = serverKey(fresh);
    const node = Array.from(serverListEl.querySelectorAll('.ssh-server-node')).find((n) => n.title === key);
    if (node) node.classList.add('selected');
  }

  function renderSessions(sessions) {
    if (!sshView || typeof sshView.renderSessions !== 'function') {
      console.error('ssh-view missing renderSessions');
      return;
    }
    sshView.renderSessions(sessionListEl, sessions, { esc });
  }

  // ---------------------------------------------------------------------------
  // Connection form (modal overlay)
  // ---------------------------------------------------------------------------

  function showConnectionForm(existing, defaultFolderId) {
    removeOverlay();
    if (sshConnectionFormFeature && typeof sshConnectionFormFeature.showConnectionForm === 'function') {
      const handle = sshConnectionFormFeature.showConnectionForm(existing, defaultFolderId, {
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
      if (handle) { activeDialogHandle = handle; return; }
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Connection form module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // Folder dialog (inline prompt-style)
  // ---------------------------------------------------------------------------

  function showAddFolderDialog() {
    removeOverlay();
    if (sshDialogsFeature && typeof sshDialogsFeature.showAddFolderDialog === 'function') {
      const handle = sshDialogsFeature.showAddFolderDialog({
        invoke,
        refreshAll,
        toast: window.toast,
      });
      if (handle) { activeDialogHandle = handle; return; }
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Folder dialog module is unavailable.');
    }
  }

  function showRenameFolderDialog(folder) {
    removeOverlay();
    if (sshDialogsFeature && typeof sshDialogsFeature.showRenameFolderDialog === 'function') {
      const handle = sshDialogsFeature.showRenameFolderDialog(folder, {
        invoke,
        refreshAll,
        toast: window.toast,
        attr,
      });
      if (handle) { activeDialogHandle = handle; return; }
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
      { icon: 'web', label: 'Connect', action: () => createSshTabFn({ serverId: server.id }) },
      { icon: 'edit', label: 'Edit', action: () => showConnectionForm(server, folderId) },
      { icon: 'copy', label: 'Duplicate', action: () => {
        if (!sshActions || typeof sshActions.duplicateServer !== 'function') return;
        sshActions.duplicateServer(invoke, server.id).then(() => refreshAll()).catch(() => {});
      }},
      { type: 'separator' },
      { icon: 'remove', label: 'Delete', danger: true, action: () => {
        showDeleteConfirmDialog(`Delete "${server.label}"?`, () => {
          if (!sshActions || typeof sshActions.deleteServer !== 'function') return;
          sshActions.deleteServer(invoke, server.id).then(() => refreshAll()).catch(() => {});
        });
      }},
    ]);
  }

  function showFolderContextMenu(e, folder) {
    showContextMenu(e, [
      { icon: 'add', label: 'Add Server Here', action: () => showConnectionForm(null, folder.id) },
      { icon: 'edit', label: 'Rename', action: () => showRenameFolderDialog(folder) },
      { type: 'separator' },
      { icon: 'remove', label: 'Delete Folder', danger: true, action: () => {
        showDeleteConfirmDialog(`Delete folder "${folder.name}" and all servers in it?`, () => {
          if (!sshActions || typeof sshActions.deleteFolder !== 'function') return;
          sshActions.deleteFolder(invoke, folder.id).then(() => refreshAll()).catch(() => {});
        });
      }},
    ]);
  }

  function showDeleteConfirmDialog(message, onConfirm) {
    removeOverlay();
    if (sshDialogsFeature && typeof sshDialogsFeature.showDeleteConfirmDialog === 'function') {
      const handle = sshDialogsFeature.showDeleteConfirmDialog(message, onConfirm, {
        esc,
      });
      if (handle) { activeDialogHandle = handle; return; }
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('SSH Error', 'Delete-confirm dialog module is unavailable.');
    }
  }

  // Hosts panel context menus (New / server / folder) render through the
  // shared window.tlMenu component (styles/design-system/components/menu.css,
  // app/ui/tl-menu.js) — same popup used by every other menu in the app,
  // including the tunnels row menu (tunnels-panel.js, tunnel-manager.js),
  // which used to go through the now-deleted app/features/ssh/context-menu.js.
  function showContextMenu(e, items) {
    if (!window.tlMenu || typeof window.tlMenu.open !== 'function') {
      if (window.toast && typeof window.toast.error === 'function') {
        window.toast.error('SSH Error', 'Menu module is unavailable.');
      }
      return;
    }
    const menuItems = (Array.isArray(items) ? items : []).map((item) => {
      if (item.type === 'separator') return { separator: true };
      return {
        label: item.label,
        icon: item.icon,
        disabled: item.disabled,
        danger: item.danger,
        title: item.title,
        onSelect: item.action,
      };
    });
    window.tlMenu.open({
      x: e.clientX,
      y: e.clientY,
      items: menuItems,
      ariaLabel: 'SSH context menu',
      routerName: 'ssh-panel-context-menu',
    });
  }

  function removeContextMenu() {
    if (window.tlMenu && typeof window.tlMenu.close === 'function') window.tlMenu.close();
  }

  function removeOverlay() {
    if (activeDialogHandle) {
      const handle = activeDialogHandle;
      activeDialogHandle = null;
      handle.close();
    }
  }

  // ---------------------------------------------------------------------------
  // Auth prompts
  // ---------------------------------------------------------------------------

  function handleHostKeyPrompt(event) {
    if (sshAuthPromptsFeature && typeof sshAuthPromptsFeature.showHostKeyPrompt === 'function') {
      const handled = sshAuthPromptsFeature.showHostKeyPrompt(event, {
        invoke,
        esc,
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

  exports.sshPanel = { init, refreshAll, refreshSessions, togglePanel, focusQuickConnect, isHidden, getServerData, exportConfig, importConfig, getSelectedServer: () => selectedServer };
})(window);
