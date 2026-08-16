(function initTermLabCommandPaletteRuntime(global) {
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
    // Above 5 (the digit-quick-pick range, unchanged — quickPickIndexFromKey
    // only ever matches keys 1-5, so raising this cap cannot put a digit
    // shortcut on anything past the fifth flat result). No reference exists
    // for how many rows IntelliJ's own Search Everywhere shows before its
    // "N more items" affordance, so this is a first-pass judgment call, not
    // a measured value.
    const MAX_RESULTS = 20;
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
      if (global.termlabDialogService && typeof global.termlabDialogService.confirmPluginPermissions === 'function') {
        return global.termlabDialogService.confirmPluginPermissions(pluginName, permissions);
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
      // group buckets the flat, cross-category fuzzy-match results under a
      // Search Everywhere-style header (renderPaletteResults()) — a broader
      // classification than `subtitle`, which stays specific (a plugin
      // name, an ssh detail string, ...). Defaults to 'Actions' so every
      // existing add() call that predates grouping still lands somewhere.
      const add = (id, title, subtitle, keywords, run, group) => {
        commands.push({ id, title, subtitle, keywords: (keywords || '').toLowerCase(), run, group: group || 'Actions' });
      };

      add('core:new-tab', 'New Tab', 'Terminal', 'tab terminal create', () => handleMenuAction('new-tab'));
      add('core:new-plain-shell-tab', 'New Plain Shell Tab', 'Terminal', 'tab terminal shell plain default login local pty', () => handleMenuAction('new-plain-shell-tab'));
      add('core:settings', 'Open Settings', 'Application', 'preferences config', () => handleMenuAction('settings'));
      add('core:manage-tunnels', 'Manage Tunnels', 'SSH', 'tunnels manager', () => handleMenuAction('manage-tunnels'), 'Tunnels');
      add('core:focus-sessions', 'Focus Sessions', 'SSH', 'ssh sessions quick connect', () => handleMenuAction('focus-sessions'), 'SSH Hosts');
      add('core:toggle-left', 'Toggle Left Panel', 'View', 'panel left sidebar files explorer tool windows', () => handleMenuAction('toggle-left-panel'));
      add('core:toggle-right', 'Toggle Right Panel', 'View', 'panel right sidebar sessions ssh tool windows', () => handleMenuAction('toggle-right-panel'));
      add('core:toggle-bottom', 'Toggle Bottom Panel', 'View', 'panel bottom', () => handleMenuAction('toggle-bottom-panel'));
      add('core:notifications', 'Show Notifications', 'View', 'notifications history alerts toast tool window', () => handleMenuAction('notifications'));

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
            },
            'Plugins'
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
            },
            'Plugins'
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
          () => createSshTab({ serverId: s.id }),
          'SSH Hosts'
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
            },
            'Tunnels'
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
            },
            'Tunnels'
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
      return scored.slice(0, MAX_RESULTS).map((x) => x.c);
    }

    // Stable-partitions a fuzzy-score-sorted result list into
    // Search-Everywhere-style named sections, in first-appearance order — so
    // the group holding the single best match sorts first, and items keep
    // their relative score order within their own group — and returns the
    // FLATTENED result: one array, in the exact order rows will render in.
    //
    // This must be the ONLY reordering step, applied once, right where
    // `filtered` is produced (below), not a second time inside
    // renderPaletteResults(). Review round 1 (phase 5b task 3) found a bug
    // where renderPaletteResults() grouped `commandPalette.filtered` for
    // display while every execution path (digit quick-pick, click,
    // Enter-after-arrow) indexed into the *un*grouped `filtered` array — two
    // different orderings sharing one index space, so whenever a query's top
    // matches spanned more than one group, pressing digit N (or clicking, or
    // arrowing to and pressing Enter on) the row visibly at position N could
    // run a *different* command than the one displayed there. Folding the
    // grouping into `filtered` itself — so render and execution share the
    // same array, not two independently-ordered ones — makes that class of
    // bug structurally impossible rather than just currently-not-happening.
    function orderResultsByGroup(results) {
      const order = [];
      const buckets = new Map();
      for (const cmd of results) {
        const name = cmd.group || 'Actions';
        if (!buckets.has(name)) {
          buckets.set(name, []);
          order.push(name);
        }
        buckets.get(name).push(cmd);
      }
      const flat = [];
      for (const name of order) {
        for (const cmd of buckets.get(name)) flat.push(cmd);
      }
      return flat;
    }

    function quickPickIndexFromKey(event) {
      if (!event || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return null;
      const key = String(event.key || '');
      if (!/^[1-5]$/.test(key)) return null;
      return Number(key) - 1;
    }

    // Per-result icon (window.tlIcon), keyed off the command id prefix set
    // by buildPaletteCommands() above. Not every command has a fitting
    // vendored icon (vendor/intellij-icons is a small fixed set — see
    // app/tool-window-runtime.js's 'tunnels' registration, which has the
    // same "no vendored plug-like icon yet" gap for the same reason) — those
    // fall through to null and render an empty icon gutter rather than a
    // broken image, matching tl-menu.js's optional-icon convention.
    function paletteIconFor(cmd) {
      const id = String((cmd && cmd.id) || '');
      if (id === 'core:new-tab' || id === 'core:new-plain-shell-tab') return 'terminal';
      if (id === 'core:settings') return 'settings';
      if (id === 'core:focus-sessions') return 'web';
      if (id === 'core:toggle-left') return 'folder';
      if (id === 'core:toggle-right') return 'web';
      if (id === 'core:notifications') return 'notifications';
      if (id.indexOf('ssh:connect:') === 0) return 'web';
      if (id.indexOf('plugin:enable:') === 0) return 'add';
      if (id.indexOf('plugin:disable:') === 0) return 'remove';
      return null;
    }

    // Renders `commandPalette.filtered` — and ONLY `commandPalette.filtered`,
    // already in its final group-then-score render order courtesy of
    // orderResultsByGroup() above — top to bottom, opening a new
    // .tl-palette__group section whenever the group name changes. Since
    // grouping is a stable partition, same-group items in `filtered` are
    // already contiguous, so a group name change here can only mean "start
    // of the next section," never "back to an earlier one." Row index `idx`
    // is this same array's index — the one and only index space shared with
    // quickPickIndexFromKey()/click/Enter's `filtered[idx]` lookups below.
    function renderPaletteResults() {
      if (!commandPalette) return;
      const listEl = commandPalette.listEl;
      listEl.innerHTML = '';

      const results = commandPalette.filtered;
      if (!results.length) {
        const empty = document.createElement('div');
        empty.className = 'tl-palette__empty';
        const q = (commandPalette.inputEl.value || '').trim();
        empty.textContent = q ? 'No matching commands' : 'Start typing to search commands';
        listEl.appendChild(empty);
        return;
      }

      let groupEl = null;
      let currentGroupName = null;
      let selectedRowEl = null;
      for (let idx = 0; idx < results.length; idx++) {
        const cmd = results[idx];
        const groupName = cmd.group || 'Actions';
        if (groupName !== currentGroupName) {
          currentGroupName = groupName;
          groupEl = document.createElement('div');
          groupEl.className = 'tl-palette__group';
          const titleEl = document.createElement('div');
          titleEl.className = 'tl-palette__group-title';
          titleEl.textContent = groupName;
          groupEl.appendChild(titleEl);
          listEl.appendChild(groupEl);
        }

        const row = document.createElement('div');
        row.className = 'tl-palette__item' + (idx === commandPalette.selectedIndex ? ' is-active' : '');

        const iconEl = document.createElement('div');
        iconEl.className = 'tl-palette__icon';
        const iconName = paletteIconFor(cmd);
        if (iconName && global.tlIcon && typeof global.tlIcon.create === 'function') {
          iconEl.appendChild(global.tlIcon.create(iconName, { size: 16, alt: '' }));
        }
        row.appendChild(iconEl);

        const mainEl = document.createElement('div');
        mainEl.className = 'tl-palette__main';
        const titleRowEl = document.createElement('div');
        titleRowEl.className = 'tl-palette__title';
        titleRowEl.textContent = cmd.title;
        mainEl.appendChild(titleRowEl);
        if (cmd.subtitle) {
          const subtitleEl = document.createElement('div');
          subtitleEl.className = 'tl-palette__subtitle';
          subtitleEl.textContent = cmd.subtitle;
          mainEl.appendChild(subtitleEl);
        }
        row.appendChild(mainEl);

        // Only the first five rendered rows ever get a digit badge —
        // quickPickIndexFromKey() only matches keys 1-5, so this stays in
        // sync with what digit-1-5 can actually reach even though the total
        // result cap (MAX_RESULTS) is well above 5.
        if (idx < MAX_QUICK_RESULTS) {
          const shortcutEl = document.createElement('div');
          shortcutEl.className = 'tl-palette__shortcut';
          shortcutEl.textContent = String(idx + 1);
          row.appendChild(shortcutEl);
        }

        row.addEventListener('mouseenter', () => {
          if (!commandPalette || commandPalette.keyboardMode) return;
          commandPalette.selectedIndex = idx;
          renderPaletteResults();
        });
        row.addEventListener('click', () => executePaletteCommand(idx));
        groupEl.appendChild(row);
        if (idx === commandPalette.selectedIndex) selectedRowEl = row;
      }

      // MAX_RESULTS went from 5 to 20 (see above) but .tl-palette__results
      // is a fixed height: min(480px, 66vh) scrolling box (palette.css) at
      // ~43px/row, so only about half the results fit without scrolling.
      // Every call site that moves commandPalette.selectedIndex (ArrowUp/
      // ArrowDown below, digit quick-pick, mouse hover-select, and the
      // index-0 reset on each keystroke) re-renders through this function,
      // so scrolling the selected row into view here — rather than at each
      // call site — covers all of them in one place. Without this, arrow-
      // navigating past the fold moved the selection but not the viewport:
      // the highlight scrolled out of sight, further ArrowDown presses
      // looked like no-ops, and Enter ran a command the user couldn't see
      // (phase 5b review finding 3). 'nearest' avoids yanking the list to
      // center/top on every keystroke when the selected row is already
      // visible.
      if (selectedRowEl && typeof selectedRowEl.scrollIntoView === 'function') {
        selectedRowEl.scrollIntoView({ block: 'nearest' });
      }
    }

    // Single teardown path for the palette's own state (unregister the
    // arrow/enter/digit/escape key handler, clear the module-level
    // reference, optionally refocus the terminal) — invoked from
    // tlDialog's onClose so it runs on every dialog-close route (our own
    // Escape handling below, a backdrop click, which tl-dialog handles
    // internally and never routes through closeCommandPalette()), not just
    // the one this module drives directly. Mirrors settings/renderers.js's
    // renderDialogShell() onClose contract.
    function teardownPaletteState(state, refocus) {
      if (typeof state.keyHandlerUnregister === 'function') {
        state.keyHandlerUnregister();
        state.keyHandlerUnregister = null;
      }
      if (commandPalette === state) commandPalette = null;
      if (refocus) {
        // Same tlDialog.count() guard executePaletteCommand's refocus below
        // uses, and for the same reason: don't steal focus into the
        // terminal if another dialog (e.g. Settings) is still open behind
        // this one. tl-dialog's close() (app/ui/tl-dialog.js) pops this
        // entry off its stack BEFORE calling onClose -> here, so count()
        // already reflects only what's left. Missing here previously
        // (phase 5b review finding 13): dismissing the palette via backdrop
        // click while Settings was open underneath refocused the terminal
        // behind the still-open modal.
        const tlDialogOpen = !!(global.tlDialog && typeof global.tlDialog.count === 'function' && global.tlDialog.count() > 0);
        if (tlDialogOpen) return;
        const pane = getCurrentPane();
        if (pane && pane.term) pane.term.focus();
      }
    }

    function closeCommandPalette(refocus = true) {
      if (!commandPalette) return;
      const state = commandPalette;
      state.pendingRefocus = refocus !== false;
      if (state.dialogHandle && typeof state.dialogHandle.close === 'function') {
        // Routes through tl-dialog's own close(), which fires onClose ->
        // teardownPaletteState(), so this and a backdrop-click dismissal
        // converge on the same cleanup path.
        state.dialogHandle.close();
      } else {
        teardownPaletteState(state, state.pendingRefocus);
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
        // Don't steal focus back into the terminal if cmd.run() opened some
        // other dialog that should keep it instead (e.g. "Open Settings",
        // or a plugin-permission/ssh-dependency confirm triggered as a
        // side effect of enabling a plugin or connecting to a host).
        // design-system-phase-5b task 4 migrated the last `.ssh-overlay`
        // producer (dialog-service.js's confirmPluginPermissions) onto
        // tl-dialog, so every dialog now shows up in tlDialog.count() —
        // no more need to also check for a `.ssh-overlay` node.
        const tlDialogOpen = !!(global.tlDialog && typeof global.tlDialog.count === 'function' && global.tlDialog.count() > 0);
        if (tlDialogOpen) return;
        const pane = getCurrentPane();
        if (pane && pane.term) pane.term.focus();
      }, 80);
    }

    async function openCommandPalette() {
      if (commandPalette) return;
      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
        console.warn('command-palette: tlDialog unavailable, cannot open palette');
        return;
      }

      const body = document.createElement('div');
      body.className = 'tl-palette';
      const input = document.createElement('input');
      input.className = 'tl-palette__input';
      input.type = 'text';
      input.placeholder = 'Type to search commands... (press 1-5 to run)';
      input.spellcheck = false;
      const listEl = document.createElement('div');
      listEl.className = 'tl-palette__results tl-scroll';
      listEl.innerHTML = '<div class="tl-palette__empty">Loading commands…</div>';
      body.appendChild(input);
      body.appendChild(listEl);

      const state = {
        dialogHandle: null,
        inputEl: input,
        listEl,
        allCommands: [],
        filtered: [],
        selectedIndex: 0,
        keyboardMode: false,
        onKeyDown: null,
        keyHandlerUnregister: null,
        pendingRefocus: true,
      };
      commandPalette = state;

      state.onKeyDown = (event) => {
        if (!commandPalette || commandPalette !== state) return false;
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
      // Registered above tl-dialog's own Escape handler (priority 225 — see
      // registerEscape() in app/ui/tl-dialog.js) so THIS handler's Escape
      // branch — which runs the palette's own teardown via
      // closeCommandPalette() — wins and tl-dialog's generic handler for
      // this same dialog instance never fires. Also owns ArrowUp/ArrowDown/
      // Enter/digit-quick-pick, none of which tl-dialog knows about.
      const keyboardRouter = global.termlabKeyboardRouter;
      if (keyboardRouter && typeof keyboardRouter.register === 'function') {
        state.keyHandlerUnregister = keyboardRouter.register({
          name: 'command-palette',
          priority: 260,
          isActive: () => !!(commandPalette === state && state.dialogHandle && state.dialogHandle.el && state.dialogHandle.el.isConnected),
          onKeyDown: (event) => state.onKeyDown(event) === true,
        });
      } else {
        console.warn('command-palette: keyboard router unavailable, palette keyboard navigation disabled');
      }

      listEl.addEventListener('mousemove', () => {
        if (commandPalette !== state) return;
        state.keyboardMode = false;
      });
      input.addEventListener('input', () => {
        if (commandPalette !== state) return;
        state.keyboardMode = false;
        // orderResultsByGroup() applied here, once — `state.filtered` is
        // the single ordering both renderPaletteResults() and every
        // execution path (digit/click/Enter below) share; see that
        // function's comment for why there must not be a second one.
        state.filtered = orderResultsByGroup(filterPaletteCommands(state.allCommands, input.value));
        state.selectedIndex = 0;
        renderPaletteResults();
      });

      state.dialogHandle = global.tlDialog.open({
        top: true,
        size: 'lg',
        ariaLabel: 'Search Everywhere',
        body,
        onOpen: () => { setTimeout(() => input.focus(), 0); },
        // Fires on every close route (our own Escape handling above, a
        // backdrop-mousedown dismissal tl-dialog handles internally and
        // never routes through closeCommandPalette()) — see
        // teardownPaletteState()'s comment.
        onClose: () => teardownPaletteState(state, state.pendingRefocus !== false),
      });

      try {
        state.allCommands = await getPaletteCommands();
        state.filtered = [];
        state.selectedIndex = 0;
        renderPaletteResults();
      } catch (event) {
        listEl.innerHTML = `<div class="tl-palette__empty">Failed to load commands: ${esc(String(event))}</div>`;
      }
    }

    installInvalidationHooks();
    global.__termlabInvalidateCommandPaletteCache = invalidateCommandCache;

    return {
      isOpen: () => Boolean(commandPalette),
      open: openCommandPalette,
      close: closeCommandPalette,
      invalidateCache: (reason) => invalidateCommandCache(reason),
    };
  }

  global.termlabCommandPaletteRuntime = {
    create,
  };
})(window);
