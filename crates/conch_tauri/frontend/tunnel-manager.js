// SSH Tunnel Manager — modal dialog for creating, starting, stopping, and deleting tunnels.

(function (exports) {
  'use strict';

  let invoke = null;
  let listen = null;
  let serverDataFn = null; // returns { folders, ungrouped, ssh_config }

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
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) removeOverlay(); });
    overlay.querySelector('#tm-close').addEventListener('click', removeOverlay);
    overlay.querySelector('#tm-new').addEventListener('click', () => showNewTunnelForm());

    const onKey = (e) => {
      if (e.key === 'Escape') { removeOverlay(); document.removeEventListener('keydown', onKey); }
    };
    document.addEventListener('keydown', onKey);
  }

  function createTunnelRow(tunnel) {
    const tr = document.createElement('tr');
    tr.className = 'tunnel-row';

    const status = tunnel.status || 'inactive';
    let statusDot = '';
    let statusLabel = '';
    if (status === 'active') {
      statusDot = '<span class="tunnel-dot active"></span>';
      statusLabel = 'Active';
    } else if (status === 'connecting') {
      statusDot = '<span class="tunnel-dot connecting"></span>';
      statusLabel = 'Connecting\u2026';
    } else if (status.startsWith('error')) {
      statusDot = '<span class="tunnel-dot error"></span>';
      statusLabel = 'Error';
    } else {
      statusDot = '<span class="tunnel-dot inactive"></span>';
      statusLabel = 'Inactive';
    }

    const remote = `${tunnel.remote_host}:${tunnel.remote_port}`;

    tr.innerHTML =
      `<td class="tunnel-col-status">${statusDot} ${esc(statusLabel)}</td>` +
      `<td>${esc(tunnel.label)}</td>` +
      `<td class="tunnel-mono">${tunnel.local_port}</td>` +
      `<td class="tunnel-mono">${esc(remote)}</td>` +
      `<td class="tunnel-mono">${esc(tunnel.session_key)}</td>`;

    // Action buttons cell
    const actionsTd = document.createElement('td');
    actionsTd.className = 'tunnel-actions';

    if (status === 'active' || status === 'connecting') {
      const stopBtn = document.createElement('button');
      stopBtn.className = 'tunnel-action-btn';
      stopBtn.textContent = 'Stop';
      stopBtn.addEventListener('click', () => doStop(tunnel.id));
      actionsTd.appendChild(stopBtn);
    } else {
      const startBtn = document.createElement('button');
      startBtn.className = 'tunnel-action-btn';
      startBtn.textContent = 'Start';
      startBtn.addEventListener('click', () => doStart(tunnel.id));
      actionsTd.appendChild(startBtn);
    }

    const deleteBtn = document.createElement('button');
    deleteBtn.className = 'tunnel-action-btn danger';
    deleteBtn.textContent = 'Delete';
    deleteBtn.addEventListener('click', () => doDelete(tunnel));
    actionsTd.appendChild(deleteBtn);

    tr.appendChild(actionsTd);
    return tr;
  }

  async function doStart(tunnelId) {
    try {
      await invoke('tunnel_start', { tunnelId });
    } catch (e) {
      alert('Tunnel error: ' + e);
    }
    // Refresh after a short delay to pick up status change
    setTimeout(() => show(), 500);
  }

  async function doStop(tunnelId) {
    try {
      await invoke('tunnel_stop', { tunnelId });
    } catch (e) {
      alert('Failed to stop tunnel: ' + e);
    }
    show();
  }

  async function doDelete(tunnel) {
    if (!confirm(`Delete tunnel "${tunnel.label}"?`)) return;
    try {
      await invoke('tunnel_delete', { tunnelId: tunnel.id });
    } catch (e) {
      alert('Failed to delete tunnel: ' + e);
    }
    show();
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

    const defaultServer = serverOptions.length > 0 ? serverOptions[0].key : '';

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
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

    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) removeOverlay(); });
    const onKey = (e) => {
      if (e.key === 'Escape') { removeOverlay(); document.removeEventListener('keydown', onKey); }
    };
    document.addEventListener('keydown', onKey);

    overlay.querySelector('#nt-cancel').addEventListener('click', () => { removeOverlay(); show(); });
    overlay.querySelector('#nt-save').addEventListener('click', () => submitNewTunnel(overlay));
  }

  async function submitNewTunnel(overlay) {
    const sessionKey = overlay.querySelector('#nt-server').value;
    const localPort = parseInt(overlay.querySelector('#nt-local-port').value, 10);
    const remoteHost = overlay.querySelector('#nt-remote-host').value.trim() || 'localhost';
    const remotePort = parseInt(overlay.querySelector('#nt-remote-port').value, 10);
    const label = overlay.querySelector('#nt-label').value.trim();

    if (!localPort || localPort < 1 || localPort > 65535) {
      alert('Local port must be between 1 and 65535.');
      overlay.querySelector('#nt-local-port').focus();
      return;
    }
    if (!remotePort || remotePort < 1 || remotePort > 65535) {
      alert('Remote port must be between 1 and 65535.');
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

    removeOverlay();

    try {
      await invoke('tunnel_save', { tunnel });
      // Start connecting immediately
      invoke('tunnel_start', { tunnelId: tunnel.id }).catch((e) => {
        alert('Tunnel start error: ' + e);
      });
      // Show the manager with updated state after a brief delay
      setTimeout(() => show(), 800);
    } catch (e) {
      alert('Failed to save tunnel: ' + e);
      show();
    }
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

  function esc(str) {
    const el = document.createElement('span');
    el.textContent = str;
    return el.innerHTML;
  }

  function attr(str) {
    return String(str || '').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  exports.tunnelManager = { init, show };
})(window);
