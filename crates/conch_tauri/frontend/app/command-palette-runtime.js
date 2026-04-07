(function initConchCommandPaletteRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listen = deps.listen;
    const esc = deps.esc;
    const handleMenuAction = deps.handleMenuAction;
    const createSshTab = deps.createSshTab;
    const getCurrentPane = deps.getCurrentPane;
    const showStatus = deps.showStatus;
    const refreshTitlebar = deps.refreshTitlebar;
    const refreshSshPanel = deps.refreshSshPanel;
    const MAX_QUICK_RESULTS = 5;
    const COMMAND_CACHE_TTL_MS = 45000;

    let commandPalette = null;
    let commandCache = {
      commands: null,
      builtAt: 0,
      invalidateReason: '',
    };
    let invalidationHooksInstalled = false;

    function invalidateCommandCache(reason) {
      commandCache.commands = null;
      commandCache.builtAt = 0;
      commandCache.invalidateReason = String(reason || 'manual');
    }

    function installInvalidationHooks() {
      if (invalidationHooksInstalled) return;
      invalidationHooksInstalled = true;
      if (typeof listen !== 'function') return;

      const events = [
        'config-changed',
        'plugin-panel-registered',
        'plugin-panels-removed',
        'plugin-widgets-updated',
        'plugin-menu-item',
      ];
      for (const eventName of events) {
        listen(eventName, () => invalidateCommandCache('event:' + eventName)).catch(() => {});
      }
    }

    function fuzzyScore(query, text) {
      const q = (query || '').trim().toLowerCase();
      const t = (text || '').toLowerCase();
      if (!q) return 1;
      let qi = 0;
      let score = 0;
      let lastHit = -2;
      for (let i = 0; i < t.length && qi < q.length; i++) {
        if (t[i] !== q[qi]) continue;
        score += (i === lastHit + 1) ? 3 : 1;
        lastHit = i;
        qi++;
      }
      if (qi !== q.length) return 0;
      return score + Math.max(0, 12 - (t.length - q.length));
    }

    function flattenServers(serverResp) {
      const out = [];
      if (!serverResp) return out;
      for (const s of (serverResp.ungrouped || [])) {
        out.push({ ...s, _group: 'Ungrouped' });
      }
      for (const f of (serverResp.folders || [])) {
        for (const s of (f.entries || [])) {
          out.push({ ...s, _group: f.name || 'Folder' });
        }
      }
      for (const s of (serverResp.ssh_config || [])) {
        out.push({ ...s, _group: '~/.ssh/config' });
      }
      return out;
    }

    function confirmPluginPermissionsForPalette(pluginName, permissions) {
      if (global.conchDialogService && typeof global.conchDialogService.confirmPluginPermissions === 'function') {
        return global.conchDialogService.confirmPluginPermissions(pluginName, permissions);
      }
      if (global.toast && typeof global.toast.error === 'function') {
        global.toast.error('Plugin Permissions', 'Dialog service unavailable; denying permission request.');
      }
      return Promise.resolve(false);
    }

    async function buildPaletteCommands() {
      const [plugins, pluginItems, serverResp, tunnels] = await Promise.all([
        invoke('scan_plugins').catch(() => []),
        invoke('get_plugin_menu_items').catch(() => []),
        invoke('remote_get_servers').catch(() => ({ folders: [], ungrouped: [], ssh_config: [] })),
        invoke('tunnel_get_all').catch(() => []),
      ]);

      const commands = [];
      const add = (id, title, subtitle, keywords, run) => {
        commands.push({ id, title, subtitle, keywords: (keywords || '').toLowerCase(), run });
      };

      add('core:new-tab', 'New Tab', 'Terminal', 'tab terminal create', () => handleMenuAction('new-tab'));
      add('core:new-plain-shell-tab', 'New Plain Shell Tab', 'Terminal', 'tab terminal shell plain default login local pty', () => handleMenuAction('new-plain-shell-tab'));
      add('core:settings', 'Open Settings', 'Application', 'preferences config', () => handleMenuAction('settings'));
      add('core:manage-tunnels', 'Manage Tunnels', 'SSH', 'tunnels manager', () => handleMenuAction('manage-tunnels'));
      add('core:focus-sessions', 'Focus Sessions', 'SSH', 'ssh sessions quick connect', () => handleMenuAction('focus-sessions'));
      add('core:toggle-left', 'Toggle Left Panel', 'View', 'panel left sidebar files explorer tool windows', () => handleMenuAction('toggle-left-panel'));
      add('core:toggle-right', 'Toggle Right Panel', 'View', 'panel right sidebar sessions ssh tool windows', () => handleMenuAction('toggle-right-panel'));
      add('core:toggle-bottom', 'Toggle Bottom Panel', 'View', 'panel bottom', () => handleMenuAction('toggle-bottom-panel'));

      for (const item of (pluginItems || [])) {
        add(
          `plugin-menu:${item.plugin}:${item.action}`,
          `${item.label}`,
          `Plugin: ${item.plugin}`,
          `plugin ${item.plugin} ${item.label} ${item.action}`,
          async () => {
            await invoke('trigger_plugin_menu_action', {
              pluginName: item.plugin,
              action: item.action,
            });
          }
        );
      }

      for (const p of (plugins || [])) {
        if (p.loaded) {
          add(
            `plugin:disable:${p.name}`,
            `Disable Plugin: ${p.name}`,
            `${p.source}`,
            `plugin disable ${p.name}`,
            async () => {
              await invoke('disable_plugin', { name: p.name, source: p.source });
              await invoke('rebuild_menu').catch(() => {});
              invalidateCommandCache('plugin-disabled');
              refreshTitlebar();
            }
          );
        } else {
          add(
            `plugin:enable:${p.name}`,
            `Enable Plugin: ${p.name}`,
            `${p.source}`,
            `plugin enable ${p.name}`,
            async () => {
              const perms = Array.isArray(p.permissions) ? p.permissions.filter(Boolean) : [];
              if (perms.length > 0) {
                const accepted = await confirmPluginPermissionsForPalette(p.name, perms);
                if (!accepted) return;
              }
              await invoke('enable_plugin', { name: p.name, source: p.source, path: p.path });
              await invoke('rebuild_menu').catch(() => {});
              invalidateCommandCache('plugin-enabled');
              refreshTitlebar();
            }
          );
        }
      }

      for (const s of flattenServers(serverResp)) {
        const label = s.label || `${s.user || 'user'}@${s.host || 'host'}`;
        const detail = `${s.user || ''}@${s.host || ''}:${s.port || 22}`.replace(/^@/, '');
        add(
          `ssh:connect:${s.id}`,
          `Connect: ${label}`,
          `${s._group} • ${detail}`,
          `ssh connect server ${label} ${detail} ${s._group}`,
          () => createSshTab({ serverId: s.id })
        );
      }

      for (const t of (tunnels || [])) {
        const status = t.status || 'inactive';
        const isActive = status === 'active' || status === 'connecting';
        if (isActive) {
          add(
            `tunnel:stop:${t.id}`,
            `Stop Tunnel: ${t.label}`,
            `${t.local_port} → ${t.remote_host}:${t.remote_port}`,
            `tunnel stop disconnect ${t.label}`,
            async () => {
              await invoke('tunnel_stop', { tunnelId: t.id });
              invalidateCommandCache('tunnel-stop');
              refreshSshPanel();
            }
          );
        } else {
          add(
            `tunnel:start:${t.id}`,
            `Start Tunnel: ${t.label}`,
            `${t.local_port} → ${t.remote_host}:${t.remote_port}`,
            `tunnel start connect ${t.label}`,
            async () => {
              await invoke('tunnel_start', { tunnelId: t.id });
              invalidateCommandCache('tunnel-start');
              refreshSshPanel();
            }
          );
        }
      }

      return commands;
    }

    async function getPaletteCommands(options) {
      const opts = options || {};
      const forceRefresh = opts.forceRefresh === true;
      const cacheIsFresh = !!commandCache.commands && (Date.now() - commandCache.builtAt) < COMMAND_CACHE_TTL_MS;
      if (!forceRefresh && cacheIsFresh) {
        return commandCache.commands;
      }
      const commands = await buildPaletteCommands();
      commandCache.commands = commands;
      commandCache.builtAt = Date.now();
      commandCache.invalidateReason = '';
      return commands;
    }

    function filterPaletteCommands(commands, query) {
      const q = (query || '').trim().toLowerCase();
      if (!q) return [];
      const scored = [];
      for (const c of commands) {
        const hay = `${c.title} ${c.subtitle} ${c.keywords}`.toLowerCase();
        const score = fuzzyScore(q, hay);
        if (score <= 0) continue;
        scored.push({ c, score });
      }
      scored.sort((a, b) => b.score - a.score || a.c.title.localeCompare(b.c.title));
      return scored.slice(0, MAX_QUICK_RESULTS).map((x) => x.c);
    }

    function quickPickIndexFromKey(event) {
      if (!event || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return null;
      const key = String(event.key || '');
      if (!/^[1-5]$/.test(key)) return null;
      return Number(key) - 1;
    }

    function renderPaletteResults() {
      if (!commandPalette) return;
      const listEl = commandPalette.listEl;
      listEl.innerHTML = '';

      const results = commandPalette.filtered;
      if (!results.length) {
        const empty = document.createElement('div');
        empty.className = 'command-palette-empty';
        const q = (commandPalette.inputEl.value || '').trim();
        empty.textContent = q ? 'No matching commands' : 'Start typing to search commands';
        listEl.appendChild(empty);
        return;
      }

      for (let i = 0; i < results.length; i++) {
        const cmd = results[i];
        const row = document.createElement('div');
        row.className = 'command-palette-item' + (i === commandPalette.selectedIndex ? ' active' : '');
        row.innerHTML =
          `<div class="command-palette-main">` +
            `<div class="command-palette-title">${esc(cmd.title)}</div>` +
            `<div class="command-palette-subtitle">${esc(cmd.subtitle || '')}</div>` +
          `</div>` +
          `<div class="command-palette-shortcut">${i + 1}</div>`;
        row.addEventListener('mouseenter', () => {
          if (!commandPalette || commandPalette.keyboardMode) return;
          commandPalette.selectedIndex = i;
          renderPaletteResults();
        });
        row.addEventListener('click', () => executePaletteCommand(i));
        listEl.appendChild(row);
      }
    }

    function closeCommandPalette(refocus = true) {
      if (!commandPalette) return;
      if (typeof commandPalette.keyHandlerUnregister === 'function') {
        commandPalette.keyHandlerUnregister();
        commandPalette.keyHandlerUnregister = null;
      }
      commandPalette.overlayEl.remove();
      commandPalette = null;
      if (refocus) {
        const pane = getCurrentPane();
        if (pane && pane.term) pane.term.focus();
      }
    }

    async function executePaletteCommand(idx) {
      if (!commandPalette) return;
      const cmd = commandPalette.filtered[idx];
      if (!cmd) return;
      closeCommandPalette(false);
      try {
        await cmd.run();
      } catch (event) {
        showStatus('Command failed: ' + String(event));
      }
      setTimeout(() => {
        if (document.querySelector('.ssh-overlay')) return;
        const pane = getCurrentPane();
        if (pane && pane.term) pane.term.focus();
      }, 80);
    }

    async function openCommandPalette() {
      if (commandPalette) return;

      const overlay = document.createElement('div');
      overlay.className = 'ssh-overlay command-palette-overlay';
      const shell = document.createElement('div');
      shell.className = 'command-palette';
      shell.innerHTML =
        `<input class="command-palette-input" placeholder="Type to search commands... (press 1-5 to run)" spellcheck="false" />` +
        `<div class="command-palette-list"><div class="command-palette-empty">Loading commands…</div></div>`;
      overlay.appendChild(shell);
      document.body.appendChild(overlay);

      const input = shell.querySelector('.command-palette-input');
      const listEl = shell.querySelector('.command-palette-list');
      const state = {
        overlayEl: overlay,
        shellEl: shell,
        inputEl: input,
        listEl,
        allCommands: [],
        filtered: [],
        selectedIndex: 0,
        keyboardMode: false,
        onKeyDown: null,
        keyHandlerUnregister: null,
      };
      commandPalette = state;

      overlay.addEventListener('mousedown', (event) => {
        if (event.target === overlay) closeCommandPalette();
      });

      state.onKeyDown = (event) => {
        if (!commandPalette) return;
        if (event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          closeCommandPalette();
          return true;
        }
        const quickIdx = quickPickIndexFromKey(event);
        if (quickIdx !== null) {
          if (quickIdx < state.filtered.length) {
            event.preventDefault();
            event.stopPropagation();
            executePaletteCommand(quickIdx);
            return true;
          }
          return false;
        }
        if (event.key === 'ArrowDown') {
          event.preventDefault();
          event.stopPropagation();
          state.keyboardMode = true;
          if (state.filtered.length > 0) {
            state.selectedIndex = Math.min(state.selectedIndex + 1, state.filtered.length - 1);
            renderPaletteResults();
          }
          return true;
        }
        if (event.key === 'ArrowUp') {
          event.preventDefault();
          event.stopPropagation();
          state.keyboardMode = true;
          if (state.filtered.length > 0) {
            state.selectedIndex = Math.max(state.selectedIndex - 1, 0);
            renderPaletteResults();
          }
          return true;
        }
        if (event.key === 'Enter') {
          event.preventDefault();
          event.stopPropagation();
          executePaletteCommand(state.selectedIndex);
          return true;
        }
        return false;
      };
      const keyboardRouter = global.conchKeyboardRouter;
      if (keyboardRouter && typeof keyboardRouter.register === 'function') {
        state.keyHandlerUnregister = keyboardRouter.register({
          name: 'command-palette',
          priority: 260,
          isActive: () => !!(commandPalette && commandPalette === state && state.overlayEl && state.overlayEl.isConnected),
          onKeyDown: (event) => state.onKeyDown(event) === true,
        });
      } else {
        console.warn('command-palette: keyboard router unavailable, palette keyboard navigation disabled');
      }

      listEl.addEventListener('mousemove', () => {
        if (!commandPalette) return;
        state.keyboardMode = false;
      });
      input.addEventListener('input', () => {
        if (!commandPalette) return;
        state.keyboardMode = false;
        state.filtered = filterPaletteCommands(state.allCommands, input.value);
        state.selectedIndex = 0;
        renderPaletteResults();
      });

      setTimeout(() => input.focus(), 0);

      try {
        state.allCommands = await getPaletteCommands();
        state.filtered = [];
        state.selectedIndex = 0;
        renderPaletteResults();
      } catch (event) {
        listEl.innerHTML = `<div class="command-palette-empty">Failed to load commands: ${esc(String(event))}</div>`;
      }
    }

    installInvalidationHooks();
    global.__conchInvalidateCommandPaletteCache = invalidateCommandCache;

    return {
      isOpen: () => Boolean(commandPalette),
      open: openCommandPalette,
      close: closeCommandPalette,
      invalidateCache: (reason) => invalidateCommandCache(reason),
    };
  }

  global.conchCommandPaletteRuntime = {
    create,
  };
})(window);
