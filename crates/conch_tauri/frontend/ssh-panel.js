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

    panelEl.innerHTML = `
      <div class="ssh-panel-header">
        <span class="ssh-panel-title">Sessions</span>
        <div class="ssh-panel-actions">
          <button class="ssh-icon-btn" id="ssh-add-server" title="New Connection">+</button>
          <button class="ssh-icon-btn" id="ssh-add-folder" title="New Folder">&#128193;</button>
          <button class="ssh-icon-btn" id="ssh-refresh" title="Refresh">&#8635;</button>
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
      }
    });

    // Buttons
    panelEl.querySelector('#ssh-add-server').addEventListener('click', () => showConnectionForm());
    panelEl.querySelector('#ssh-add-folder').addEventListener('click', showAddFolderDialog);
    panelEl.querySelector('#ssh-refresh').addEventListener('click', refreshAll);

    // Auth prompts
    listen('ssh-host-key-prompt', handleHostKeyPrompt);
    listen('ssh-password-prompt', handlePasswordPrompt);

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
    if (fitActiveTabFn) setTimeout(fitActiveTabFn, 50);
    saveLayoutState();
  }

  function hidePanel() {
    panelWrapEl.classList.add('hidden');
    if (fitActiveTabFn) setTimeout(fitActiveTabFn, 50);
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

  function handleGlobalKeydown(e) {
    // Cmd+/ — focus quick connect
    if ((e.metaKey || e.ctrlKey) && e.key === '/') {
      e.preventDefault();
      focusQuickConnect();
      return;
    }
    // Cmd+Shift+S — toggle panel
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === 's') {
      e.preventDefault();
      togglePanel();
      return;
    }
  }

  // ---------------------------------------------------------------------------
  // Resize drag
  // ---------------------------------------------------------------------------

  function initResize() {
    if (!resizeHandleEl) return;

    let dragging = false;
    let startX = 0;
    let startWidth = 0;

    resizeHandleEl.addEventListener('mousedown', (e) => {
      e.preventDefault();
      dragging = true;
      startX = e.clientX;
      startWidth = panelEl.offsetWidth;
      resizeHandleEl.classList.add('dragging');
      document.body.style.cursor = 'col-resize';
      document.body.style.userSelect = 'none';
    });

    window.addEventListener('mousemove', (e) => {
      if (!dragging) return;
      // Panel is on the right, so dragging left = bigger panel
      const delta = startX - e.clientX;
      const newWidth = Math.max(180, Math.min(500, startWidth + delta));
      panelEl.style.width = newWidth + 'px';
      if (fitActiveTabFn) fitActiveTabFn();
    });

    window.addEventListener('mouseup', () => {
      if (!dragging) return;
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
      // Normal tree view
      for (const folder of serverData.folders) {
        frag.appendChild(createFolderNode(folder));
      }
      for (const server of serverData.ungrouped) {
        frag.appendChild(createServerNode(server));
      }
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
      `<span class="ssh-section-header-inline">Tunnels</span>` +
      `<button class="ssh-icon-btn ssh-icon-btn-sm" id="ssh-add-tunnel" title="New Tunnel">+</button>`;
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

    // Wire add button
    tunnelsSectionEl.querySelector('#ssh-add-tunnel').addEventListener('click', () => {
      if (window.tunnelManager) window.tunnelManager.show();
    });
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

    if (errorMsg) {
      el.title = 'Error: ' + errorMsg + ' (click to retry)';
    }

    el.innerHTML =
      `<span class="tunnel-dot ${dotClass}"></span>` +
      `<span class="ssh-tunnel-label">${esc(tunnel.label)}</span>`;

    // Click to toggle start/stop/retry
    el.addEventListener('click', async () => {
      if (status === 'active' || status === 'connecting') {
        try { await invoke('tunnel_stop', { tunnelId: tunnel.id }); } catch (e) { console.error(e); }
        setTimeout(refreshTunnels, 300);
      } else if (errorMsg && window.tunnelManager) {
        // Show the error in a dialog with retry option
        window.tunnelManager.showError('Tunnel Error', errorMsg, async () => {
          try { await invoke('tunnel_start', { tunnelId: tunnel.id }); } catch (e) {
            window.tunnelManager.showError('Tunnel Error', String(e));
          }
          setTimeout(refreshTunnels, 500);
        });
      } else {
        try {
          await invoke('tunnel_start', { tunnelId: tunnel.id });
        } catch (e) {
          if (window.tunnelManager) {
            window.tunnelManager.showError('Tunnel Error', String(e), async () => {
              try { await invoke('tunnel_start', { tunnelId: tunnel.id }); } catch (e2) {
                window.tunnelManager.showError('Tunnel Error', String(e2));
              }
              setTimeout(refreshTunnels, 500);
            });
          }
        }
        setTimeout(refreshTunnels, 500);
      }
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
        try { await invoke('tunnel_start', { tunnelId: tunnel.id }); } catch (err) { alert('Tunnel error: ' + err); }
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

  function submitForm(overlay, existing, andConnect) {
    const host = overlay.querySelector('#cf-host').value.trim();
    if (!host) { overlay.querySelector('#cf-host').focus(); return; }

    const label = overlay.querySelector('#cf-label').value.trim();
    const port = parseInt(overlay.querySelector('#cf-port').value, 10) || 22;
    const user = overlay.querySelector('#cf-user').value.trim() || 'root';
    const password = overlay.querySelector('#cf-password').value;
    const keyPath = overlay.querySelector('#cf-key-path').value.trim() || null;
    const proxyType = overlay.querySelector('#cf-proxy-type').value;
    const proxyValue = overlay.querySelector('#cf-proxy-value').value.trim();
    const folderId = overlay.querySelector('#cf-folder').value || null;

    const authMethod = password ? 'password' : 'key';
    const proxyJump = proxyType === 'jump' && proxyValue ? proxyValue : null;
    const proxyCommand = proxyType === 'command' && proxyValue ? proxyValue : null;

    const entry = {
      id: existing ? existing.id : crypto.randomUUID(),
      label: label || `${user}@${host}`,
      host,
      port,
      user,
      auth_method: authMethod,
      key_path: keyPath,
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
      .catch((e) => alert('Failed to save: ' + e));
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
      invoke('remote_add_folder', { name }).then(() => refreshAll()).catch((e) => alert('Failed: ' + e));
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
        .then(() => refreshAll()).catch((e) => alert('Failed: ' + e));
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
        if (confirm(`Delete "${server.label}"?`)) {
          invoke('remote_delete_server', { serverId: server.id }).then(() => refreshAll()).catch(() => {});
        }
      }},
    ]);
  }

  function showFolderContextMenu(e, folder) {
    showContextMenu(e, [
      { label: 'Add Server Here', action: () => showConnectionForm(null, folder.id) },
      { label: 'Rename', action: () => showRenameFolderDialog(folder) },
      { type: 'separator' },
      { label: 'Delete Folder', danger: true, action: () => {
        if (confirm(`Delete folder "${folder.name}" and all servers in it?`)) {
          invoke('remote_delete_folder', { folderId: folder.id }).then(() => refreshAll()).catch(() => {});
        }
      }},
    ]);
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
    const accepted = confirm(message + '\n\n' + detail + '\n\nAccept and save?');
    invoke('auth_respond_host_key', { promptId: prompt_id, accepted }).catch(() => {});
  }

  function handlePasswordPrompt(event) {
    const { prompt_id, message } = event.payload;
    const password = prompt(message);
    invoke('auth_respond_password', { promptId: prompt_id, password: password || null }).catch(() => {});
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  function esc(str) {
    const el = document.createElement('span');
    el.textContent = str;
    return el.innerHTML;
  }

  function attr(str) {
    return String(str || '').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  function getServerData() { return serverData; }

  exports.sshPanel = { init, refreshAll, refreshSessions, togglePanel, focusQuickConnect, isHidden, getServerData };
})(window);
