(function initConchSshView(global) {
  'use strict';

  function makeSectionHeader(text) {
    const el = document.createElement('div');
    el.className = 'ssh-section-header';
    el.textContent = text;
    return el;
  }

  function createServerNode(server, dimmed, folderId, highlighted, deps) {
    const d = deps || {};
    const esc = typeof d.esc === 'function' ? d.esc : (value) => String(value == null ? '' : value);
    const el = document.createElement('div');
    el.className = 'ssh-server-node' + (dimmed ? ' dimmed' : '') + (highlighted ? ' highlighted' : '');
    el.title = `${server.user}@${server.host}:${server.port}`;

    const label = server.label || `${server.user}@${server.host}`;
    const detail = server.host + (server.port !== 22 ? ':' + server.port : '');
    el.innerHTML =
      `<span class="ssh-server-label">${esc(label)}</span>` +
      `<span class="ssh-server-detail">${esc(detail)}</span>`;

    el.addEventListener('dblclick', () => {
      if (typeof d.onServerDblClick === 'function') d.onServerDblClick(server);
    });

    el.addEventListener('contextmenu', (event) => {
      event.preventDefault();
      if (typeof d.onServerContextMenu === 'function') d.onServerContextMenu(event, server, folderId);
    });

    return el;
  }

  function createFolderNode(folder, deps) {
    const d = deps || {};
    const esc = typeof d.esc === 'function' ? d.esc : (value) => String(value == null ? '' : value);
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
      if (typeof d.onFolderToggle === 'function') d.onFolderToggle(folder, !expanded);
    });

    header.addEventListener('contextmenu', (event) => {
      event.preventDefault();
      if (typeof d.onFolderContextMenu === 'function') d.onFolderContextMenu(event, folder);
    });

    el.appendChild(header);

    if (expanded) {
      const list = document.createElement('div');
      list.className = 'ssh-folder-entries';
      for (const server of folder.entries) {
        list.appendChild(createServerNode(server, false, folder.id, false, d));
      }
      el.appendChild(list);
    }

    return el;
  }

  function renderServerList(ctx) {
    const c = ctx || {};
    const serverListEl = c.serverListEl;
    if (!serverListEl) return;
    const frag = document.createDocumentFragment();

    if (c.searchQuery) {
      const matches = typeof c.getFilteredServers === 'function' ? c.getFilteredServers(c.searchQuery) : [];
      for (let i = 0; i < matches.length; i += 1) {
        frag.appendChild(createServerNode(matches[i], false, null, i === c.searchSelectedIndex, c));
      }
      if (matches.length === 0) {
        const hint = document.createElement('div');
        hint.className = 'ssh-search-hint';
        hint.textContent = 'No matches — Enter to quick-connect';
        frag.appendChild(hint);
      }
    } else {
      const data = c.serverData || { folders: [], ungrouped: [], ssh_config: [] };
      const hasServers = data.folders.length > 0 || data.ungrouped.length > 0 || data.ssh_config.length > 0;
      if (hasServers) {
        const sep = document.createElement('div');
        sep.className = 'ssh-panel-separator';
        frag.appendChild(sep);

        const headerRow = document.createElement('div');
        headerRow.className = 'ssh-tunnels-header';
        headerRow.innerHTML = '<span class="ssh-section-header-inline">SSH Sessions</span>';
        frag.appendChild(headerRow);
      }

      for (const folder of data.folders) {
        frag.appendChild(createFolderNode(folder, c));
      }
      for (const server of data.ungrouped) {
        frag.appendChild(createServerNode(server, false, null, false, c));
      }
      if (data.ssh_config.length > 0) {
        frag.appendChild(makeSectionHeader('~/.ssh/config'));
        for (const server of data.ssh_config) {
          frag.appendChild(createServerNode(server, true, null, false, c));
        }
      }
    }

    serverListEl.innerHTML = '';
    serverListEl.appendChild(frag);
  }

  function renderSessions(sessionListEl, sessions, deps) {
    if (!sessionListEl) return;
    const esc = deps && typeof deps.esc === 'function' ? deps.esc : (value) => String(value == null ? '' : value);
    sessionListEl.innerHTML = '';
    if (!sessions || sessions.length === 0) return;

    const frag = document.createDocumentFragment();
    frag.appendChild(makeSectionHeader('Active'));

    for (const session of sessions) {
      const el = document.createElement('div');
      el.className = 'ssh-session-node';
      el.innerHTML =
        '<span class="ssh-session-dot"></span>' +
        `<span class="ssh-session-label">${esc(session.user)}@${esc(session.host)}</span>`;
      frag.appendChild(el);
    }

    sessionListEl.appendChild(frag);
  }

  function createTunnelNode(tunnel, deps) {
    const d = deps || {};
    const esc = typeof d.esc === 'function' ? d.esc : (value) => String(value == null ? '' : value);
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
      (errorMsg ? `<span class="ssh-tunnel-error-indicator" title="Error: ${esc(errorMsg)}">!</span>` : '') +
      `<button class="ssh-tunnel-btn ssh-tunnel-action-btn" title="${btnTitle}">${errorMsg ? 'Retry' : btnIcon}</button>` +
      '<button class="ssh-tunnel-btn ssh-tunnel-menu-btn" title="More actions">⋯</button>';

    if (errorMsg) el.title = 'Error: ' + errorMsg;

    const actionBtn = el.querySelector('.ssh-tunnel-action-btn');
    actionBtn.addEventListener('click', async (event) => {
      event.stopPropagation();
      actionBtn.disabled = true;
      try {
        if (isConnected) {
          if (typeof d.onStopTunnel === 'function') await d.onStopTunnel(tunnel);
          if (d.toast && typeof d.toast.info === 'function') d.toast.info('Tunnel Disconnected', tunnel.label);
        } else {
          if (typeof d.onStartTunnel === 'function') await d.onStartTunnel(tunnel);
          if (d.toast && typeof d.toast.success === 'function') d.toast.success('Tunnel Connected', tunnel.label);
        }
      } catch (error) {
        if (d.toast && typeof d.toast.error === 'function') d.toast.error('Tunnel Error', String(error));
      }
      if (typeof d.onRefreshTunnels === 'function') {
        setTimeout(d.onRefreshTunnels, isConnected ? 400 : 500);
      }
    });

    const menuBtn = el.querySelector('.ssh-tunnel-menu-btn');
    menuBtn.addEventListener('click', (event) => {
      event.stopPropagation();
      const rect = menuBtn.getBoundingClientRect();
      if (typeof d.onTunnelContextMenu === 'function') {
        d.onTunnelContextMenu(
          { preventDefault() {}, clientX: rect.right - 4, clientY: rect.bottom + 2 },
          tunnel,
          status
        );
      }
    });

    el.addEventListener('contextmenu', (event) => {
      event.preventDefault();
      if (typeof d.onTunnelContextMenu === 'function') d.onTunnelContextMenu(event, tunnel, status);
    });

    return el;
  }

  function renderTunnels(tunnelsSectionEl, tunnels, deps) {
    if (!tunnelsSectionEl) return;
    tunnelsSectionEl.innerHTML = '';
    if (tunnels.length === 0 && !tunnelsSectionEl.dataset.showEmpty) return;

    const frag = document.createDocumentFragment();

    const sep = document.createElement('div');
    sep.className = 'ssh-panel-separator';
    frag.appendChild(sep);

    const headerRow = document.createElement('div');
    headerRow.className = 'ssh-tunnels-header';
    headerRow.innerHTML = '<span class="ssh-section-header-inline">Tunnels</span>';
    frag.appendChild(headerRow);

    for (const tunnel of tunnels) {
      frag.appendChild(createTunnelNode(tunnel, deps));
    }

    if (tunnels.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'ssh-tunnel-empty';
      empty.textContent = 'No tunnels configured';
      frag.appendChild(empty);
    }

    tunnelsSectionEl.appendChild(frag);
  }

  global.conchSshView = {
    renderServerList,
    renderSessions,
    renderTunnels,
  };
})(window);
