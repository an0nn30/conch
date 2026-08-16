// SSH Tunnel Manager — modal dialog for creating, starting, stopping, and deleting tunnels.

(function (exports) {
  'use strict';

  let invoke = null;
  let listen = null;
  let serverDataFn = null; // returns { folders, ungrouped, ssh_config }

  // The tl-dialog handle for whichever "base" tunnel dialog is currently
  // open (the manager table, or the New/Edit Tunnel form — these three are
  // mutually exclusive, each replacing the last). removeOverlay() below
  // closes only this — never a blanket DOM query — so it can't reach into
  // ssh/vault/keygen/files/plugin dialogs, or even this module's own
  // delete-confirm/error dialogs, which nest ON TOP of the base dialog and
  // manage their own local dismiss (see showDeleteDialog/showErrorDialog).
  // Same pattern as ssh-panel.js's activeDialogHandle/trackDialogHandle.
  let activeDialogHandle = null;

  function trackDialogHandle(handle) {
    if (!handle) return handle;
    const originalClose = handle.close;
    handle.close = function (result) {
      if (activeDialogHandle === handle) activeDialogHandle = null;
      return originalClose.call(handle, result);
    };
    activeDialogHandle = handle;
    return handle;
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
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') return;

    let handle = null;
    let closed = false;
    const closeManager = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'SSH Tunnels',
      ariaLabel: 'SSH tunnels',
      size: 'lg',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
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
        `;

        const tbody = bodyEl.querySelector('#tunnel-tbody');
        for (const t of tunnels) {
          tbody.appendChild(createTunnelRow(t));
        }
      },
      buttons: [
        { label: 'Close', onSelect: closeManager },
        { label: 'New Tunnel…', onSelect: () => { closeManager(); showNewTunnelForm(); } },
      ],
      onClose: closeManager,
    });

    trackDialogHandle(handle);
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
      statusLabel = 'Connecting…';
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
    moreBtn.textContent = '⋯';
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

  // Nests ON TOP of the manager dialog (still open behind it) rather than
  // replacing it — matches the old overlay, which never called
  // removeOverlay() here and set a raised z-index to layer above the
  // manager. tl-dialog's stacking (depth-based z-index) does that
  // automatically now. Local dismiss only: unlike the base dialogs
  // (manager/new/edit), closing this one never re-shows the manager — it
  // was never replaced, so there's nothing to restore.
  function showDeleteDialog(tunnel) {
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') return;

    let handle = null;
    let dismissed = false;
    const dismiss = () => {
      if (dismissed) return;
      dismissed = true;
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'Delete Tunnel',
      ariaLabel: 'Delete tunnel',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `<div class="ssh-auth-message">Delete <strong>${esc(tunnel.label)}</strong>?</div>`;
      },
      buttons: [
        { label: 'Cancel', onSelect: dismiss },
        { label: 'Delete', primary: true, danger: true, onSelect: async () => {
          dismiss();
          await doDelete(tunnel);
        } },
      ],
      onClose: dismiss,
    });
  }

  // Renders through the shared window.tlMenu component
  // (styles/design-system/components/menu.css, app/ui/tl-menu.js), which
  // owns positioning, dismissal, single-instance behavior, and (since
  // design-system-phase-4) auto-focusing the first item — replacing the
  // first-item .focus() call this function used to do by hand.
  function showRowMenu(e, tunnel, status, errorMsg) {
    if (!window.tlMenu || typeof window.tlMenu.open !== 'function') {
      console.error('tunnel-manager: window.tlMenu is unavailable');
      return;
    }

    const items = [];
    if (status === 'active' || status === 'connecting') {
      items.push({ label: 'Stop', onSelect: () => doStop(tunnel.id) });
    } else {
      items.push({ label: errorMsg ? 'Retry' : 'Start', onSelect: () => doStart(tunnel) });
    }
    items.push({ label: 'Edit', icon: 'edit', onSelect: () => showEditTunnelForm(tunnel) });
    if (errorMsg) {
      items.push({ label: 'View Error', onSelect: () => showErrorDialog('Tunnel Error', errorMsg, () => doStart(tunnel), () => showEditTunnelForm(tunnel)) });
    }
    items.push({ separator: true });
    items.push({ label: 'Delete', icon: 'remove', danger: true, onSelect: () => showDeleteDialog(tunnel) });

    window.tlMenu.open({
      x: e.clientX,
      y: e.clientY,
      items,
      ariaLabel: 'Tunnel actions',
      routerName: 'tunnel-manager-row-context-menu',
    });
  }

  // ---------------------------------------------------------------------------
  // New tunnel form
  // ---------------------------------------------------------------------------

  function showNewTunnelForm() {
    removeOverlay();
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') return;

    const data = serverDataFn ? serverDataFn() : { folders: [], ungrouped: [], ssh_config: [] };
    const allServers = [
      ...data.ungrouped,
      ...(data.folders || []).flatMap((f) => f.entries),
      ...(data.ssh_config || []),
    ];

    const serverOptions = allServers.map((s) => {
      const key = `${s.user}@${s.host}:${s.port}`;
      return { key, label: `${s.label} — ${key}` };
    });

    let handle = null;
    let dismissed = false;
    // No show() on Escape/backdrop — matches the old overlay, which only
    // returned to the manager via the explicit Cancel button (or after a
    // successful/failed Save), never on Escape/backdrop dismissal.
    const dismiss = () => {
      if (dismissed) return;
      dismissed = true;
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'New SSH Tunnel',
      ariaLabel: 'New SSH tunnel',
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="tl-field">
            <span class="tl-field__label">SSH Server</span>
            <select class="tl-combo-select" id="nt-server">
              ${serverOptions.map((s) =>
                `<option value="${attr(s.key)}">${esc(s.label)}</option>`
              ).join('')}
            </select>
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Local Port</span>
            <input type="number" class="tl-input" id="nt-local-port" min="1" max="65535" placeholder="8080" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Remote Host</span>
            <input type="text" class="tl-input" id="nt-remote-host" value="localhost" spellcheck="false" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Remote Port</span>
            <input type="number" class="tl-input" id="nt-remote-port" min="1" max="65535" placeholder="80" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Label (optional)</span>
            <input type="text" class="tl-input" id="nt-label" placeholder="e.g. Web Server" spellcheck="false" />
          </div>
        `;

        bodyEl.querySelectorAll('select.tl-combo-select').forEach((select) => {
          if (window.tlCombo && typeof window.tlCombo.attach === 'function') window.tlCombo.attach(select);
        });
        // Bind/Target port fields — the reference's spinner case.
        bodyEl.querySelectorAll('#nt-local-port, #nt-remote-port').forEach((input) => {
          if (window.tlSpinner && typeof window.tlSpinner.attach === 'function') window.tlSpinner.attach(input);
        });

        const localPortInput = bodyEl.querySelector('#nt-local-port');
        setTimeout(() => { if (localPortInput) localPortInput.focus(); }, 50);
      },
      buttons: [
        { label: 'Cancel', onSelect: () => { dismiss(); show(); } },
        { label: 'Save & Connect', primary: true, onSelect: () => submitNewTunnel(handle.el, dismiss) },
      ],
      onClose: dismiss,
    });

    trackDialogHandle(handle);
  }

  async function submitNewTunnel(panelEl, dismissOverlay) {
    const sessionKey = panelEl.querySelector('#nt-server').value;
    const localPort = parseInt(panelEl.querySelector('#nt-local-port').value, 10);
    const remoteHost = panelEl.querySelector('#nt-remote-host').value.trim() || 'localhost';
    const remotePort = parseInt(panelEl.querySelector('#nt-remote-port').value, 10);
    const label = panelEl.querySelector('#nt-label').value.trim();

    if (!localPort || localPort < 1 || localPort > 65535) {
      window.toast.warn('Invalid Port', 'Local port must be between 1 and 65535.');
      panelEl.querySelector('#nt-local-port').focus();
      return;
    }
    if (!remotePort || remotePort < 1 || remotePort > 65535) {
      window.toast.warn('Invalid Port', 'Remote port must be between 1 and 65535.');
      panelEl.querySelector('#nt-remote-port').focus();
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
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') return;

    const data = serverDataFn ? serverDataFn() : { folders: [], ungrouped: [], ssh_config: [] };
    const allServers = [
      ...data.ungrouped,
      ...(data.folders || []).flatMap((f) => f.entries),
      ...(data.ssh_config || []),
    ];

    const serverOptions = allServers.map((s) => {
      const key = `${s.user}@${s.host}:${s.port}`;
      return { key, label: `${s.label} — ${key}` };
    });

    let handle = null;
    let dismissed = false;
    const dismiss = () => {
      if (dismissed) return;
      dismissed = true;
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'Edit SSH Tunnel',
      ariaLabel: 'Edit SSH tunnel',
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="tl-field">
            <span class="tl-field__label">SSH Server</span>
            <select class="tl-combo-select" id="et-server">
              ${serverOptions.map((s) =>
                `<option value="${attr(s.key)}" ${s.key === tunnel.session_key ? 'selected' : ''}>${esc(s.label)}</option>`
              ).join('')}
            </select>
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Local Port</span>
            <input type="number" class="tl-input" id="et-local-port" value="${tunnel.local_port}" min="1" max="65535" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Remote Host</span>
            <input type="text" class="tl-input" id="et-remote-host" value="${attr(tunnel.remote_host)}" spellcheck="false" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Remote Port</span>
            <input type="number" class="tl-input" id="et-remote-port" value="${tunnel.remote_port}" min="1" max="65535" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Label</span>
            <input type="text" class="tl-input" id="et-label" value="${attr(tunnel.label)}" spellcheck="false" />
          </div>
        `;

        bodyEl.querySelectorAll('select.tl-combo-select').forEach((select) => {
          if (window.tlCombo && typeof window.tlCombo.attach === 'function') window.tlCombo.attach(select);
        });
        // Bind/Target port fields — the reference's spinner case.
        bodyEl.querySelectorAll('#et-local-port, #et-remote-port').forEach((input) => {
          if (window.tlSpinner && typeof window.tlSpinner.attach === 'function') window.tlSpinner.attach(input);
        });

        const localPortInput = bodyEl.querySelector('#et-local-port');
        setTimeout(() => { if (localPortInput) localPortInput.focus(); }, 50);
      },
      buttons: [
        { label: 'Cancel', onSelect: () => { dismiss(); show(); } },
        { label: 'Save', primary: true, onSelect: () => submitEditTunnel(handle.el, tunnel, dismiss) },
      ],
      // Escape/backdrop return to the manager here too — matches the old
      // overlay, where every dismissal path except Save's own (which calls
      // show() itself once after the network round-trip, see
      // submitEditTunnel) re-showed it. Gated on the tl-dialog close
      // reason so Save's dismiss() call (no result argument) doesn't
      // trigger a second, premature show() on top of its own.
      onClose: (result) => {
        dismiss();
        if (result === 'escape' || result === 'backdrop') show();
      },
    });

    trackDialogHandle(handle);
  }

  async function submitEditTunnel(panelEl, original, dismissOverlay) {
    const sessionKey = panelEl.querySelector('#et-server').value;
    const localPort = parseInt(panelEl.querySelector('#et-local-port').value, 10);
    const remoteHost = panelEl.querySelector('#et-remote-host').value.trim() || 'localhost';
    const remotePort = parseInt(panelEl.querySelector('#et-remote-port').value, 10);
    const label = panelEl.querySelector('#et-label').value.trim();

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
  // Error dialog — nests on top of the manager dialog, same rationale and
  // local-dismiss-only contract as showDeleteDialog above.
  // ---------------------------------------------------------------------------

  function showErrorDialog(title, message, onRetry, onEdit) {
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') return;

    let handle = null;
    let dismissed = false;
    const dismiss = () => {
      if (dismissed) return;
      dismissed = true;
      if (handle) handle.close();
    };

    const buttons = [
      { label: 'Dismiss', onSelect: dismiss },
    ];
    if (onEdit) {
      buttons.push({ label: 'Edit', onSelect: () => { dismiss(); onEdit(); } });
    }
    if (onRetry) {
      buttons.push({ label: 'Retry', primary: true, onSelect: () => { dismiss(); onRetry(); } });
    }

    handle = window.tlDialog.open({
      title: title || 'Tunnel Error',
      ariaLabel: title || 'Tunnel error',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `<div class="ssh-error-text">${esc(message)}</div>`;
      },
      buttons,
      onClose: dismiss,
    });
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  function removeOverlay() {
    if (activeDialogHandle) {
      const handle = activeDialogHandle;
      activeDialogHandle = null;
      handle.close();
    }
  }

  const esc = window.utils.esc;
  const attr = window.utils.attr;

  exports.tunnelManager = { init, show, showEdit: showEditTunnelForm, showError: showErrorDialog };
})(window);
