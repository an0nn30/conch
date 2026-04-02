(function initConchCommandPaletteRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const esc = deps.esc;
    const handleMenuAction = deps.handleMenuAction;
    const createSshTab = deps.createSshTab;
    const getCurrentPane = deps.getCurrentPane;
    const showStatus = deps.showStatus;
    const refreshTitlebar = deps.refreshTitlebar;
    const refreshSshPanel = deps.refreshSshPanel;

    let commandPalette = null;

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
      return new Promise((resolve) => {
        const overlay = document.createElement('div');
        overlay.className = 'ssh-overlay';
        const items = permissions
          .map((p) => `<div style="font-size:12px; color:var(--fg); line-height:1.5;">• ${esc(p)}</div>`)
          .join('');
        overlay.innerHTML = `
          <div class="ssh-form" style="min-width:420px; max-width:620px;">
            <div class="ssh-form-title">Plugin Permissions</div>
            <div class="ssh-form-body">
              <div style="margin-bottom:10px; font-size:12px; color:var(--fg);">
                Plugin "${esc(pluginName)}" requests:
              </div>
              <div style="display:flex; flex-direction:column; gap:4px; margin-bottom:12px;">
                ${items}
              </div>
              <div style="font-size:12px; color:var(--dim-fg);">Allow and enable this plugin?</div>
            </div>
            <div class="ssh-form-buttons">
              <button class="ssh-form-btn" id="cpp-deny">Deny</button>
              <button class="ssh-form-btn primary" id="cpp-allow">Allow</button>
            </div>
          </div>`;
        const done = (accepted) => {
          document.removeEventListener('keydown', onKey, true);
          overlay.remove();
          resolve(accepted);
        };
        const onKey = (event) => {
          if (event.key !== 'Escape') return;
          event.preventDefault();
          event.stopPropagation();
          done(false);
        };
        overlay.addEventListener('mousedown', (event) => {
          if (event.target === overlay) done(false);
        });
        overlay.querySelector('#cpp-deny').addEventListener('click', () => done(false));
        overlay.querySelector('#cpp-allow').addEventListener('click', () => done(true));
        document.addEventListener('keydown', onKey, true);
        document.body.appendChild(overlay);
      });
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
      add('core:settings', 'Open Settings', 'Application', 'preferences config', () => handleMenuAction('settings'));
      add('core:manage-tunnels', 'Manage Tunnels', 'SSH', 'tunnels manager', () => handleMenuAction('manage-tunnels'));
      add('core:focus-sessions', 'Focus Sessions', 'SSH', 'ssh sessions quick connect', () => handleMenuAction('focus-sessions'));
      add('core:toggle-left', 'Toggle File Explorer', 'View', 'panel files explorer', () => handleMenuAction('toggle-left-panel'));
      add('core:toggle-right', 'Toggle Sessions Panel', 'View', 'panel ssh sessions', () => handleMenuAction('toggle-right-panel'));
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
              refreshSshPanel();
            }
          );
        }
      }

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
      return scored.map((x) => x.c);
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
          `<div class="command-palette-title">${esc(cmd.title)}</div>` +
          `<div class="command-palette-subtitle">${esc(cmd.subtitle || '')}</div>`;
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
      document.removeEventListener('keydown', commandPalette.onKeyDown, true);
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
        `<input class="command-palette-input" placeholder="Type to search commands (connect, tunnel, plugin)..." spellcheck="false" />` +
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
          return;
        }
        if (event.key === 'ArrowDown') {
          event.preventDefault();
          event.stopPropagation();
          state.keyboardMode = true;
          if (state.filtered.length > 0) {
            state.selectedIndex = Math.min(state.selectedIndex + 1, state.filtered.length - 1);
            renderPaletteResults();
          }
          return;
        }
        if (event.key === 'ArrowUp') {
          event.preventDefault();
          event.stopPropagation();
          state.keyboardMode = true;
          if (state.filtered.length > 0) {
            state.selectedIndex = Math.max(state.selectedIndex - 1, 0);
            renderPaletteResults();
          }
          return;
        }
        if (event.key === 'Enter') {
          event.preventDefault();
          event.stopPropagation();
          executePaletteCommand(state.selectedIndex);
        }
      };
      document.addEventListener('keydown', state.onKeyDown, true);

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
        state.allCommands = await buildPaletteCommands();
        state.filtered = [];
        state.selectedIndex = 0;
        renderPaletteResults();
      } catch (event) {
        listEl.innerHTML = `<div class="command-palette-empty">Failed to load commands: ${esc(String(event))}</div>`;
      }
    }

    return {
      isOpen: () => Boolean(commandPalette),
      open: openCommandPalette,
      close: closeCommandPalette,
    };
  }

  global.conchCommandPaletteRuntime = {
    create,
  };
})(window);
