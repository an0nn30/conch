// SSH Panel — server tree, quick connect, connection form, session management.

(function (exports) {
  'use strict';

  let invoke = null;
  let listen = null;
  let createSshTabFn = null;
  let panelEl = null;
  let panelWrapEl = null;
  let resizeHandleEl = null;
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

  function init(opts) {
    invoke = opts.invoke;
    listen = opts.listen;
    createSshTabFn = opts.createSshTab;
    fitActiveTabFn = opts.fitActiveTab;
    panelEl = opts.panelEl;
    panelWrapEl = opts.panelWrapEl;
    resizeHandleEl = opts.resizeHandleEl;
    refocusTerminalFn = opts.refocusTerminal;

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
      <div class="ssh-active-sessions" id="ssh-active-sessions"></div>
      <div class="ssh-tunnels-section" id="ssh-tunnels-section"></div>
      <div class="ssh-server-list" id="ssh-server-list"></div>
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

    // Global shortcuts
    document.addEventListener('keydown', handleGlobalKeydown);

    // Resize drag + state restore
    initResize();
    restoreLayout();

    refreshAll();
  }

  // ---------------------------------------------------------------------------
  // Panel visibility
  // ---------------------------------------------------------------------------

  function isHidden() {
    return panelWrapEl.classList.contains('hidden');
  }

  function showPanel() {
    panelWrapEl.classList.remove('hidden');
    if (fitActiveTabFn) fitActiveTabFn();
    saveLayoutState();
  }

  function hidePanel() {
    panelWrapEl.classList.add('hidden');
    if (fitActiveTabFn) fitActiveTabFn();
    saveLayoutState();
  }

  function togglePanel() {
    if (isHidden()) showPanel(); else hidePanel();
  }

  function focusQuickConnect() {
    panelWasHiddenBeforeQuickConnect = isHidden();
    if (isHidden()) showPanel();
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

  function handleGlobalKeydown(e) {
    // Keyboard shortcuts are now handled via native menu accelerators.
    // The menu emits events which are caught in the menu-action listener.
    // Keep Escape handling for the quick connect input (handled in its own listener).
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
      invoke('save_window_layout', {
        layout: {
          ssh_panel_width: panelEl.offsetWidth,
          ssh_panel_visible: !isHidden(),
        },
      }).catch(() => {});
    }, 300);
  }

  async function restoreLayout() {
    try {
      const saved = await invoke('get_saved_layout');
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
      serverData = await invoke('remote_get_servers');
    } catch (e) {
      console.error('Failed to load servers:', e);
      serverData = { folders: [], ungrouped: [], ssh_config: [] };
    }
    renderServerList();
    await refreshSessions();
    await refreshTunnels();
  }

  async function exportConfig() {
    // Load current data for the selection form.
    let data;
    let tunnels;
    try {
      data = await invoke('remote_get_servers');
      tunnels = await invoke('tunnel_get_all');
    } catch (e) {
      if (window.toast) window.toast.error('Export Failed', String(e));
      return;
    }

    removeOverlay();
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';

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

    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) removeOverlay(); });
    const onKey = (e) => { if (e.key === 'Escape') { removeOverlay(); document.removeEventListener('keydown', onKey); } };
    document.addEventListener('keydown', onKey);
    overlay.querySelector('#exp-cancel').addEventListener('click', removeOverlay);

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

      // Check if any selected tunnels depend on servers not in the export.
      const selectedTunnels = tunnels.filter(t => tunnelIds.includes(t.id));
      const missingServers = [];
      for (const t of selectedTunnels) {
        const server = findServerForTunnel(t);
        if (server && !serverIds.includes(server.id)) {
          missingServers.push({ tunnel: t, server });
        }
      }

      if (missingServers.length > 0) {
        const shouldInclude = await showDependencyPrompt(missingServers);
        if (shouldInclude === null) return; // cancelled
        if (shouldInclude) {
          for (const ms of missingServers) {
            if (!serverIds.includes(ms.server.id)) serverIds.push(ms.server.id);
          }
        }
      }

      removeOverlay();
      document.removeEventListener('keydown', onKey);
      try {
        await invoke('remote_export', { serverIds, tunnelIds });
        if (window.toast) window.toast.info('Export', `Exported ${serverIds.length} server(s), ${tunnelIds.length} tunnel(s)`);
      } catch (e) {
        if (String(e) === 'Export cancelled') return;
        console.error('Export failed:', e);
        if (window.toast) window.toast.error('Export Failed', String(e));
      }
    });
  }


  function showDependencyPrompt(missingServers) {
    return new Promise((resolve) => {
      const existing = document.querySelector('.ssh-overlay.dep-prompt');
      if (existing) existing.remove();

      const overlay = document.createElement('div');
      overlay.className = 'ssh-overlay dep-prompt';

      let listHtml = '';
      for (const ms of missingServers) {
        listHtml += `<div class="ssh-export-item" style="padding:2px 0;">
          <span>${esc(ms.tunnel.label)}</span>
          <span class="ssh-export-dim">\u2192 ${esc(ms.server.label)} (${esc(ms.server.user)}@${esc(ms.server.host)}:${ms.server.port})</span>
        </div>`;
      }

      overlay.innerHTML = `
        <div class="ssh-form" style="min-width:400px;">
          <div class="ssh-form-title">Include Server Connections?</div>
          <div class="ssh-form-body">
            <div style="margin-bottom:8px;font-size:12px;color:var(--fg);">
              The following tunnels depend on server connections that aren't in your export:
            </div>
            ${listHtml}
            <div style="margin-top:10px;font-size:11px;color:var(--dim-fg);">
              Without these servers, the tunnels may not work on the importing machine.
            </div>
          </div>
          <div class="ssh-form-buttons">
            <button class="ssh-form-btn" id="dep-cancel">Cancel</button>
            <button class="ssh-form-btn" id="dep-skip">Export Without</button>
            <button class="ssh-form-btn primary" id="dep-include">Include Servers</button>
          </div>
        </div>
      `;

      document.body.appendChild(overlay);
      overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) { overlay.remove(); resolve(null); } });
      overlay.querySelector('#dep-cancel').addEventListener('click', () => { overlay.remove(); resolve(null); });
      overlay.querySelector('#dep-skip').addEventListener('click', () => { overlay.remove(); resolve(false); });
      overlay.querySelector('#dep-include').addEventListener('click', () => { overlay.remove(); resolve(true); });

      const onKey = (e) => { if (e.key === 'Escape') { overlay.remove(); document.removeEventListener('keydown', onKey); resolve(null); } };
      document.addEventListener('keydown', onKey);
    });
  }

  async function importConfig() {
    try {
      const msg = await invoke('remote_import');
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
      const sessions = await invoke('remote_get_sessions');
      renderSessions(sessions);
    } catch (e) {
      console.error('Failed to load sessions:', e);
    }
  }

  // ---------------------------------------------------------------------------
  // Server filtering
  // ---------------------------------------------------------------------------

  function getAllServers() {
    const all = [];
    for (const f of serverData.folders) {
      for (const s of f.entries) all.push(s);
    }
    for (const s of serverData.ungrouped) all.push(s);
    for (const s of serverData.ssh_config) all.push(s);
    return all;
  }

  function serverMatchesQuery(server, query) {
    if (!query) return true;
    const hay = `${server.label} ${server.host} ${server.user}@${server.host}`.toLowerCase();
    return query.split(/\s+/).every((term) => hay.includes(term));
  }

  function getFilteredServers(query) {
    if (!query) return [];
    return getAllServers().filter((s) => serverMatchesQuery(s, query));
  }

  // ---------------------------------------------------------------------------
  // Server tree rendering
  // ---------------------------------------------------------------------------

  function renderServerList() {
    const frag = document.createDocumentFragment();

    if (searchQuery) {
      // Flat filtered list
      const matches = getFilteredServers(searchQuery);
      for (let i = 0; i < matches.length; i++) {
        frag.appendChild(createServerNode(matches[i], false, null, i === searchSelectedIndex));
      }
      if (matches.length === 0) {
        const hint = document.createElement('div');
        hint.className = 'ssh-search-hint';
        hint.textContent = 'No matches \u2014 Enter to quick-connect';
        frag.appendChild(hint);
      }
    } else {
      // SSH Sessions section header
      const hasServers = serverData.folders.length > 0
        || serverData.ungrouped.length > 0
        || serverData.ssh_config.length > 0;

      if (hasServers) {
        const sep = document.createElement('div');
        sep.className = 'ssh-panel-separator';
        frag.appendChild(sep);

        const headerRow = document.createElement('div');
        headerRow.className = 'ssh-tunnels-header';
        headerRow.innerHTML =
          `<span class="ssh-section-header-inline">SSH Sessions</span>`;
        frag.appendChild(headerRow);
      }

      // Folders
      for (const folder of serverData.folders) {
        frag.appendChild(createFolderNode(folder));
      }
      // Ungrouped servers
      for (const server of serverData.ungrouped) {
        frag.appendChild(createServerNode(server));
      }
      // ~/.ssh/config servers (dimmed)
      if (serverData.ssh_config.length > 0) {
        frag.appendChild(makeSectionHeader('~/.ssh/config'));
        for (const server of serverData.ssh_config) {
          frag.appendChild(createServerNode(server, true));
        }
      }
    }

    serverListEl.innerHTML = '';
    serverListEl.appendChild(frag);
  }

  function makeSectionHeader(text) {
    const el = document.createElement('div');
    el.className = 'ssh-section-header';
    el.textContent = text;
    return el;
  }

  function createFolderNode(folder) {
    const el = document.createElement('div');
    el.className = 'ssh-folder';

    const header = document.createElement('div');
    header.className = 'ssh-folder-header';
    const expanded = folder.expanded !== false;
    header.innerHTML =
      `<span class="ssh-folder-arrow">${expanded ? '▼' : '▶'}</span>` +
      `<span class="ssh-folder-name">${esc(folder.name)}</span>` +
      `<span class="ssh-folder-count">${folder.entries.length}</span>`;

    header.addEventListener('click', () => {
      invoke('remote_set_folder_expanded', { folderId: folder.id, expanded: !expanded }).catch(() => {});
      folder.expanded = !expanded;
      renderServerList();
    });

    header.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      showFolderContextMenu(e, folder);
    });

    el.appendChild(header);

    if (expanded) {
      const list = document.createElement('div');
      list.className = 'ssh-folder-entries';
      for (const server of folder.entries) {
        list.appendChild(createServerNode(server, false, folder.id));
      }
      el.appendChild(list);
    }

    return el;
  }

  function createServerNode(server, dimmed, folderId, highlighted) {
    const el = document.createElement('div');
    el.className = 'ssh-server-node' + (dimmed ? ' dimmed' : '') + (highlighted ? ' highlighted' : '');
    el.title = `${server.user}@${server.host}:${server.port}`;

    const label = server.label || `${server.user}@${server.host}`;
    const detail = server.host + (server.port !== 22 ? ':' + server.port : '');

    el.innerHTML =
      `<span class="ssh-server-label">${esc(label)}</span>` +
      `<span class="ssh-server-detail">${esc(detail)}</span>`;

    el.addEventListener('dblclick', () => createSshTabFn({ serverId: server.id }));

    el.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      showServerContextMenu(e, server, folderId);
    });

    return el;
  }

  function renderSessions(sessions) {
    sessionListEl.innerHTML = '';
    if (!sessions || sessions.length === 0) return;

    const frag = document.createDocumentFragment();
    frag.appendChild(makeSectionHeader('Active'));

    for (const s of sessions) {
      const el = document.createElement('div');
      el.className = 'ssh-session-node';
      el.innerHTML =
        `<span class="ssh-session-dot"></span>` +
        `<span class="ssh-session-label">${esc(s.user)}@${esc(s.host)}</span>`;
      frag.appendChild(el);
    }

    sessionListEl.appendChild(frag);
  }

  // ---------------------------------------------------------------------------
  // Tunnels section in sidebar
  // ---------------------------------------------------------------------------

  async function refreshTunnels() {
    let tunnels = [];
    try {
      tunnels = await invoke('tunnel_get_all');
    } catch (e) {
      console.error('Failed to load tunnels:', e);
    }
    renderTunnels(tunnels);
  }

  function renderTunnels(tunnels) {
    tunnelsSectionEl.innerHTML = '';
    if (tunnels.length === 0 && !tunnelsSectionEl.dataset.showEmpty) return;

    const frag = document.createDocumentFragment();

    // Separator + header
    const sep = document.createElement('div');
    sep.className = 'ssh-panel-separator';
    frag.appendChild(sep);

    const headerRow = document.createElement('div');
    headerRow.className = 'ssh-tunnels-header';
    headerRow.innerHTML =
      `<span class="ssh-section-header-inline">Tunnels</span>`;
    frag.appendChild(headerRow);

    for (const t of tunnels) {
      frag.appendChild(createTunnelNode(t));
    }

    if (tunnels.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'ssh-tunnel-empty';
      empty.textContent = 'No tunnels configured';
      frag.appendChild(empty);
    }

    tunnelsSectionEl.appendChild(frag);
  }

  function createTunnelNode(tunnel) {
    const el = document.createElement('div');
    el.className = 'ssh-tunnel-node';

    const status = tunnel.status || null;
    let dotClass = 'inactive';
    let errorMsg = null;
    if (status === 'active') dotClass = 'active';
    else if (status === 'connecting') dotClass = 'connecting';
    else if (status && status.startsWith('error')) {
      dotClass = 'error';
      errorMsg = status.replace(/^error:\s*/, '');
    }

    const isConnected = status === 'active' || status === 'connecting';
    const btnIcon = isConnected
      ? '<svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style="vertical-align:-2px"><path d="M 2 2 v 12 h 12 v -12 z"/></svg>'
      : '<svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style="vertical-align:-2px"><path d="M 3 2 v 12 l 11 -6 z"/></svg>';
    const btnTitle = isConnected ? 'Disconnect' : (errorMsg ? 'Retry' : 'Connect');

    el.innerHTML =
      `<span class="tunnel-dot ${dotClass}"></span>` +
      `<span class="ssh-tunnel-label">${esc(tunnel.label)}</span>` +
      (errorMsg ? `<span class="ssh-tunnel-error-indicator" title="Error: ${attr(errorMsg)}">!</span>` : '') +
      `<button class="ssh-tunnel-btn ssh-tunnel-edit-btn" title="Edit tunnel">Edit</button>` +
      `<button class="ssh-tunnel-btn ssh-tunnel-action-btn" title="${btnTitle}">${errorMsg ? 'Retry' : btnIcon}</button>`;

    if (errorMsg) el.title = 'Error: ' + errorMsg;
    el.querySelector('.ssh-tunnel-edit-btn').addEventListener('click', (e) => {
      e.stopPropagation();
      if (window.tunnelManager) window.tunnelManager.showEdit(tunnel);
    });

    const btn = el.querySelector('.ssh-tunnel-action-btn');
    btn.addEventListener('click', async (e) => {
      e.stopPropagation();
      btn.disabled = true;
      if (isConnected) {
        try {
          await invoke('tunnel_stop', { tunnelId: tunnel.id });
          window.toast.info('Tunnel Disconnected', tunnel.label);
        } catch (err) {
          window.toast.error('Tunnel Error', String(err));
        }
      } else {
        try {
          await invoke('tunnel_start', { tunnelId: tunnel.id });
          window.toast.success('Tunnel Connected', tunnel.label);
        } catch (err) {
          window.toast.error('Tunnel Error', String(err));
        }
      }
      setTimeout(refreshTunnels, 400);
    });

    // Right-click for context menu
    el.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      showTunnelContextMenu(e, tunnel, status);
    });

    return el;
  }

  function showTunnelContextMenu(e, tunnel, status) {
    const items = [];
    if (status === 'active' || status === 'connecting') {
      items.push({ label: 'Stop', action: async () => {
        try { await invoke('tunnel_stop', { tunnelId: tunnel.id }); } catch (err) { console.error(err); }
        setTimeout(refreshTunnels, 300);
      }});
    } else {
      items.push({ label: 'Start', action: async () => {
        try { await invoke('tunnel_start', { tunnelId: tunnel.id }); } catch (err) { window.toast.error('Tunnel Error', String(err)); }
        setTimeout(refreshTunnels, 500);
      }});
    }
    items.push({ label: 'Edit', action: () => {
      if (window.tunnelManager) window.tunnelManager.showEdit(tunnel);
    }});
    items.push({ type: 'separator' });
    items.push({ label: 'Delete', danger: true, action: async () => {
      try { await invoke('tunnel_delete', { tunnelId: tunnel.id }); } catch (err) { console.error(err); }
      refreshTunnels();
    }});

    showContextMenu(e, items);
  }

  // ---------------------------------------------------------------------------
  // Connection form (modal overlay)
  // ---------------------------------------------------------------------------

  function showConnectionForm(existing, defaultFolderId) {
    removeOverlay();

    const isEdit = !!existing;
    const title = isEdit ? 'Edit SSH Connection' : 'New SSH Connection';

    // Build folder options
    const folderOptions = [{ id: '', name: '(none)' }];
    for (const f of serverData.folders) {
      folderOptions.push({ id: f.id, name: f.name });
    }

    // Determine default folder
    let selectedFolder = defaultFolderId || '';
    if (isEdit && !selectedFolder) {
      for (const f of serverData.folders) {
        if (f.entries.some((e) => e.id === existing.id)) {
          selectedFolder = f.id;
          break;
        }
      }
    }

    // Determine proxy state
    let proxyType = 'none';
    let proxyValue = '';
    if (existing) {
      if (existing.proxy_jump) { proxyType = 'jump'; proxyValue = existing.proxy_jump; }
      else if (existing.proxy_command) { proxyType = 'command'; proxyValue = existing.proxy_command; }
    }

    const existingVaultId = existing ? (existing.vault_account_id || '') : '';

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.innerHTML = `
      <div class="ssh-form">
        <div class="ssh-form-title">${esc(title)}</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">Session Name
            <input type="text" id="cf-label" value="${attr(existing ? existing.label : '')}"
                   placeholder="optional" spellcheck="false" />
          </label>
          <div class="ssh-form-row">
            <label class="ssh-form-label" style="flex:1">Host / IP
              <input type="text" id="cf-host" value="${attr(existing ? existing.host : '')}"
                     placeholder="example.com" spellcheck="false" required />
            </label>
            <label class="ssh-form-label" style="width:80px">Port
              <input type="number" id="cf-port" value="${existing ? existing.port : 22}" min="1" max="65535" />
            </label>
          </div>
          <label class="ssh-form-label">Account
            <select id="cf-vault-account">
              <option value="">Manual credentials</option>
              <option value="__create__">+ Create New Account...</option>
            </select>
          </label>
          <div id="cf-vault-account-info" style="display:none;padding:6px 8px;border-radius:4px;background:var(--bg);border:1px solid var(--selection);margin-bottom:8px;font-size:12px"></div>
          <div id="cf-manual-creds">
            <label class="ssh-form-label">Username
              <input type="text" id="cf-user" value="${attr(existing ? existing.user : '')}"
                     placeholder="root" spellcheck="false" />
            </label>
            <label class="ssh-form-label">Password
              <input type="password" id="cf-password" value="" placeholder="leave empty for key auth" />
            </label>
            <label class="ssh-form-label">Private Key
              <input type="text" id="cf-key-path" value="${attr(existing && existing.key_path ? existing.key_path : '')}"
                     placeholder="~/.ssh/id_ed25519" spellcheck="false" />
            </label>
          </div>
          <details class="ssh-form-advanced" ${proxyType !== 'none' ? 'open' : ''}>
            <summary>Advanced</summary>
            <label class="ssh-form-label">Proxy Type
              <select id="cf-proxy-type">
                <option value="none" ${proxyType === 'none' ? 'selected' : ''}>None</option>
                <option value="jump" ${proxyType === 'jump' ? 'selected' : ''}>ProxyJump</option>
                <option value="command" ${proxyType === 'command' ? 'selected' : ''}>ProxyCommand</option>
              </select>
            </label>
            <label class="ssh-form-label">Proxy Value
              <input type="text" id="cf-proxy-value" value="${attr(proxyValue)}"
                     placeholder="user@jumphost or ssh -W %h:%p host" spellcheck="false" />
            </label>
          </details>
          <label class="ssh-form-label">Save to Folder
            <select id="cf-folder">
              ${folderOptions.map((f) =>
                `<option value="${attr(f.id)}" ${f.id === selectedFolder ? 'selected' : ''}>${esc(f.name)}</option>`
              ).join('')}
            </select>
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="cf-cancel">Cancel</button>
          <button class="ssh-form-btn" id="cf-save">Save</button>
          <button class="ssh-form-btn primary" id="cf-save-connect">Save & Connect</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);

    // Populate vault account picker
    populateAccountPicker(overlay, existingVaultId);

    // Account picker change handler
    const accountSelect = overlay.querySelector('#cf-vault-account');
    accountSelect.addEventListener('change', () => {
      const val = accountSelect.value;
      if (val === '__create__') {
        handleCreateNewAccount(overlay, existingVaultId);
        return;
      }
      updateCredentialFieldsVisibility(overlay);
    });

    // Focus the host field
    const hostInput = overlay.querySelector('#cf-host');
    setTimeout(() => hostInput.focus(), 50);

    // Close on overlay background click
    overlay.addEventListener('mousedown', (e) => {
      if (e.target === overlay) removeOverlay();
    });

    // Escape to close
    const onKey = (e) => {
      if (e.key === 'Escape') { removeOverlay(); document.removeEventListener('keydown', onKey); }
    };
    document.addEventListener('keydown', onKey);

    // Button handlers
    overlay.querySelector('#cf-cancel').addEventListener('click', removeOverlay);
    overlay.querySelector('#cf-save').addEventListener('click', () => submitForm(overlay, existing, false));
    overlay.querySelector('#cf-save-connect').addEventListener('click', () => submitForm(overlay, existing, true));
  }

  async function populateAccountPicker(overlay, selectedId) {
    const select = overlay.querySelector('#cf-vault-account');
    if (!select) return;

    let accounts = [];
    if (window.vault && window.vault.getAccounts) {
      try {
        accounts = await window.vault.getAccounts();
      } catch (_) {
        // Vault may not exist or be locked — just show manual option.
      }
    }

    // Rebuild options: keep Manual + Create New, insert accounts between them.
    let html = '<option value="">Manual credentials</option>';
    for (const a of accounts) {
      const authLabel = a.auth_type === 'password' ? 'password' : a.auth_type === 'key' ? 'key' : 'key+pw';
      html += `<option value="${attr(a.id)}">${esc(a.display_name)} (${esc(a.username)}, ${authLabel})</option>`;
    }
    html += '<option value="__create__">+ Create New Account...</option>';
    select.innerHTML = html;

    // Restore selection
    if (selectedId) {
      select.value = selectedId;
    }

    updateCredentialFieldsVisibility(overlay);
  }

  function updateCredentialFieldsVisibility(overlay) {
    const select = overlay.querySelector('#cf-vault-account');
    const manualCreds = overlay.querySelector('#cf-manual-creds');
    const accountInfo = overlay.querySelector('#cf-vault-account-info');
    if (!select || !manualCreds || !accountInfo) return;

    const val = select.value;
    if (val && val !== '__create__') {
      // A vault account is selected — hide manual fields, show info.
      manualCreds.style.display = 'none';
      const selectedOption = select.options[select.selectedIndex];
      accountInfo.style.display = 'block';
      accountInfo.textContent = 'Using vault account: ' + selectedOption.textContent;
    } else {
      // Manual credentials
      manualCreds.style.display = '';
      accountInfo.style.display = 'none';
    }
  }

  function handleCreateNewAccount(overlay, fallbackId) {
    if (!window.vault) {
      window.toast.error('Vault Unavailable', 'Vault module not loaded');
      const select = overlay.querySelector('#cf-vault-account');
      if (select) select.value = fallbackId || '';
      return;
    }

    window.vault.ensureUnlocked(() => {
      window.vault.showAccountForm(null);
      // After the vault overlay is dismissed, re-populate the picker
      // with a brief delay so the vault save completes first.
      const checkInterval = setInterval(() => {
        const vaultOverlay = document.getElementById('vault-overlay');
        if (!vaultOverlay) {
          clearInterval(checkInterval);
          populateAccountPicker(overlay, '');
        }
      }, 300);
    });
  }

  function submitForm(overlay, existing, andConnect) {
    const host = overlay.querySelector('#cf-host').value.trim();
    if (!host) { overlay.querySelector('#cf-host').focus(); return; }

    const label = overlay.querySelector('#cf-label').value.trim();
    const port = parseInt(overlay.querySelector('#cf-port').value, 10) || 22;
    const proxyType = overlay.querySelector('#cf-proxy-type').value;
    const proxyValue = overlay.querySelector('#cf-proxy-value').value.trim();
    const folderId = overlay.querySelector('#cf-folder').value || null;
    const proxyJump = proxyType === 'jump' && proxyValue ? proxyValue : null;
    const proxyCommand = proxyType === 'command' && proxyValue ? proxyValue : null;

    // Check if a vault account is selected.
    const accountSelect = overlay.querySelector('#cf-vault-account');
    const vaultAccountId = accountSelect && accountSelect.value && accountSelect.value !== '__create__'
      ? accountSelect.value
      : null;

    // When vault account is used, manual credential fields may be hidden.
    const user = vaultAccountId
      ? (existing ? existing.user : null)
      : (overlay.querySelector('#cf-user').value.trim() || 'root');
    const password = vaultAccountId ? '' : overlay.querySelector('#cf-password').value;
    const keyPath = vaultAccountId
      ? null
      : (overlay.querySelector('#cf-key-path').value.trim() || null);
    const authMethod = vaultAccountId ? null : (password ? 'password' : 'key');

    const entry = {
      id: existing ? existing.id : crypto.randomUUID(),
      label: label || `${user || 'root'}@${host}`,
      host,
      port,
      user: user || null,
      auth_method: authMethod,
      key_path: keyPath,
      vault_account_id: vaultAccountId,
      proxy_command: proxyCommand,
      proxy_jump: proxyJump,
    };

    removeOverlay();

    invoke('remote_save_server', { entry, folderId })
      .then(() => {
        refreshAll();
        if (andConnect) {
          createSshTabFn({ serverId: entry.id, password: password || undefined });
        }
      })
      .catch((e) => window.toast.error('Save Failed', String(e)));
  }

  // ---------------------------------------------------------------------------
  // Folder dialog (inline prompt-style)
  // ---------------------------------------------------------------------------

  function showAddFolderDialog() {
    removeOverlay();

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">New Folder</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">Name
            <input type="text" id="fd-name" value="" placeholder="Folder name" spellcheck="false" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="fd-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="fd-create">Create</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    const nameInput = overlay.querySelector('#fd-name');
    setTimeout(() => nameInput.focus(), 50);

    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) removeOverlay(); });
    const onKey = (e) => {
      if (e.key === 'Escape') { removeOverlay(); document.removeEventListener('keydown', onKey); }
      if (e.key === 'Enter') { doCreate(); document.removeEventListener('keydown', onKey); }
    };
    document.addEventListener('keydown', onKey);

    const doCreate = () => {
      const name = nameInput.value.trim();
      if (!name) { nameInput.focus(); return; }
      removeOverlay();
      invoke('remote_add_folder', { name }).then(() => refreshAll()).catch((e) => window.toast.error('Folder Error', String(e)));
    };

    overlay.querySelector('#fd-cancel').addEventListener('click', removeOverlay);
    overlay.querySelector('#fd-create').addEventListener('click', doCreate);
  }

  function showRenameFolderDialog(folder) {
    removeOverlay();

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">Rename Folder</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">Name
            <input type="text" id="rf-name" value="${attr(folder.name)}" spellcheck="false" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="rf-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="rf-save">Save</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    const nameInput = overlay.querySelector('#rf-name');
    setTimeout(() => { nameInput.focus(); nameInput.select(); }, 50);

    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) removeOverlay(); });
    const onKey = (e) => {
      if (e.key === 'Escape') { removeOverlay(); document.removeEventListener('keydown', onKey); }
      if (e.key === 'Enter') { doSave(); document.removeEventListener('keydown', onKey); }
    };
    document.addEventListener('keydown', onKey);

    const doSave = () => {
      const name = nameInput.value.trim();
      if (!name) { nameInput.focus(); return; }
      removeOverlay();
      invoke('remote_rename_folder', { folderId: folder.id, newName: name })
        .then(() => refreshAll()).catch((e) => window.toast.error('Error', String(e)));
    };

    overlay.querySelector('#rf-cancel').addEventListener('click', removeOverlay);
    overlay.querySelector('#rf-save').addEventListener('click', doSave);
  }

  // ---------------------------------------------------------------------------
  // Context menus
  // ---------------------------------------------------------------------------

  function showServerContextMenu(e, server, folderId) {
    showContextMenu(e, [
      { label: 'Connect', action: () => createSshTabFn({ serverId: server.id }) },
      { label: 'Edit', action: () => showConnectionForm(server, folderId) },
      { label: 'Duplicate', action: () => {
        invoke('remote_duplicate_server', { serverId: server.id }).then(() => refreshAll()).catch(() => {});
      }},
      { type: 'separator' },
      { label: 'Delete', danger: true, action: () => {
        showDeleteConfirmDialog(`Delete "${server.label}"?`, () => {
          invoke('remote_delete_server', { serverId: server.id }).then(() => refreshAll()).catch(() => {});
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
          invoke('remote_delete_folder', { folderId: folder.id }).then(() => refreshAll()).catch(() => {});
        });
      }},
    ]);
  }

  function showDeleteConfirmDialog(message, onConfirm) {
    removeOverlay();
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.style.zIndex = '5000';
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">Confirm Delete</div>
        <div class="ssh-form-body">
          <div class="ssh-auth-message">${esc(message)}</div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="dc-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="dc-delete" style="background:var(--red);border-color:var(--red)">Delete</button>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    const dismiss = () => overlay.remove();
    overlay.querySelector('#dc-cancel').addEventListener('click', dismiss);
    overlay.querySelector('#dc-delete').addEventListener('click', () => { dismiss(); onConfirm(); });
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) dismiss(); });
    const onKey = (e) => {
      if (e.key === 'Escape') { dismiss(); document.removeEventListener('keydown', onKey); }
    };
    document.addEventListener('keydown', onKey);
  }

  function showContextMenu(e, items) {
    removeContextMenu();
    const menu = document.createElement('div');
    menu.className = 'ssh-context-menu';
    menu.style.left = e.clientX + 'px';
    menu.style.top = e.clientY + 'px';

    for (const item of items) {
      if (item.type === 'separator') {
        const sep = document.createElement('div');
        sep.className = 'ssh-context-menu-sep';
        menu.appendChild(sep);
        continue;
      }
      const el = document.createElement('div');
      el.className = 'ssh-context-menu-item' + (item.danger ? ' danger' : '');
      el.textContent = item.label;
      el.addEventListener('click', () => { removeContextMenu(); item.action(); });
      menu.appendChild(el);
    }

    document.body.appendChild(menu);

    // Clamp to viewport
    requestAnimationFrame(() => {
      const rect = menu.getBoundingClientRect();
      if (rect.right > window.innerWidth) menu.style.left = (window.innerWidth - rect.width - 4) + 'px';
      if (rect.bottom > window.innerHeight) menu.style.top = (window.innerHeight - rect.height - 4) + 'px';
    });

    setTimeout(() => document.addEventListener('click', removeContextMenu, { once: true }), 0);
  }

  function removeContextMenu() {
    document.querySelectorAll('.ssh-context-menu').forEach((el) => el.remove());
  }

  function removeOverlay() {
    document.querySelectorAll('.ssh-overlay').forEach((el) => el.remove());
  }

  // ---------------------------------------------------------------------------
  // Auth prompts
  // ---------------------------------------------------------------------------

  function handleHostKeyPrompt(event) {
    const { prompt_id, message, detail } = event.payload;

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.style.zIndex = '5000';
    overlay.innerHTML = `
      <div class="ssh-form" style="max-width:520px">
        <div class="ssh-form-title">SSH Host Key Verification</div>
        <div class="ssh-form-body">
          <div class="ssh-auth-message">${esc(message)}</div>
          <pre class="ssh-auth-detail">${esc(detail)}</pre>
          <div class="ssh-auth-question">Do you want to continue connecting and save this key?</div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="hk-reject">Reject</button>
          <button class="ssh-form-btn primary" id="hk-accept">Accept & Save</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);

    const respond = (accepted) => {
      overlay.remove();
      invoke('auth_respond_host_key', { promptId: prompt_id, accepted }).catch(() => {});
    };

    overlay.querySelector('#hk-reject').addEventListener('click', () => respond(false));
    overlay.querySelector('#hk-accept').addEventListener('click', () => respond(true));
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) respond(false); });
    const onKey = (e) => {
      if (e.key === 'Escape') { respond(false); document.removeEventListener('keydown', onKey); }
      if (e.key === 'Enter') { respond(true); document.removeEventListener('keydown', onKey); }
    };
    document.addEventListener('keydown', onKey);
  }

  function handlePasswordPrompt(event) {
    const { prompt_id, message } = event.payload;

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.style.zIndex = '5000';
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">SSH Authentication</div>
        <div class="ssh-form-body">
          <div class="ssh-auth-message">${esc(message)}</div>
          <label class="ssh-form-label">Password
            <input type="password" id="pw-input" spellcheck="false" autocomplete="off" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="pw-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="pw-connect">Connect</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    setTimeout(() => overlay.querySelector('#pw-input').focus(), 50);

    const respond = (password) => {
      overlay.remove();
      invoke('auth_respond_password', { promptId: prompt_id, password }).catch(() => {});
    };

    overlay.querySelector('#pw-cancel').addEventListener('click', () => respond(null));
    overlay.querySelector('#pw-connect').addEventListener('click', () => {
      respond(overlay.querySelector('#pw-input').value || null);
    });
    overlay.querySelector('#pw-input').addEventListener('keydown', (e) => {
      if (e.key === 'Enter') respond(overlay.querySelector('#pw-input').value || null);
    });
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) respond(null); });
    const onKey = (e) => {
      if (e.key === 'Escape') { respond(null); document.removeEventListener('keydown', onKey); }
    };
    document.addEventListener('keydown', onKey);
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
