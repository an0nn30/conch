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
  // Only vault_create/vault_unlock (task-4's inline import vault step) are
  // called through this — reuses the existing vault feature's data-service
  // rather than reaching into the vault crate or duplicating the invoke
  // calls. Loaded before this file in index.html.
  const vaultDataService = exports.termlabVaultFeatureDataService || {};
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

  // Wraps handle.close() so activeDialogHandle self-clears no matter how
  // the dialog actually closes (Escape, backdrop click, a Cancel/Save
  // button, or removeOverlay() below) — every showX feature module already
  // funnels its own dismiss/finish logic through handle.close() at least
  // once (including from tl-dialog's onClose fallback), so wrapping it here
  // catches all of them without changing every feature module to also poke
  // at this panel's private state. Matches keygen.js/files-panel.js, which
  // self-clear their own tracked handle the same way.
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

  function invalidateCommandPaletteCache(reason) {
    if (typeof window.__termlabInvalidateCommandPaletteCache === 'function') {
      window.__termlabInvalidateCommandPaletteCache(reason || 'ssh-panel');
    }
  }

  // Export/Import are reachable from the app menu whether or not the Hosts tool
  // window has ever rendered — in zen mode, or with the right panel closed, it
  // has not, so init() never ran and `invoke` is still null. Fall back to the
  // shared client (the same pattern init() already uses for layoutService)
  // rather than requiring the panel's DOM to exist first.
  function ensureInvoke() {
    if (typeof invoke === 'function') return true;
    const client = window.termlabServices && window.termlabServices.tauriClient;
    if (client && typeof client.invoke === 'function') {
      invoke = client.invoke;
      return true;
    }
    return false;
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

    // A server/tunnel import (legacy JSON or bundle — see
    // share_commands.rs/remote/server_commands.rs) landed, possibly from a
    // different window (windows.rs::create_new_window opens more
    // index.html-backed windows, each running its own copy of this panel
    // with no polling or refresh-on-focus of its own). Refresh so this
    // window's Hosts/Tunnels lists don't go stale relative to what was just
    // imported elsewhere. Harmless no-op refetch on the window that
    // triggered the import itself, since its own import flow already calls
    // refreshAll() directly.
    listen('ssh-config-changed', refreshAll);

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
    if (!ensureInvoke()) {
      if (window.toast) window.toast.error('Export Failed', 'Backend connection unavailable.');
      return;
    }
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

    // "user@host:port", or just "host:port" when the entry has no user — an
    // ~/.ssh/config alias without a User directive used to render as "@host:22".
    const hostLabel = (s) => (s.user ? s.user + '@' : '') + s.host + ':' + s.port;

    // Picker input. Folder and ungrouped hosts default to checked (they are the
    // user's own entries); ~/.ssh/config aliases default to unchecked, matching
    // the previous dialog's defaults.
    const serverGroups = [];
    for (const folder of data.folders) {
      if (!folder.entries.length) continue;
      serverGroups.push({
        label: folder.name,
        entries: folder.entries.map((s) => ({ id: s.id, label: s.label, detail: hostLabel(s), checked: true })),
      });
    }
    if (data.ungrouped.length) {
      serverGroups.push({
        label: 'Ungrouped',
        entries: data.ungrouped.map((s) => ({ id: s.id, label: s.label, detail: hostLabel(s), checked: true })),
      });
    }
    if (data.ssh_config && data.ssh_config.length) {
      serverGroups.push({
        label: '~/.ssh/config',
        entries: data.ssh_config.map((s) => ({ id: s.id, label: s.label, detail: hostLabel(s), checked: false })),
      });
    }

    const tunnelGroups = tunnels.length
      ? [{
          label: 'Tunnels',
          entries: tunnels.map((t) => ({
            id: t.id,
            label: t.label,
            detail: `L${t.local_port} \u2192 ${t.remote_host}:${t.remote_port}`,
            checked: true,
          })),
        }]
      : [];

    const hasServers = serverGroups.length > 0;
    const hasTunnels = tunnels.length > 0;

    // Build a lookup of all servers by their session key (user@host:port).
    const allServers = [];
    for (const f of data.folders) for (const s of f.entries) allServers.push(s);
    for (const s of data.ungrouped) allServers.push(s);
    if (data.ssh_config) for (const s of data.ssh_config) allServers.push(s);

    // Delegates to ssh-store's findServerForTunnel, which mirrors the
    // backend planner's own resolution (server_entry_id first, then
    // session_key matched by host+port) rather than comparing session-key
    // strings verbatim — see that function's doc comment for why the old
    // exact-string match let a dependency the export would actually pull in
    // go unnoticed by this prompt (2026-08-16 review finding I3).
    function findServerForTunnel(t) {
      if (!sshStore || typeof sshStore.findServerForTunnel !== 'function') {
        console.error('ssh-store missing findServerForTunnel');
        return null;
      }
      return sshStore.findServerForTunnel(t, allServers);
    }

    let handle = null;
    let exportPicker = null;
    let closed = false;
    const closeExportDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'Export Connections',
      ariaLabel: 'Export connections',
      // lg (720px): the picker rows carry a label plus user@host:port, which
      // truncates at the 520px md width once a host has a long user or an IPv6
      // literal.
      size: 'lg',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div data-role="picker"></div>
          <div class="ssh-export-section">Credentials</div>
          <div class="tl-check-list"><label class="tl-check"><input type="checkbox" id="exp-include-credentials" />Include saved credentials (the recipient will receive passwords and private keys)</label></div>
          <div class="tl-field" style="margin-top:8px;">
            <label class="tl-field__label" for="exp-password">Bundle password</label>
            <input type="password" class="tl-input" id="exp-password" autocomplete="new-password" />
          </div>
          <div class="tl-field">
            <label class="tl-field__label" for="exp-password-confirm">Confirm password</label>
            <input type="password" class="tl-input" id="exp-password-confirm" autocomplete="new-password" />
          </div>
          <div class="ssh-export-dim">Anyone with this password can read everything in the bundle.</div>
        `;

        const picker = window.termlabExportPicker;
        const host = bodyEl.querySelector('[data-role="picker"]');
        if (picker && typeof picker.mount === 'function' && host) {
          exportPicker = picker.mount(host, [
            hasServers ? { id: 'servers', label: 'Servers', type: 'server', groups: serverGroups } : null,
            hasTunnels ? { id: 'tunnels', label: 'Tunnels', type: 'tunnel', groups: tunnelGroups } : null,
          ].filter(Boolean));
        } else {
          console.error('export picker unavailable');
        }
      },
      onOpen: (panel) => {
        // Drive the Export button's enabled state through its live `disabled`
        // DOM property — tl-dialog's footer-button click gate reads that
        // property, not the `disabled` value passed to buttons[] at open()
        // time (see buildFooterButton's comment in app/ui/tl-dialog.js).
        const bodyEl = panel.querySelector('.tl-dialog__body');
        const footerEnd = panel.querySelector('.tl-dialog__footer-end');
        const exportBtn = footerEnd
          ? Array.from(footerEnd.querySelectorAll('.tl-btn')).find((btn) => btn.textContent === 'Export')
          : null;
        if (!bodyEl || !exportBtn) return;
        const passwordEl = bodyEl.querySelector('#exp-password');
        const confirmEl = bodyEl.querySelector('#exp-password-confirm');
        const shareUi = window.termlabShareUi || { canExport };
        const refreshExportGate = () => {
          const selectedCount = bodyEl.querySelectorAll('input[data-type]:checked').length;
          const enabled = shareUi.canExport({
            selectedCount,
            password: passwordEl ? passwordEl.value : '',
            confirm: confirmEl ? confirmEl.value : '',
          });
          exportBtn.disabled = !enabled;
          exportBtn.setAttribute('aria-disabled', enabled ? 'false' : 'true');
        };
        bodyEl.addEventListener('input', refreshExportGate);
        bodyEl.addEventListener('change', refreshExportGate);
        refreshExportGate();
        if (exportPicker && typeof exportPicker.focusFilter === 'function') exportPicker.focusFilter();
      },
      buttons: [
        { label: 'Cancel', onSelect: closeExportDialog },
        { label: 'Export', primary: true, disabled: true, onSelect: async () => {
          const bodyEl = handle.el;
          let serverIds = [...bodyEl.querySelectorAll('input[data-type="server"]:checked')].map(cb => cb.value);
          const tunnelIds = [...bodyEl.querySelectorAll('input[data-type="tunnel"]:checked')].map(cb => cb.value);

          if (serverIds.length === 0 && tunnelIds.length === 0) {
            if (window.toast) window.toast.error('Export', 'Nothing selected');
            return;
          }

          const includeCredentials = !!bodyEl.querySelector('#exp-include-credentials').checked;
          const password = bodyEl.querySelector('#exp-password').value;
          const confirmPassword = bodyEl.querySelector('#exp-password-confirm').value;
          if (!canExport({ selectedCount: serverIds.length + tunnelIds.length, password, confirm: confirmPassword })) {
            if (window.toast) window.toast.error('Export', 'Enter matching, non-empty passwords.');
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
          // Ids the user was shown and explicitly chose "Export Without" for
          // — sent to the backend so it can skip auto-pulling them (rather
          // than the backend inferring "not relevant" from a plain absence
          // from serverIds, which it cannot tell apart from "declined").
          let declinedServerIds = [];
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
            } else {
              declinedServerIds = dedupedDependencies.map((dep) => dep.server.id);
            }
          }

          closeExportDialog();

          const runExport = async () => {
            try {
              if (!sshDataService || typeof sshDataService.exportBundle !== 'function') {
                throw new Error('SSH data service unavailable: exportBundle');
              }
              const summary = await sshDataService.exportBundle(invoke, serverIds, tunnelIds, includeCredentials, password, declinedServerIds);
              showExportSummary(summary);
            } catch (e) {
              if (String(e) === 'Export cancelled') return;
              console.error('Export failed:', e);
              if (window.toast) window.toast.error('Export Failed', String(e));
            }
          };

          // Preview step (2026-08-16 review finding I3): ask the backend to
          // actually run the planner and report exactly what it would
          // export — auto-pulled hosts, embedded keys, warnings — and let
          // the user cancel before anything is encoded or written. This
          // runs the real planner, not a frontend guess, so it can never
          // disagree with what `exportBundle` below is about to produce.
          const previewThenExport = async () => {
            let preview;
            try {
              if (!sshDataService || typeof sshDataService.previewExport !== 'function') {
                throw new Error('SSH data service unavailable: previewExport');
              }
              preview = await sshDataService.previewExport(invoke, serverIds, tunnelIds, includeCredentials, declinedServerIds);
            } catch (e) {
              console.error('Export preview failed:', e);
              if (window.toast) window.toast.error('Export Failed', String(e));
              return;
            }
            const confirmed = await showExportPreviewDialog(preview);
            if (!confirmed) return;
            await runExport();
          };

          // Credentials must come from an unlocked vault; run the app's
          // existing unlock flow (setup/unlock dialog, or an immediate
          // callback if already unlocked) before hitting the backend, which
          // itself refuses with "Unlock the vault to include credentials"
          // rather than prompting — see share_commands.rs::share_export
          // (share_export_preview has the same requirement).
          if (includeCredentials && window.vault && typeof window.vault.ensureUnlocked === 'function') {
            window.vault.ensureUnlocked(previewThenExport);
          } else {
            await previewThenExport();
          }
        } },
      ],
      onClose: closeExportDialog,
    });
    trackDialogHandle(handle);
  }

  /** Pure gate for the export dialog's Export button — exported below as
   * window.termlabShareUi.canExport so scripts/tests/test_share_export_gate.mjs
   * can exercise it without a browser. */
  function canExport({ selectedCount, password, confirm }) {
    return selectedCount > 0 && !!password && !!confirm && password === confirm;
  }

  /** Show exactly what `share_export_preview` reports would be written —
   * counts, which private keys would be embedded (by filename/comment,
   * never material), auto-pulled dependencies, and warnings — and let the
   * user confirm or cancel before anything is encoded or saved to disk.
   * This is the design spec's export step 5 ("Preview... The user confirms
   * or cancels") and closes 2026-08-16 review finding I3: previously the
   * save dialog ran and the file was written before the user saw any of
   * this. Resolves `true` on Export, `false` on Cancel or dialog close. */
  function showExportPreviewDialog(preview) {
    const p = preview || {};
    const keys = Array.isArray(p.keys) ? p.keys : [];
    const autoPulled = Array.isArray(p.auto_pulled) ? p.auto_pulled : [];
    const warnings = Array.isArray(p.warnings) ? p.warnings : [];
    return new Promise((resolve) => {
      let handle = null;
      let closed = false;
      let decided = false;
      const finish = (result) => {
        decided = true;
        if (closed) return;
        closed = true;
        if (handle) handle.close();
        resolve(result);
      };
      handle = window.tlDialog.open({
        title: 'Confirm Export',
        ariaLabel: 'Confirm export',
        size: 'md',
        body: (bodyEl) => {
          bodyEl.innerHTML = `
            <div>This bundle will contain ${esc(p.servers || 0)} server(s), ${esc(p.tunnels || 0)} tunnel(s), and ${esc(p.credentials || 0)} credential(s).</div>
            ${keys.length ? '<div class="ssh-export-section">Private keys to be embedded</div><ul>' + keys.map((k) => `<li>${esc(k)}</li>`).join('') + '</ul>' : ''}
            ${autoPulled.length ? '<div class="ssh-export-section">Also included</div><ul>' + autoPulled.map((w) => `<li>${esc(w)}</li>`).join('') + '</ul>' : ''}
            ${warnings.length ? '<div class="ssh-export-section">Warnings</div><ul>' + warnings.map((w) => `<li>${esc(w)}</li>`).join('') + '</ul>' : ''}
          `;
        },
        buttons: [
          { label: 'Cancel', onSelect: () => finish(false) },
          { label: 'Export', primary: true, onSelect: () => finish(true) },
        ],
        onClose: () => { if (!decided) finish(false); },
      });
      trackDialogHandle(handle);
    });
  }

  /** Show the export summary — a "Export complete" dialog listing which
   * dependency servers were auto-pulled beyond the user's selection and any
   * warnings, when either is non-empty; otherwise a toast is enough (per
   * task-4 brief). auto_pulled is always surfaced, even with zero warnings
   * — silently dropping it is what previously let an auto-pulled dependency
   * (including its credentials) travel into a bundle unnoticed (2026-08-16
   * review finding). */
  function showExportSummary(summary) {
    const s = summary || {};
    const warnings = Array.isArray(s.warnings) ? s.warnings : [];
    const autoPulled = Array.isArray(s.auto_pulled) ? s.auto_pulled : [];
    if (!warnings.length && !autoPulled.length) {
      if (window.toast) {
        window.toast.info('Export complete', `${s.servers || 0} server(s), ${s.tunnels || 0} tunnel(s), ${s.credentials || 0} credential(s) saved to ${s.path || ''}`);
      }
      return;
    }
    let handle = null;
    let closed = false;
    const close = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };
    handle = window.tlDialog.open({
      title: 'Export complete',
      ariaLabel: 'Export complete',
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div>${esc(s.servers || 0)} server(s), ${esc(s.tunnels || 0)} tunnel(s), ${esc(s.credentials || 0)} credential(s) saved to ${esc(s.path || '')}.</div>
          ${autoPulled.length ? '<div class="ssh-export-section">Also included</div><ul>' + autoPulled.map((w) => `<li>${esc(w)}</li>`).join('') + '</ul>' : ''}
          ${warnings.length ? '<div class="ssh-export-section">Warnings</div><ul>' + warnings.map((w) => `<li>${esc(w)}</li>`).join('') + '</ul>' : ''}
        `;
      },
      buttons: [{ label: 'OK', primary: true, onSelect: close }],
      onClose: close,
    });
    trackDialogHandle(handle);
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

  /** Pick a file (bundle or legacy JSON, routed by the backend's magic-byte
   * check — see share_commands.rs::detect_import_kind), then either prompt
   * for the bundle password or import a legacy file directly, and finish
   * with the unified summary dialog/toast. */
  async function importConfig() {
    if (!ensureInvoke()) {
      if (window.toast) window.toast.error('Import Failed', 'Backend connection unavailable.');
      return;
    }
    let fileInfo;
    try {
      if (!sshDataService || typeof sshDataService.pickImportFile !== 'function') {
        throw new Error('SSH data service unavailable: pickImportFile');
      }
      fileInfo = await sshDataService.pickImportFile(invoke);
    } catch (e) {
      if (String(e) === 'Import cancelled') return;
      console.error('Import failed:', e);
      if (window.toast) window.toast.error('Import Failed', String(e));
      return;
    }

    if (!fileInfo || !fileInfo.path) return;

    if (fileInfo.kind === 'bundle') {
      showImportPasswordDialog(fileInfo.path);
      return;
    }

    await finishImport(fileInfo.path, '');
  }

  /** Call share_import_apply (via data-service's importFile) for an
   * already-picked `path`, refresh the panel and show the summary
   * dialog/toast on success. Returns null on success, or the error message
   * string on failure.
   *
   * `decisions` is optional and forwarded to importFile as-is — omitted
   * (the legacy-file path in importConfig, and finishImport below) it
   * defaults to `[]` inside data-service.js, matching the old combined
   * import command's behaviour: every row keeps the planner's default
   * action. task-5's preview dialog (showImportPreviewDialog below) passes
   * the real per-row overrides the user made there. Fix round 1 (review
   * finding 3): this used to call `invoke('share_import_apply', …)`
   * directly for the preview path, bypassing sshDataService the way every
   * other share_* call in this flow (share_import_plan via planImport)
   * does not — routed through importFile instead, per data-service.js's
   * own comment anticipating exactly this extension. */
  async function runImport(path, password, decisions) {
    try {
      if (!sshDataService || typeof sshDataService.importFile !== 'function') {
        throw new Error('SSH data service unavailable: importFile');
      }
      const summary = await sshDataService.importFile(invoke, path, password, decisions);
      await refreshAll();
      showImportSummary(summary);
      return null;
    } catch (e) {
      return String(e);
    }
  }

  /** Shared tail of the legacy-file import path: call runImport (no
   * decisions — see its doc comment) and toast on failure. Only
   * importConfig's legacy-file branch calls this now — the bundle path
   * (task-4's vault step, task-5's preview dialog) calls runImport directly
   * with real decisions instead. */
  async function finishImport(path, password) {
    const err = await runImport(path, password);
    if (err) {
      console.error('Import failed:', err);
      if (window.toast) window.toast.error('Import Failed', err);
    }
  }

  /** task-5: the preview dialog between the vault step and the apply call.
   * Mounts window.termlabImportPreview (app/features/ssh/import-preview.js)
   * with the rows share_import_plan produced, lets the user override each
   * row's action (and bulk-override by status), and on confirm calls
   * runImport with those real decisions — replacing the decisions: [] shim
   * task-4 left in continueImportAfterPlan and the vault dialogs' success
   * paths. Cancelling aborts the import: share_import_apply is never
   * called.
   *
   * Fix round 1 (review findings 1-2): mounted.isValid() — false while any
   * Rename row is blank — gates the Import button live, on every 'input'
   * and 'change' inside the body (typing a label, changing an action,
   * running a bulk pick can all flip it); import-preview.js also marks the
   * offending input via aria-invalid so a blank row is findable among
   * hundreds. Confirm additionally checks findLabelCollisions against the
   * assembled decisions — isValid() only catches blank labels, not two
   * different rows resolving to the SAME label (e.g. two Renames a user
   * typed identically by hand, past what suggestRenameFor's live avoidance
   * already steers them away from) — and refuses with the offending labels
   * named rather than letting share_import_apply reject the whole batch
   * with one row-less error. */
  function showImportPreviewDialog(path, password, preview) {
    const rows = Array.isArray(preview && preview.rows) ? preview.rows : [];
    let handle = null;
    let closed = false;
    let mounted = null;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };

    const syncImportEnabled = () => {
      if (!handle || !mounted) return;
      const submitBtn = handle.el.querySelector('.tl-dialog__footer .tl-btn--primary');
      if (submitBtn) submitBtn.disabled = !mounted.isValid();
    };

    const confirmImport = async () => {
      if (!mounted) return;
      const decisions = mounted.decisions();

      const findCollisions = window.termlabImportPreview && window.termlabImportPreview.findLabelCollisions;
      const collisions = typeof findCollisions === 'function' ? findCollisions(rows, decisions) : [];
      if (collisions.length) {
        const names = collisions.map((group) => `"${group[0].label}"`).join(', ');
        if (window.toast) {
          window.toast.error(
            'Import Failed',
            `These items would end up with the same name after import: ${names}. Change one of each pair before importing.`,
          );
        }
        return;
      }

      const submitBtn = handle.el.querySelector('.tl-dialog__footer .tl-btn--primary');
      if (submitBtn) submitBtn.disabled = true;
      const err = await runImport(path, password, decisions);
      if (err) {
        if (submitBtn) submitBtn.disabled = false;
        console.error('Import failed:', err);
        if (window.toast) window.toast.error('Import Failed', err);
        return;
      }
      closeDialog();
    };

    handle = window.tlDialog.open({
      title: 'Review Import',
      ariaLabel: 'Review import',
      size: 'lg',
      body: (bodyEl) => {
        if (!window.termlabImportPreview || typeof window.termlabImportPreview.mount !== 'function') {
          bodyEl.innerHTML = '<div class="ssh-export-dim" role="alert">Import preview module unavailable.</div>';
          return;
        }
        mounted = window.termlabImportPreview.mount(bodyEl, rows);
        // 'input' catches every keystroke in a rename field (needed for the
        // button to re-enable/disable live as the user types, not just on
        // blur); 'change' catches action-select and bulk-action picks. Both
        // are registered here rather than assumed covered by mount()'s own
        // internal listeners because THIS handler needs `handle`, which
        // does not exist yet inside this synchronous body callback (tl-
        // dialog.js calls opts.body(bodyEl) before assigning its own return
        // value to the `handle` variable above) — safe because these
        // listeners only ever fire later, once open() has returned and
        // `handle` is set, same as showImportVaultCreateDialog's
        // updateSubmitEnabled precedent.
        bodyEl.addEventListener('input', syncImportEnabled);
        bodyEl.addEventListener('change', syncImportEnabled);
      },
      buttons: [
        { label: 'Cancel', onSelect: closeDialog },
        { label: 'Import', primary: true, onSelect: confirmImport },
      ],
      onClose: closeDialog,
    });
    trackDialogHandle(handle);
    // Initial sync: default_action is never 'rename' (see action_str's doc
    // comment in share_commands.rs), so isValid() is true at mount in
    // practice — but running this once now, rather than trusting that
    // invariant, means the button's start state is never wrong even if that
    // ever changes.
    syncImportEnabled();
  }

  /** One password field + Unlock button, per task-6 brief. On failure
   * (wrong password, not a bundle, newer schema version — see
   * share_commands.rs's ShareError-derived messages) the SAME dialog is
   * re-rendered with an inline error rather than closed and reopened,
   * matching vault/dialogs.js's showUnlockDialog precedent of keeping the
   * dialog open across a failed attempt.
   *
   * task-4: on success this no longer applies the import directly. It
   * calls share_import_plan (via planImport) to learn whether the bundle
   * carries credentials and, if so, what state this machine's vault is in,
   * and hands off to continueImportAfterPlan to resolve that before the
   * apply call. A decode failure (including a wrong bundle password) still
   * surfaces here as an inline error, since share_import_plan decodes the
   * bundle the same way share_import_apply does. */
  function showImportPasswordDialog(path) {
    let handle = null;
    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };
    let errorMsg = '';

    const render = (bodyEl) => {
      bodyEl.innerHTML = `
        <div class="tl-field">
          <label class="tl-field__label" for="imp-bundle-password">Bundle password</label>
          <input type="password" class="tl-input" id="imp-bundle-password" autocomplete="current-password" />
        </div>
        ${errorMsg ? `<div class="ssh-export-dim" role="alert">${esc(errorMsg)}</div>` : ''}
      `;
      const input = bodyEl.querySelector('#imp-bundle-password');
      if (input) {
        input.addEventListener('keydown', (event) => {
          if (event.key !== 'Enter') return;
          event.preventDefault();
          submitUnlock();
        });
        setTimeout(() => input.focus(), 50);
      }
    };

    const submitUnlock = async () => {
      const bodyEl = handle.el.querySelector('.tl-dialog__body');
      const input = bodyEl ? bodyEl.querySelector('#imp-bundle-password') : null;
      const submitBtn = handle.el.querySelector('.tl-dialog__footer .tl-btn--primary');
      const password = input ? input.value : '';
      if (submitBtn) submitBtn.disabled = true;
      if (input) input.disabled = true;

      let preview;
      try {
        if (!sshDataService || typeof sshDataService.planImport !== 'function') {
          throw new Error('SSH data service unavailable: planImport');
        }
        preview = await sshDataService.planImport(invoke, path, password);
      } catch (e) {
        errorMsg = String(e);
        if (submitBtn) submitBtn.disabled = false;
        if (input) input.disabled = false;
        if (bodyEl) render(bodyEl);
        return;
      }

      closeDialog();
      await continueImportAfterPlan(path, password, preview);
    };

    handle = window.tlDialog.open({
      title: 'Import Bundle',
      ariaLabel: 'Import bundle',
      size: 'sm',
      body: render,
      buttons: [
        { label: 'Cancel', onSelect: closeDialog },
        { label: 'Unlock', primary: true, onSelect: submitUnlock },
      ],
      onClose: closeDialog,
    });
    trackDialogHandle(handle);
  }

  /** task-4: the vault step. Runs after share_import_plan has decoded the
   * bundle and reported `includes_credentials`/`vault_state`. A
   * credentials-free bundle, or one whose vault is already unlocked,
   * proceeds straight through to the preview dialog (task-5). "locked" and
   * "absent" show a dialog to unlock/create the vault first; the preview
   * dialog only opens once that succeeds. Cancelling either vault dialog
   * aborts the import — share_import_apply is never called, so nothing is
   * written. */
  async function continueImportAfterPlan(path, password, preview) {
    const p = preview || {};
    if (!p.includes_credentials || p.vault_state === 'unlocked') {
      showImportPreviewDialog(path, password, p);
      return;
    }
    if (p.vault_state === 'locked') {
      showImportVaultUnlockDialog(path, password);
      return;
    }
    showImportVaultCreateDialog(path, password);
  }

  /** I2 (2026-08-17 review): re-run share_import_plan now that the vault's
   * state has just changed (unlocked or created), rather than carrying the
   * preview captured while it was locked/absent through unchanged.
   *
   * `share_import_plan` classifies a bundled credential against
   * `existing_account_ids`, which share_commands.rs's `do_import_plan`
   * only populates `if !vault_mgr.is_locked()` — so the preview shown
   * *before* this vault step ran had an empty id list and classified every
   * bundled credential as New/Add regardless of what the vault actually
   * already held. Threading that stale preview through unchanged (the
   * pre-fix behaviour) let a user approve a credential row labelled "New"
   * that apply time — once the vault really is unlocked and re-plans
   * against it — correctly calls SameId/Replace, silently overwriting an
   * existing credential the user was never shown the chance to Skip.
   *
   * Returns the fresh preview, or null if the re-plan itself failed (a
   * toast is already shown in that case — the caller only needs to check
   * for null and stop). */
  async function replanAfterVaultChange(path, password) {
    try {
      if (!sshDataService || typeof sshDataService.planImport !== 'function') {
        throw new Error('SSH data service unavailable: planImport');
      }
      return await sshDataService.planImport(invoke, path, password);
    } catch (e) {
      console.error('Re-plan after vault change failed:', e);
      if (window.toast) window.toast.error('Import Failed', String(e));
      return null;
    }
  }

  /** task-4, vault_state "locked": one password field + Unlock button,
   * calling the existing `vault_unlock` command (vault_commands.rs:137) —
   * not a new command. On failure (e.g. "Incorrect master password") the
   * SAME dialog re-shows with an inline error, matching
   * showImportPasswordDialog's precedent above. Cancelling aborts the
   * import: share_import_apply is never called. On success, the import is
   * re-planned from scratch (see replanAfterVaultChange's doc comment —
   * fix round 2, I2) rather than reusing the preview captured while the
   * vault was still locked — so no `preview` parameter is threaded in
   * here any more. */
  function showImportVaultUnlockDialog(path, password) {
    let handle = null;
    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };
    let errorMsg = '';

    const render = (bodyEl) => {
      bodyEl.innerHTML = `
        <div>This bundle contains saved credentials. Unlock your vault to import them.</div>
        <div class="tl-field">
          <label class="tl-field__label" for="imp-vault-unlock-password">Master password</label>
          <input type="password" class="tl-input" id="imp-vault-unlock-password" autocomplete="current-password" />
        </div>
        ${errorMsg ? `<div class="ssh-export-dim" role="alert">${esc(errorMsg)}</div>` : ''}
      `;
      const input = bodyEl.querySelector('#imp-vault-unlock-password');
      if (input) {
        input.addEventListener('keydown', (event) => {
          if (event.key !== 'Enter') return;
          event.preventDefault();
          submitUnlock();
        });
        setTimeout(() => input.focus(), 50);
      }
    };

    const submitUnlock = async () => {
      const bodyEl = handle.el.querySelector('.tl-dialog__body');
      const input = bodyEl ? bodyEl.querySelector('#imp-vault-unlock-password') : null;
      const submitBtn = handle.el.querySelector('.tl-dialog__footer .tl-btn--primary');
      const vaultPassword = input ? input.value : '';
      if (submitBtn) submitBtn.disabled = true;
      if (input) input.disabled = true;

      try {
        if (!vaultDataService || typeof vaultDataService.unlockVault !== 'function') {
          throw new Error('Vault data service unavailable: unlockVault');
        }
        await vaultDataService.unlockVault(invoke, vaultPassword);
      } catch (e) {
        errorMsg = String(e);
        if (submitBtn) submitBtn.disabled = false;
        if (input) input.disabled = false;
        if (bodyEl) render(bodyEl);
        return;
      }

      closeDialog();
      const freshPreview = await replanAfterVaultChange(path, password);
      if (!freshPreview) return;
      showImportPreviewDialog(path, password, freshPreview);
    };

    handle = window.tlDialog.open({
      title: 'Unlock Vault',
      ariaLabel: 'Unlock vault to import credentials',
      size: 'sm',
      body: render,
      buttons: [
        { label: 'Cancel', onSelect: closeDialog },
        { label: 'Unlock', primary: true, onSelect: submitUnlock },
      ],
      onClose: closeDialog,
    });
    trackDialogHandle(handle);
  }

  /** task-4, vault_state "absent": master password + confirm, submitted
   * through the existing `vault_create` command (vault_commands.rs:122).
   * Whatever password rule `VaultManager::create` enforces is the only rule
   * enforced here — this dialog only checks presence and that the two
   * fields match, and surfaces the backend's rejection (e.g. "master
   * password must be at least 8 characters") via `errorMsg` below rather
   * than pre-checking length itself (fix round 2, I4 — this is now also
   * true of `vault/dialogs.js`'s showSetupDialog, which used to enforce its
   * own, different minimum). The Create Vault button's enabled state is
   * driven off the live `disabled` DOM property on the 'input' events
   * below, not the `disabled` value passed to `buttons[]` at open time —
   * tl-dialog's footer click gate reads the live property (see
   * buildFooterButton in ui/tl-dialog.js; this exact bug — gating on the
   * stale build-time value — was found and fixed earlier in this project).
   * Cancelling aborts the import: share_import_apply is never called. On
   * success, the import is re-planned from scratch (see
   * replanAfterVaultChange's doc comment — fix round 2, I2) rather than
   * reusing the preview captured before the vault existed — so no
   * `preview` parameter is threaded in here any more. */
  function showImportVaultCreateDialog(path, password) {
    let handle = null;
    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };
    let errorMsg = '';

    const updateSubmitEnabled = () => {
      const bodyEl = handle.el.querySelector('.tl-dialog__body');
      const submitBtn = handle.el.querySelector('.tl-dialog__footer .tl-btn--primary');
      if (!bodyEl || !submitBtn) return;
      const pwInput = bodyEl.querySelector('#imp-vault-create-password');
      const confirmInput = bodyEl.querySelector('#imp-vault-create-confirm');
      const pwVal = pwInput ? pwInput.value : '';
      const confirmVal = confirmInput ? confirmInput.value : '';
      submitBtn.disabled = !pwVal || !confirmVal || pwVal !== confirmVal;
    };

    const render = (bodyEl) => {
      bodyEl.innerHTML = `
        <div>This bundle contains saved credentials. To store them you need a vault on this machine. Pick a master password — you'll use it to unlock the vault from now on.</div>
        <div class="tl-field">
          <label class="tl-field__label" for="imp-vault-create-password">Master password</label>
          <input type="password" class="tl-input" id="imp-vault-create-password" autocomplete="new-password" />
        </div>
        <div class="tl-field">
          <label class="tl-field__label" for="imp-vault-create-confirm">Confirm password</label>
          <input type="password" class="tl-input" id="imp-vault-create-confirm" autocomplete="new-password" />
        </div>
        ${errorMsg ? `<div class="ssh-export-dim" role="alert">${esc(errorMsg)}</div>` : ''}
      `;
      const pwInput = bodyEl.querySelector('#imp-vault-create-password');
      const confirmInput = bodyEl.querySelector('#imp-vault-create-confirm');
      for (const el of [pwInput, confirmInput]) {
        if (!el) continue;
        el.addEventListener('input', updateSubmitEnabled);
        el.addEventListener('keydown', (event) => {
          if (event.key !== 'Enter') return;
          event.preventDefault();
          submitCreate();
        });
      }
      setTimeout(() => { if (pwInput) pwInput.focus(); }, 50);
    };

    const submitCreate = async () => {
      const bodyEl = handle.el.querySelector('.tl-dialog__body');
      const submitBtn = handle.el.querySelector('.tl-dialog__footer .tl-btn--primary');
      if (submitBtn && submitBtn.disabled) return;
      const pwInput = bodyEl ? bodyEl.querySelector('#imp-vault-create-password') : null;
      const confirmInput = bodyEl ? bodyEl.querySelector('#imp-vault-create-confirm') : null;
      const vaultPassword = pwInput ? pwInput.value : '';
      if (submitBtn) submitBtn.disabled = true;
      if (pwInput) pwInput.disabled = true;
      if (confirmInput) confirmInput.disabled = true;

      try {
        if (!vaultDataService || typeof vaultDataService.createVault !== 'function') {
          throw new Error('Vault data service unavailable: createVault');
        }
        await vaultDataService.createVault(invoke, vaultPassword);
      } catch (e) {
        errorMsg = String(e);
        // Fields come back empty after this re-render, so leaving submitBtn
        // disabled (set above) is already the correct state — no explicit
        // reset needed, unlike the unlock dialogs above.
        if (bodyEl) render(bodyEl);
        return;
      }

      closeDialog();
      const freshPreview = await replanAfterVaultChange(path, password);
      if (!freshPreview) return;
      showImportPreviewDialog(path, password, freshPreview);
    };

    handle = window.tlDialog.open({
      title: 'Create Vault',
      ariaLabel: 'Create vault to import credentials',
      size: 'sm',
      body: render,
      buttons: [
        { label: 'Cancel', onSelect: closeDialog },
        { label: 'Create Vault', primary: true, disabled: true, onSelect: submitCreate },
      ],
      onClose: closeDialog,
    });
    trackDialogHandle(handle);
  }

  /** Show the unified import summary — "Imported N host(s), M tunnel(s), K
   * credential(s). S skipped." (task-6 brief) — as a toast when there is
   * nothing more to say, or a dialog listing skipped entries and, when
   * credentials were held back for lack of an unlocked vault, the line
   * telling the user how to get them next time.
   *
   * Fix round 2, C1: also mentions when `summary.reconciled` is nonzero —
   * `share_commands.rs`'s `apply_decisions` silently fell back to a fresh
   * planner default for that many rows because the decision this dialog's
   * caller sent no longer matched what the row actually was by apply time
   * (a retried import, or another window that got there first). The import
   * still completed; this just tells the user something moved under them
   * rather than staying silent about it. */
  function showImportSummary(summary) {
    const s = summary || {};
    const skipped = Array.isArray(s.skipped) ? s.skipped : [];
    const heldBack = !!s.credentials_held_back;
    const reconciled = Number(s.reconciled) || 0;
    const headline = `Imported ${s.servers || 0} host(s), ${s.tunnels || 0} tunnel(s), ${s.credentials || 0} credential(s). ${skipped.length} skipped.`;

    if (!skipped.length && !heldBack && !reconciled) {
      if (window.toast) window.toast.info('Import complete', headline);
      return;
    }

    let handle = null;
    let closed = false;
    const close = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };
    handle = window.tlDialog.open({
      title: 'Import complete',
      ariaLabel: 'Import complete',
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div>${esc(headline)}</div>
          ${heldBack ? '<div class="ssh-export-note">Credentials were not imported because this machine has no unlocked vault. Create or unlock your vault and import again.</div>' : ''}
          ${reconciled ? `<div class="ssh-export-note">${reconciled} item(s) changed since you reviewed them, so they were applied with their current default action instead of the one you picked.</div>` : ''}
          ${skipped.length ? '<div class="ssh-export-section">Skipped</div><ul>' + skipped.map((w) => `<li>${esc(w)}</li>`).join('') + '</ul>' : ''}
        `;
      },
      buttons: [{ label: 'OK', primary: true, onSelect: close }],
      onClose: close,
    });
    trackDialogHandle(handle);
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
      if (handle) { trackDialogHandle(handle); return; }
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
      if (handle) { trackDialogHandle(handle); return; }
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
      if (handle) { trackDialogHandle(handle); return; }
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
      if (handle) { trackDialogHandle(handle); return; }
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
  // Pure export-dialog gate logic, exposed for scripts/tests/test_share_export_gate.mjs
  // (matches tl-icon.js's/_tl-dialog.js's precedent for exposing pure logic for
  // browser-less testing).
  exports.termlabShareUi = { canExport };
})(window);
