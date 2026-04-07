// SSH Tunnel Manager — modal dialog for creating, starting, stopping, and deleting tunnels.

(function (exports) {
  'use strict';

  let invoke = null;
  let listen = null;
  let serverDataFn = null; // returns { folders, ungrouped, ssh_config }

  function setOverlayDialogAttributes(overlay, label) {
    if (!overlay) return;
    overlay.setAttribute('role', 'dialog');
    overlay.setAttribute('aria-modal', 'true');
    overlay.setAttribute('aria-label', String(label || 'Dialog'));
  }

  function registerOverlayEscape(overlay, name, onEscape) {
    const keyboardRouter = window.conchKeyboardRouter;
    if (!keyboardRouter || typeof keyboardRouter.register !== 'function') {
      console.warn('tunnel-manager: keyboard router unavailable, skipping escape registration:', name || 'tunnel-overlay');
      return () => {};
    }
    return keyboardRouter.register({
      name: name || 'tunnel-overlay',
      priority: 220,
      isActive: () => !!(overlay && overlay.isConnected),
      onKeyDown: (event) => {
        if (!overlay || !overlay.isConnected) return false;
        if (event.key !== 'Escape') return false;
        onEscape(event);
        return true;
      },
    });
  }

  function init(opts) {
    invoke = opts.invoke;
    listen = opts.listen;
    serverDataFn = opts.getServerData;
  }

  // ---------------------------------------------------------------------------
  // Main tunnel manager dialog
  // ---------------------------------------------------------------------------

  async function show() {
    removeOverlay();
    const tunnels = await loadTunnels();
    renderManager(tunnels);
  }

  async function loadTunnels() {
    try {
      return await invoke('tunnel_get_all');
    } catch (e) {
      console.error('Failed to load tunnels:', e);
      return [];
    }
  }

  function renderManager(tunnels) {
    removeOverlay();

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.id = 'tunnel-manager-overlay';
    setOverlayDialogAttributes(overlay, 'SSH tunnels');

    overlay.innerHTML = `
      <div class="ssh-form tunnel-manager-dialog">
        <div class="ssh-form-title">SSH Tunnels</div>
        <div class="tunnel-manager-body">
          <div class="tunnel-table-wrap">
            <table class="tunnel-table">
              <thead>
                <tr>
                  <th class="tunnel-col-status">Status</th>
                  <th>Label</th>
                  <th>Local</th>
                  <th>Remote</th>
                  <th>Via</th>
                  <th class="tunnel-col-actions"></th>
                </tr>
              </thead>
              <tbody id="tunnel-tbody"></tbody>
            </table>
            ${tunnels.length === 0 ? '<div class="tunnel-empty">No tunnels configured</div>' : ''}
          </div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="tm-close">Close</button>
          <button class="ssh-form-btn" id="tm-new">New Tunnel\u2026</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);

    const tbody = overlay.querySelector('#tunnel-tbody');
    for (const t of tunnels) {
      tbody.appendChild(createTunnelRow(t));
    }

    // Events
    let closed = false;
    let unregisterEscape = null;
    const closeManager = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterEscape === 'function') unregisterEscape();
      unregisterEscape = null;
      removeOverlay();
    };

    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) closeManager(); });
    overlay.querySelector('#tm-close').addEventListener('click', closeManager);
    overlay.querySelector('#tm-new').addEventListener('click', () => {
      closeManager();
      showNewTunnelForm();
    });
    unregisterEscape = registerOverlayEscape(overlay, 'tunnel-manager-main', () => closeManager());
  }

  function createTunnelRow(tunnel) {
    const tr = document.createElement('tr');
    tr.className = 'tunnel-row';

    const status = tunnel.status || 'inactive';
    let statusDotClass = 'inactive';
    let statusLabel = 'Inactive';
    let statusChipClass = 'inactive';
    let errorMsg = null;
    if (status === 'active') {
      statusLabel = 'Active';
      statusDotClass = 'active';
      statusChipClass = 'active';
    } else if (status === 'connecting') {
      statusLabel = 'Connecting\u2026';
      statusDotClass = 'connecting';
      statusChipClass = 'connecting';
    } else if (status.startsWith('error')) {
      errorMsg = status.replace(/^error:\s*/, '');
      statusLabel = 'Needs Attention';
      statusDotClass = 'error';
      statusChipClass = 'error';
    }

    const remote = `${tunnel.remote_host}:${tunnel.remote_port}`;
    const isActive = status === 'active';
    const isConnecting = status === 'connecting';
    const isRunning = isActive || isConnecting;
    const startStopIcon = isRunning
      ? '<svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style="vertical-align:-2px"><path d="M 2 2 v 12 h 12 v -12 z"/></svg>'
      : '<svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style="vertical-align:-2px"><path d="M 3 2 v 12 l 11 -6 z"/></svg>';

    tr.innerHTML =
      `<td class="tunnel-col-status"><span class="tunnel-dot ${statusDotClass}"></span><span class="tunnel-status-chip ${statusChipClass}">${esc(statusLabel)}</span></td>` +
      `<td><div class="tunnel-label">${esc(tunnel.label)}</div>${errorMsg ? `<div class="tunnel-error-inline" title="${attr(errorMsg)}">${esc(errorMsg)}</div>` : ''}</td>` +
      `<td class="tunnel-mono">${tunnel.local_port}</td>` +
      `<td class="tunnel-mono">${esc(remote)}</td>` +
      `<td class="tunnel-mono">${esc(tunnel.session_key)}</td>`;

    // Row click opens edit flow.
    tr.addEventListener('click', () => showEditTunnelForm(tunnel));

    // Compact actions cell: one primary toggle + overflow menu.
    const actionsTd = document.createElement('td');
    actionsTd.className = 'tunnel-actions';
    const actionIcon = document.createElement('span');
    actionIcon.className = 'tunnel-action-icon';
    actionIcon.setAttribute('role', 'button');
    actionIcon.setAttribute('tabindex', '0');
    actionIcon.title = isRunning ? 'Stop Tunnel' : (errorMsg ? 'Retry Connection' : 'Start Tunnel');
    actionIcon.innerHTML = startStopIcon;
    const handlePrimaryAction = async (e) => {
      e.stopPropagation();
      actionIcon.classList.add('disabled');
      actionIcon.style.pointerEvents = 'none';
      if (isRunning) {
        await doStop(tunnel.id);
      } else {
        await doStart(tunnel);
      }
    };
    actionIcon.addEventListener('click', handlePrimaryAction);
    actionIcon.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        handlePrimaryAction(e);
      }
    });
    actionsTd.appendChild(actionIcon);

    const moreBtn = document.createElement('button');
    moreBtn.className = 'tunnel-action-btn tunnel-action-more';
    moreBtn.textContent = '\u22ef';
    moreBtn.title = 'More Actions';
    moreBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      showRowMenu(e, tunnel, status, errorMsg);
    });
    actionsTd.appendChild(moreBtn);

    tr.appendChild(actionsTd);
    tr.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      showRowMenu(e, tunnel, status, errorMsg);
    });
    return tr;
  }

  async function doStart(tunnel) {
    const tunnelId = typeof tunnel === 'object' && tunnel !== null ? tunnel.id : tunnel;
    try {
      await invoke('tunnel_start', { tunnelId });
    } catch (e) {
      showErrorDialog(
        'Tunnel Error',
        String(e),
        () => doStart(tunnel),
        typeof tunnel === 'object' && tunnel !== null ? () => showEditTunnelForm(tunnel) : null
      );
      return;
    }
    setTimeout(() => show(), 500);
  }

  async function doStop(tunnelId) {
    try {
      await invoke('tunnel_stop', { tunnelId });
    } catch (e) {
      window.toast.error('Tunnel Error', 'Failed to stop: ' + e);
    }
    show();
  }

  async function doDelete(tunnel) {
    try {
      await invoke('tunnel_delete', { tunnelId: tunnel.id });
    } catch (e) {
      window.toast.error('Tunnel Error', 'Failed to delete: ' + e);
    }
    show();
  }

  function showDeleteDialog(tunnel) {
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.style.zIndex = '3100';
    setOverlayDialogAttributes(overlay, 'Delete tunnel');
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">Delete Tunnel</div>
        <div class="ssh-form-body">
          <div class="ssh-auth-message">Delete <strong>${esc(tunnel.label)}</strong>?</div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="del-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="del-confirm" style="background:var(--red);border-color:var(--red)">Delete</button>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    let dismissed = false;
    let unregisterEscape = null;
    const dismiss = () => {
      if (dismissed) return;
      dismissed = true;
      if (typeof unregisterEscape === 'function') unregisterEscape();
      unregisterEscape = null;
      overlay.remove();
    };
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) dismiss(); });
    overlay.querySelector('#del-cancel').addEventListener('click', dismiss);
    overlay.querySelector('#del-confirm').addEventListener('click', async () => {
      dismiss();
      await doDelete(tunnel);
    });
    unregisterEscape = registerOverlayEscape(overlay, 'tunnel-delete-dialog', () => dismiss());
  }

  function showRowMenu(e, tunnel, status, errorMsg) {
    removeContextMenu();

    const items = [];
    if (status === 'active' || status === 'connecting') {
      items.push({ label: 'Stop', action: () => doStop(tunnel.id) });
    } else {
      items.push({ label: errorMsg ? 'Retry' : 'Start', action: () => doStart(tunnel) });
    }
    items.push({ label: 'Edit', action: () => showEditTunnelForm(tunnel) });
    if (errorMsg) {
      items.push({ label: 'View Error', action: () => showErrorDialog('Tunnel Error', errorMsg, () => doStart(tunnel), () => showEditTunnelForm(tunnel)) });
    }
    items.push({ type: 'separator' });
    items.push({ label: 'Delete', danger: true, action: () => showDeleteDialog(tunnel) });

    const menu = document.createElement('div');
    menu.className = 'ssh-context-menu';
    menu.setAttribute('role', 'menu');
    menu.setAttribute('aria-label', 'Tunnel actions');
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
      el.setAttribute('role', 'menuitem');
      el.tabIndex = 0;
      const activate = () => {
        removeContextMenu();
        item.action();
      };
      el.addEventListener('click', activate);
      el.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        activate();
      });
      menu.appendChild(el);
    }

    document.body.appendChild(menu);
    requestAnimationFrame(() => {
      const rect = menu.getBoundingClientRect();
      if (rect.right > window.innerWidth) menu.style.left = (window.innerWidth - rect.width - 4) + 'px';
      if (rect.bottom > window.innerHeight) menu.style.top = (window.innerHeight - rect.height - 4) + 'px';
      const firstItem = menu.querySelector('.ssh-context-menu-item[role="menuitem"]');
      if (firstItem && typeof firstItem.focus === 'function') firstItem.focus();
    });
    setTimeout(() => document.addEventListener('click', removeContextMenu, { once: true }), 0);
  }

  function removeContextMenu() {
    document.querySelectorAll('.ssh-context-menu').forEach((el) => el.remove());
  }

  // ---------------------------------------------------------------------------
  // New tunnel form
  // ---------------------------------------------------------------------------

  function showNewTunnelForm() {
    removeOverlay();

    const data = serverDataFn ? serverDataFn() : { folders: [], ungrouped: [], ssh_config: [] };
    const allServers = [
      ...data.ungrouped,
      ...(data.folders || []).flatMap((f) => f.entries),
      ...(data.ssh_config || []),
    ];

    const serverOptions = allServers.map((s) => {
      const key = `${s.user}@${s.host}:${s.port}`;
      return { key, label: `${s.label} \u2014 ${key}` };
    });

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    setOverlayDialogAttributes(overlay, 'New SSH tunnel');
    overlay.innerHTML = `
      <div class="ssh-form">
        <div class="ssh-form-title">New SSH Tunnel</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">SSH Server
            <select id="nt-server">
              ${serverOptions.map((s) =>
                `<option value="${attr(s.key)}">${esc(s.label)}</option>`
              ).join('')}
            </select>
          </label>
          <div class="ssh-form-row">
            <label class="ssh-form-label" style="flex:1">Local Port
              <input type="number" id="nt-local-port" min="1" max="65535" placeholder="8080" />
            </label>
            <label class="ssh-form-label" style="flex:1">Remote Host
              <input type="text" id="nt-remote-host" value="localhost" spellcheck="false" />
            </label>
            <label class="ssh-form-label" style="width:90px">Remote Port
              <input type="number" id="nt-remote-port" min="1" max="65535" placeholder="80" />
            </label>
          </div>
          <label class="ssh-form-label">Label (optional)
            <input type="text" id="nt-label" placeholder="e.g. Web Server" spellcheck="false" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="nt-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="nt-save">Save & Connect</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    setTimeout(() => overlay.querySelector('#nt-local-port').focus(), 50);

    let dismissed = false;
    let unregisterEscape = null;
    const dismiss = () => {
      if (dismissed) return;
      dismissed = true;
      if (typeof unregisterEscape === 'function') unregisterEscape();
      unregisterEscape = null;
      removeOverlay();
    };

    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) dismiss(); });
    unregisterEscape = registerOverlayEscape(overlay, 'tunnel-new-form', () => dismiss());

    overlay.querySelector('#nt-cancel').addEventListener('click', () => { dismiss(); show(); });
    overlay.querySelector('#nt-save').addEventListener('click', () => submitNewTunnel(overlay, dismiss));
  }

  async function submitNewTunnel(overlay, dismissOverlay) {
    const sessionKey = overlay.querySelector('#nt-server').value;
    const localPort = parseInt(overlay.querySelector('#nt-local-port').value, 10);
    const remoteHost = overlay.querySelector('#nt-remote-host').value.trim() || 'localhost';
    const remotePort = parseInt(overlay.querySelector('#nt-remote-port').value, 10);
    const label = overlay.querySelector('#nt-label').value.trim();

    if (!localPort || localPort < 1 || localPort > 65535) {
      window.toast.warn('Invalid Port', 'Local port must be between 1 and 65535.');
      overlay.querySelector('#nt-local-port').focus();
      return;
    }
    if (!remotePort || remotePort < 1 || remotePort > 65535) {
      window.toast.warn('Invalid Port', 'Remote port must be between 1 and 65535.');
      overlay.querySelector('#nt-remote-port').focus();
      return;
    }

    const tunnelLabel = label || `:${localPort} -> ${remoteHost}:${remotePort}`;

    const tunnel = {
      id: crypto.randomUUID(),
      label: tunnelLabel,
      session_key: sessionKey,
      local_port: localPort,
      remote_host: remoteHost,
      remote_port: remotePort,
      auto_start: false,
    };

    if (typeof dismissOverlay === 'function') dismissOverlay();
    else removeOverlay();

    try {
      await invoke('tunnel_save', { tunnel });
      // Re-show the manager immediately so the new tunnel is visible
      await show();
      // Start connecting in the background, then refresh to update status
      invoke('tunnel_start', { tunnelId: tunnel.id })
        .then(() => show())
        .catch((e) => {
          window.toast.error('Tunnel Error', String(e));
          show();
        });
    } catch (e) {
      window.toast.error('Save Failed', String(e));
      show();
    }
  }

  // ---------------------------------------------------------------------------
  // Edit tunnel form
  // ---------------------------------------------------------------------------

  function showEditTunnelForm(tunnel) {
    removeOverlay();

    const data = serverDataFn ? serverDataFn() : { folders: [], ungrouped: [], ssh_config: [] };
    const allServers = [
      ...data.ungrouped,
      ...(data.folders || []).flatMap((f) => f.entries),
      ...(data.ssh_config || []),
    ];

    const serverOptions = allServers.map((s) => {
      const key = `${s.user}@${s.host}:${s.port}`;
      return { key, label: `${s.label} \u2014 ${key}` };
    });

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    setOverlayDialogAttributes(overlay, 'Edit SSH tunnel');
    overlay.innerHTML = `
      <div class="ssh-form">
        <div class="ssh-form-title">Edit SSH Tunnel</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">SSH Server
            <select id="et-server">
              ${serverOptions.map((s) =>
                `<option value="${attr(s.key)}" ${s.key === tunnel.session_key ? 'selected' : ''}>${esc(s.label)}</option>`
              ).join('')}
            </select>
          </label>
          <div class="ssh-form-row">
            <label class="ssh-form-label" style="flex:1">Local Port
              <input type="number" id="et-local-port" value="${tunnel.local_port}" min="1" max="65535" />
            </label>
            <label class="ssh-form-label" style="flex:1">Remote Host
              <input type="text" id="et-remote-host" value="${attr(tunnel.remote_host)}" spellcheck="false" />
            </label>
            <label class="ssh-form-label" style="width:90px">Remote Port
              <input type="number" id="et-remote-port" value="${tunnel.remote_port}" min="1" max="65535" />
            </label>
          </div>
          <label class="ssh-form-label">Label
            <input type="text" id="et-label" value="${attr(tunnel.label)}" spellcheck="false" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="et-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="et-save">Save</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    setTimeout(() => overlay.querySelector('#et-local-port').focus(), 50);

    let dismissed = false;
    let unregisterEscape = null;
    const dismiss = () => {
      if (dismissed) return;
      dismissed = true;
      if (typeof unregisterEscape === 'function') unregisterEscape();
      unregisterEscape = null;
      removeOverlay();
    };

    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) { dismiss(); show(); } });
    unregisterEscape = registerOverlayEscape(overlay, 'tunnel-edit-form', () => {
      dismiss();
      show();
    });

    overlay.querySelector('#et-cancel').addEventListener('click', () => { dismiss(); show(); });
    overlay.querySelector('#et-save').addEventListener('click', () => submitEditTunnel(overlay, tunnel, dismiss));
  }

  async function submitEditTunnel(overlay, original, dismissOverlay) {
    const sessionKey = overlay.querySelector('#et-server').value;
    const localPort = parseInt(overlay.querySelector('#et-local-port').value, 10);
    const remoteHost = overlay.querySelector('#et-remote-host').value.trim() || 'localhost';
    const remotePort = parseInt(overlay.querySelector('#et-remote-port').value, 10);
    const label = overlay.querySelector('#et-label').value.trim();

    if (!localPort || localPort < 1 || localPort > 65535) {
      window.toast.warn('Invalid Port', 'Local port must be between 1 and 65535.');
      return;
    }
    if (!remotePort || remotePort < 1 || remotePort > 65535) {
      window.toast.warn('Invalid Port', 'Remote port must be between 1 and 65535.');
      return;
    }

    const tunnel = {
      id: original.id,
      label: label || `:${localPort} -> ${remoteHost}:${remotePort}`,
      session_key: sessionKey,
      local_port: localPort,
      remote_host: remoteHost,
      remote_port: remotePort,
      auto_start: original.auto_start || false,
    };

    if (typeof dismissOverlay === 'function') dismissOverlay();
    else removeOverlay();

    try {
      // Stop the tunnel if it was running (config changed).
      await invoke('tunnel_stop', { tunnelId: original.id }).catch(() => {});
      await invoke('tunnel_save', { tunnel });
    } catch (e) {
      window.toast.error('Save Failed', String(e));
    }
    show();
  }

  // ---------------------------------------------------------------------------
  // Error dialog
  // ---------------------------------------------------------------------------

  function showErrorDialog(title, message, onRetry, onEdit) {
    // Don't remove the manager overlay — layer this on top
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.style.zIndex = '3100';
    setOverlayDialogAttributes(overlay, title || 'Tunnel error');
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">${esc(title)}</div>
        <div class="ssh-form-body">
          <div class="ssh-error-text">${esc(message)}</div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="err-dismiss">Dismiss</button>
          ${onEdit ? '<button class="ssh-form-btn" id="err-edit">Edit</button>' : ''}
          ${onRetry ? '<button class="ssh-form-btn primary" id="err-retry">Retry</button>' : ''}
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    let dismissed = false;
    let unregisterEscape = null;
    const dismiss = () => {
      if (dismissed) return;
      dismissed = true;
      if (typeof unregisterEscape === 'function') unregisterEscape();
      unregisterEscape = null;
      overlay.remove();
    };
    overlay.querySelector('#err-dismiss').addEventListener('click', dismiss);
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) dismiss(); });

    if (onRetry) {
      overlay.querySelector('#err-retry').addEventListener('click', () => {
        dismiss();
        onRetry();
      });
    }
    if (onEdit) {
      overlay.querySelector('#err-edit').addEventListener('click', () => {
        dismiss();
        onEdit();
      });
    }

    unregisterEscape = registerOverlayEscape(overlay, 'tunnel-error-dialog', () => dismiss());
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  function removeOverlay() {
    const el = document.getElementById('tunnel-manager-overlay');
    if (el) el.remove();
    // Also remove any other ssh-overlay that the new-tunnel form might have created
    document.querySelectorAll('.ssh-overlay').forEach((el) => el.remove());
  }

  const esc = window.utils.esc;
  const attr = window.utils.attr;

  exports.tunnelManager = { init, show, showEdit: showEditTunnelForm, showError: showErrorDialog };
})(window);
