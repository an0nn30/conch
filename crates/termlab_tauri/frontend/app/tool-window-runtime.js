(function initTermLabToolWindowRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listen = deps.listen;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    // Tauri v2 drag-drop events only arrive through onDragDropEvent — plain
    // window listens never fire for them; passed to the files panel for OS
    // file drops (see app/features/files/native-drop.js's dispatcher).
    const onDragDropEvent = deps.currentWindow
      && typeof deps.currentWindow.onDragDropEvent === 'function'
      ? (handler) => deps.currentWindow.onDragDropEvent(handler)
      : null;
    const layoutService = deps.layoutService
      || (global.termlabLayoutService && typeof global.termlabLayoutService.create === 'function'
        ? global.termlabLayoutService.create({ invoke })
        : null);
    const debouncedFitAndResize = deps.debouncedFitAndResize;
    const getCurrentTab = deps.getCurrentTab;
    const getCurrentPane = deps.getCurrentPane;
    const createSshTab = deps.createSshTab;
    const activateTab = deps.activateTab;
    const openPluginDockedViewFromRequest = deps.openPluginDockedViewFromRequest;
    const setFocusedPane = deps.setFocusedPane;
    const closePane = deps.closePane;
    const getPluginViewPaneById = deps.getPluginViewPaneById;
    const registeredPluginToolWindows = new Set();
    let resizeDragDepth = 0;
    let transferRuntimeStartup = null;

    function ensureTransferRuntimeStarted() {
      if (transferRuntimeStartup) return transferRuntimeStartup;
      if (!global.termlabTransferRuntime
          || typeof global.termlabTransferRuntime.ensureStarted !== 'function') {
        return Promise.resolve();
      }
      transferRuntimeStartup = Promise.resolve(global.termlabTransferRuntime.ensureStarted({
        invoke,
        listen: listenOnCurrentWindow,
        toast: global.toast,
      }));
      return transferRuntimeStartup;
    }

    function refreshShortcutFallbacks() {
      if (typeof global.__termlabRefreshKeyboardShortcutFallbacks === 'function') {
        global.__termlabRefreshKeyboardShortcutFallbacks().catch(() => {});
      }
    }

    // ---- panel-host-action routing (the reverse bridge, host -> parent) ----
    //
    // The mirror of dispatchPanelHostEvent in app/panel-host-runtime.js: an
    // `{event, payload}` message off `panel_host_action`
    // (crates/termlab_tauri/src/panel_host.rs), checked against
    // HOST_ACTION_EVENTS (app/core/panel-host-bridge.js) — the single source
    // of truth both this table and the host's publishAction validation read
    // from. An event not in that list is a version-skewed host, or simply
    // nothing wired to it yet: logged and dropped, never thrown, exactly
    // like the forward direction's rule.
    //
    // `open-in-editor` replays the SAME calls files-panel.js's own
    // openInEditor makes when it has a real editor in-window
    // (openLocalFile(path) / openRemoteFile({paneId, remotePath, hostLabel,
    // size})) — read straight off `global.termlabEditorService`, the same
    // global every other in-window caller (vim-mode's :w, the ⌘O chooser)
    // uses, since this parent IS a composed main window and always has one.
    function routePanelHostAction(message) {
      if (!message) return;
      const bridge = global.termlabPanelHostBridge;
      const knownEvents = bridge && Array.isArray(bridge.HOST_ACTION_EVENTS) ? bridge.HOST_ACTION_EVENTS : [];
      if (!knownEvents.includes(message.event)) {
        console.warn('panel host: ignoring unlisted panel-host-action', message.event);
        return;
      }
      const editor = global.termlabEditorService;
      if (!editor) return;
      const payload = message.payload || {};
      if (payload.kind === 'local') {
        editor.openLocalFile(payload.path);
        return;
      }
      if (payload.kind === 'remote') {
        editor.openRemoteFile({
          paneId: payload.paneId,
          remotePath: payload.remotePath,
          hostLabel: payload.hostLabel,
          size: payload.size,
        });
      }
    }

    function beginResizeDrag() {
      resizeDragDepth += 1;
      document.body.classList.add('panel-resize-dragging');
    }

    function endResizeDrag() {
      resizeDragDepth = Math.max(0, resizeDragDepth - 1);
      if (resizeDragDepth === 0) {
        document.body.classList.remove('panel-resize-dragging');
      }
    }

    // The five built-in tool windows, in registration order (the order is
    // load-bearing — see the comment on 'notifications'). Shared verbatim
    // between the main window's init() and a panel host's registrationsOnly
    // boot: a host mounts a panel through the SAME renderFn the docked panel
    // would have used, so there is exactly one definition of each.
    function registerBuiltInToolWindows() {
      global.toolWindowManager.register('file-explorer', {
        title: 'SFTP',
        icon: 'sftp',
        type: 'built-in',
        defaultZone: 'bottom',
        companions: [{ id: 'transfer-center', position: 'bottom' }],
        renderFn: (container) => {
          const panelEl = document.createElement('div');
          panelEl.id = 'files-panel';
          container.appendChild(panelEl);
          if (global.filesPanel) {
            global.filesPanel.init({
              invoke,
              listen: listenOnCurrentWindow,
              onDragDropEvent,
              panelEl,
              panelWrapEl: document.getElementById('left-sidebar'),
              resizeHandleEl: null,
              layoutService,
              fitActiveTab: debouncedFitAndResize,
              getActiveTab: () => getCurrentTab(),
            });
          }
        },
      });

      // Registered after SFTP so file-explorer remains the first/default
      // active window in the bottom zone for existing layouts.
      global.toolWindowManager.register('transfer-center', {
        title: 'Transfers',
        icon: null,
        type: 'built-in',
        defaultZone: 'bottom',
        renderFn: (container) => {
          const panelEl = document.createElement('div');
          panelEl.id = 'transfer-center-panel';
          container.appendChild(panelEl);
          if (global.transferCenterPanel) {
            return global.transferCenterPanel.init({
              panelEl,
              invoke,
              listen: listenOnCurrentWindow,
            });
          }
          return undefined;
        },
      });

      global.toolWindowManager.register('ssh-sessions', {
        title: 'Hosts',
        icon: 'web',
        type: 'built-in',
        defaultZone: 'right-top',
        renderFn: (container) => {
          const panelEl = document.createElement('div');
          panelEl.id = 'ssh-panel';
          container.appendChild(panelEl);
          if (global.sshPanel) {
            global.sshPanel.init({
              invoke,
              listen: listenOnCurrentWindow,
              createSshTab,
              panelEl,
              panelWrapEl: document.getElementById('right-sidebar'),
              resizeHandleEl: null,
              layoutService,
              fitActiveTab: debouncedFitAndResize,
              refocusTerminal: () => {
                const pane = getCurrentPane();
                if (pane && pane.term) pane.term.focus();
              },
            });
          }
        },
      });

      global.toolWindowManager.register('tunnels', {
        title: 'Tunnels',
        icon: null, // no vendored plug-like icon yet; label suffices — Phase 2 known gap
        type: 'built-in',
        defaultZone: 'right-bottom',
        renderFn: (container) => {
          const panelEl = document.createElement('div');
          panelEl.id = 'tunnels-panel';
          container.appendChild(panelEl);
          if (global.tunnelsPanel) {
            global.tunnelsPanel.init({
              invoke,
              listen: listenOnCurrentWindow,
              panelEl,
            });
          }
        },
      });

      // Registered after 'tunnels' so tunnels stays the right-bottom zone's
      // auto-activated window on first boot; notifications starts inactive
      // with a strip button (per tool-window-manager.js's first-registrant-
      // activates-the-zone rule).
      global.toolWindowManager.register('notifications', {
        title: 'Notifications',
        icon: 'notifications',
        type: 'built-in',
        defaultZone: 'right-bottom',
        renderFn: (container) => {
          const panelEl = document.createElement('div');
          panelEl.id = 'notifications-panel';
          container.appendChild(panelEl);
          if (global.notificationsPanel) {
            global.notificationsPanel.init({ panelEl });
          }
        },
      });
    }

    // Plugin tool windows: the live subscriptions plus the replay of whatever
    // is already loaded. Split out of init() for the same reason the built-ins
    // are — a panel host needs the registrations without any of the docked
    // wiring around them. Assumes `pluginWidgets.init` has already run (init()
    // calls it with the terminal callbacks, a host without them).
    //
    // `opts.chrome === false` (what a panel host passes) suppresses the app
    // titlebar refreshes below. A host has no titlebar — `titlebar.init` runs
    // only from event-wiring-runtime.js, which is downstream of the host
    // branch, and it inserts into `#app`, which a host hides — so refreshing
    // one is not merely wasted work: `refresh()` on an UNINITIALIZED titlebar
    // skips rendering (no `menuAreaEl`) but still runs `registerAccelerators`,
    // publishing ~18 app-menu bindings (Cmd/Ctrl+W, +T, +N, +O, +comma, F2,
    // +D …) into the keyboard router at priority 115 with a null
    // `menuActionHandler`. They do nothing and the router still
    // preventDefaults on a match, so every one of those combos becomes a DEAD
    // key. A main window hides this because shortcut-runtime registers at
    // priority 120 and outranks them; a host has no shortcut-runtime handler
    // at all, so the dead table would win outright — squarely against this
    // window's scoped-keys-only constraint.
    //
    // Suppressed at the call site rather than by guarding `titlebar.refresh`
    // itself on initialization: that guard would ALSO stop the same dead
    // (but masked) table being registered in every macOS main window, which
    // is a change to shared chrome behaviour with no bearing on panel hosts.
    // Worth doing — as its own change, with its own blast-radius review.
    function initPluginToolWindows(opts) {
      if (!global.pluginWidgets) return;
      const withChrome = !opts || opts.chrome !== false;
      const refreshTitlebar = () => {
        if (!withChrome) return;
        if (global.titlebar && typeof global.titlebar.refresh === 'function') {
          global.titlebar.refresh().catch(() => {});
        }
      };

      const registerPluginToolWindow = async (panelInfo) => {
        const { handle, plugin, name, location } = panelInfo || {};
        refreshTitlebar();
        const zoneMap = { left: 'left-top', right: 'right-top', bottom: 'bottom' };
        const defaultZone = zoneMap[location] || 'right-bottom';
        const twmId = 'plugin:' + plugin;
        if (global.toolWindowManager && plugin && !registeredPluginToolWindows.has(twmId)) {
          registeredPluginToolWindows.add(twmId);
          global.toolWindowManager.register(twmId, {
            title: name || plugin,
            type: 'plugin',
            defaultZone,
            renderFn: async (container) => {
              const inner = document.createElement('div');
              inner.className = 'plugin-panel-content';
              inner.dataset.pluginHandle = handle;
              inner.dataset.pluginName = plugin;
              container.appendChild(inner);
              try {
                const result = await invoke('request_plugin_render', { pluginName: plugin });
                if (result) global.pluginWidgets.renderWidgets(inner, result, plugin);
              } catch (error) {
                console.error('Initial plugin render failed:', error);
              }
            },
          });
          refreshShortcutFallbacks();
        }
      };

      listen('plugin-panel-registered', async (event) => {
        await registerPluginToolWindow(event.payload);
      });

      listen('plugin-panels-removed', (event) => {
        refreshTitlebar();
        const { plugin, handles } = event.payload;
        if (global.toolWindowManager) {
          registeredPluginToolWindows.delete('plugin:' + plugin);
          global.toolWindowManager.unregister('plugin:' + plugin);
          refreshShortcutFallbacks();
        }
        for (const handle of handles) {
          const container = document.querySelector(`[data-plugin-handle="${handle}"]`);
          if (container) container.remove();
        }
      });

      return invoke('get_plugin_panels').then(async (panels) => {
        if (!Array.isArray(panels)) return;
        for (const panel of panels) {
          await registerPluginToolWindow({
            handle: panel.handle,
            plugin: panel.plugin_name,
            name: panel.panel_name,
            location: panel.location,
          });
        }
      }).catch(() => {});
    }

    // The panel-host boot's entry point: REGISTRATIONS ONLY — the four
    // built-ins, the plugin subscriptions, and the plugin replay, and nothing
    // else. No `toolWindowManager.init()`, so the manager never binds a zone,
    // sidebar, strip or bottom-zone element and every zone/strip update inside
    // register() is an inherent no-op rather than a flag someone has to
    // remember to thread through. No layout read-back or save wiring either
    // (a host owns no layout), no panel-host event subscriptions (those are
    // the PARENT's), and none of init()'s neighbouring inits — vault, keygen,
    // tunnel manager, settings — which belong to a terminal window.
    //
    // `registrationsOnly` is required rather than assumed: this is the only
    // mode registerAll() has today, and naming it at the call site keeps a
    // future "register into a live layout" caller from silently getting the
    // stripped-down one.
    async function registerAll(opts) {
      if (!opts || opts.registrationsOnly !== true) {
        throw new Error('registerAll requires { registrationsOnly: true }');
      }
      if (!global.toolWindowManager) return;
      await ensureTransferRuntimeStarted();
      registerBuiltInToolWindows();
      refreshShortcutFallbacks();
      // `chrome: false` — a host has no app titlebar to refresh, and asking
      // for one would install a dead accelerator table. See the long comment
      // on initPluginToolWindows.
      await initPluginToolWindows({ chrome: false });
    }

    async function init() {
      await ensureTransferRuntimeStarted();

      const bottomZoneWrapEl = document.getElementById('bottom-zone-wrap');
      const bottomZoneResizeEl = document.getElementById('bottom-zone-resize');
      let initialLayoutData = null;

      // A collapsed sidebar measures ~0-1px wide. Persisting that would wipe
      // the user's real width — and a side with no tool windows (as left is
      // once SFTP moves to the bottom zone) collapses on every launch, so the
      // damage compounds. Remember the last width that looked like a real
      // sidebar and save that instead; the restore guard also ignores <= 100.
      const MIN_REAL_SIDEBAR_WIDTH = 100;
      const MIN_REAL_BOTTOM_HEIGHT = 60;
      const lastRealWidths = { left: 0, right: 0 };
      let lastRealBottomHeight = 0;

      function rememberRealWidth(side, value) {
        if (value > MIN_REAL_SIDEBAR_WIDTH) lastRealWidths[side] = value;
        return lastRealWidths[side] > MIN_REAL_SIDEBAR_WIDTH ? lastRealWidths[side] : value;
      }

      // Same trap as the sidebar widths: a hidden or zen-collapsed bottom zone
      // measures 0, and persisting that wipes the user's height for good.
      function rememberRealBottomHeight(value) {
        if (value > MIN_REAL_BOTTOM_HEIGHT) lastRealBottomHeight = value;
        return lastRealBottomHeight > MIN_REAL_BOTTOM_HEIGHT ? lastRealBottomHeight : value;
      }

      function saveLayoutNow() {
        const twm = global.toolWindowManager;
        if (!twm) return;
        const measured = twm.getSidebarWidths();
        const widths = {
          left: rememberRealWidth('left', measured.left),
          right: rememberRealWidth('right', measured.right),
        };
        const appRoot = document.getElementById('app');
        const zenActive = !!(appRoot && appRoot.classList.contains('zen-mode'));
        const zenRestore = global.__termlabZenRestoreState || {};
        const leftVisible = zenActive && typeof zenRestore.leftVisible === 'boolean'
          ? !!zenRestore.leftVisible
          : (typeof twm.isPanelOpen === 'function' ? twm.isPanelOpen('left') : twm.isPanelVisible('left'));
        const rightVisible = zenActive && typeof zenRestore.rightVisible === 'boolean'
          ? !!zenRestore.rightVisible
          : (typeof twm.isPanelOpen === 'function' ? twm.isPanelOpen('right') : twm.isPanelVisible('right'));
        const bottomVisible = zenActive && typeof zenRestore.bottomVisible === 'boolean'
          ? !!zenRestore.bottomVisible
          : twm.isPanelVisible('bottom');
        const payload = {
          ssh_panel_width: widths.right,
          ssh_panel_visible: rightVisible,
          files_panel_width: widths.left,
          files_panel_visible: leftVisible,
          bottom_panel_visible: bottomVisible,
          bottom_panel_height: rememberRealBottomHeight(bottomZoneWrapEl ? bottomZoneWrapEl.offsetHeight : 0),
          // A window whose zen came from the "new windows open in zen"
          // default persists the state it INHERITED, not the live one.
          // Otherwise a throwaway window would write zen_mode = true into the
          // shared layout and the main window would open in zen next launch —
          // the flag is cleared the moment the user toggles zen by hand, so a
          // deliberate choice still persists.
          zen_mode: window.__termlabZenIsSessionDefault === true
            ? window.__termlabInitialZenMode === true
            : !!(appRoot && appRoot.classList.contains('zen-mode')),
          tool_window_zones: twm.getZoneAssignments(),
          active_tool_windows: typeof twm.getActiveZoneAssignments === 'function'
            ? twm.getActiveZoneAssignments()
            : {},
          // Which tool windows are popped out into their own OS window. Keyed
          // by tool-window id, exactly like tool_window_zones — a view mode
          // belongs to the tool window, not to the main window showing it.
          tool_window_view_modes: typeof twm.getViewModes === 'function'
            ? twm.getViewModes()
            : {},
          split_ratios: twm.getSplitRatios(),
        };
        if (layoutService && typeof layoutService.saveLayout === 'function') {
          layoutService.saveLayout(payload);
        } else {
          invoke('save_window_layout', { layout: payload }).catch(() => {});
        }
      }

      let windowResaveSaveTimer = null;
      function debouncedSaveLayout() {
        if (windowResaveSaveTimer) clearTimeout(windowResaveSaveTimer);
        windowResaveSaveTimer = setTimeout(() => {
          saveLayoutNow();
        }, 150);
      }

      if (global.toolWindowManager) {
        global.toolWindowManager.init({
          fitActiveTab: debouncedFitAndResize,
          saveLayout: saveLayoutNow,
          // The manager's only backend calls: the panel-host commands behind
          // the View Mode trait (open/focus/hide a popped-out tool window).
          invoke,
        });

        // Rust emits these to the PARENT window (emit_to(parent_label)), so
        // they must be listened for on this window specifically, not app-wide.
        listenOnCurrentWindow('panel-host-shown', (event) => {
          const id = event && event.payload ? event.payload.toolWindowId : null;
          if (id) global.toolWindowManager.notifyHostShown(id);
        });
        listenOnCurrentWindow('panel-host-hidden', (event) => {
          const id = event && event.payload ? event.payload.toolWindowId : null;
          if (id) global.toolWindowManager.notifyHostHidden(id);
        });
        // The only one of the four carrying a generation token: `reqId`
        // identifies WHICH host docked, so an echo from a host that has
        // already been replaced can be told apart from a current dock-back.
        listenOnCurrentWindow('panel-host-docked', (event) => {
          const payload = (event && event.payload) || null;
          const id = payload ? payload.toolWindowId : null;
          if (id) global.toolWindowManager.notifyHostDocked(id, payload.reqId);
        });
        // A host that self-aborted (its boot found no registration for the id)
        // is not coming back with a panel — the manager resets the trait
        // rather than waiting for a remount that will never arrive.
        listenOnCurrentWindow('panel-host-aborted', (event) => {
          const id = event && event.payload ? event.payload.toolWindowId : null;
          if (id) global.toolWindowManager.notifyHostAborted(id);
        });
        // Rust emits this to the PARENT too (emit_to(entry.parent_label) in
        // panel_host_action) — a host asking this window to do something it
        // has no local means to do itself. See routePanelHostAction above.
        listenOnCurrentWindow('panel-host-action', (event) => {
          routePanelHostAction(event && event.payload ? event.payload : null);
        });

        try {
          initialLayoutData = layoutService && typeof layoutService.getSavedLayout === 'function'
            ? await layoutService.getSavedLayout()
            : await invoke('get_saved_layout');
          if (initialLayoutData.files_panel_width > 100) {
            rememberRealWidth('left', initialLayoutData.files_panel_width);
            global.toolWindowManager.setSidebarWidth('left', initialLayoutData.files_panel_width);
          }
          if (initialLayoutData.ssh_panel_width > 100) {
            rememberRealWidth('right', initialLayoutData.ssh_panel_width);
            global.toolWindowManager.setSidebarWidth('right', initialLayoutData.ssh_panel_width);
          }
          if (initialLayoutData.tool_window_zones && Object.keys(initialLayoutData.tool_window_zones).length > 0) {
            global.toolWindowManager.setPersistedZones(initialLayoutData.tool_window_zones);

            // Migration: saved layouts from before the Tunnels tool window
            // existed have no 'tunnels' entry in tool_window_zones. Such a
            // layout's active_tool_windows map predates the window too, so
            // hasPersistedActiveForSide('right') would otherwise suppress
            // auto-activating Tunnels on first boot after upgrading (the
            // right-bottom zone would just sit on whatever it last knew
            // about, or nothing). Only step in when right-bottom truly has
            // no recorded active window — if it does, this is either an
            // up-to-date layout (leave it alone) or the user deliberately
            // hid Tunnels after it existed (also leave it alone).
            if (!Object.prototype.hasOwnProperty.call(initialLayoutData.tool_window_zones, 'tunnels')) {
              if (!initialLayoutData.active_tool_windows || typeof initialLayoutData.active_tool_windows !== 'object') {
                initialLayoutData.active_tool_windows = {};
              }
              if (!Object.prototype.hasOwnProperty.call(initialLayoutData.active_tool_windows, 'right-bottom')) {
                initialLayoutData.active_tool_windows['right-bottom'] = 'tunnels';
              }
            }

            // Migration: SFTP (still id 'file-explorer') moved out of a side
            // zone into the bottom zone, which did not exist before. A layout
            // that records no window in 'bottom' predates the move, so its
            // pinned side zone would override the new default forever. Once
            // any window is recorded in 'bottom' the layout knows about the
            // zone and the user's own arrangement is left alone.
            //
            // This must run at most once. Without a durable marker, a user
            // who deliberately moves SFTP back out of 'bottom' (and leaves
            // nothing else there) would get force-migrated back on every
            // restart, defeating their own choice. A localStorage flag makes
            // the one-time-ness durable across restarts without a persisted-
            // layout schema change; the knowsBottomZone check below remains
            // as a second guard so a cleared flag still won't re-migrate a
            // layout that already knows about the bottom zone.
            const SFTP_BOTTOM_MIGRATION_KEY = 'termlab.migration.sftpBottomZone';
            let sftpBottomMigrationAlreadyRan = false;
            try {
              sftpBottomMigrationAlreadyRan = global.localStorage.getItem(SFTP_BOTTOM_MIGRATION_KEY) === '1';
            } catch (_) {}

            const savedZones = initialLayoutData.tool_window_zones;
            const knowsBottomZone = Object.keys(savedZones)
              .some((id) => String(savedZones[id]).startsWith('bottom'));
            if (!sftpBottomMigrationAlreadyRan && !knowsBottomZone && savedZones['file-explorer']) {
              const previousZone = savedZones['file-explorer'];
              savedZones['file-explorer'] = 'bottom-left';
              global.toolWindowManager.setPersistedZones(savedZones);
              if (!initialLayoutData.active_tool_windows || typeof initialLayoutData.active_tool_windows !== 'object') {
                initialLayoutData.active_tool_windows = {};
              }
              if (initialLayoutData.active_tool_windows[previousZone] === 'file-explorer') {
                delete initialLayoutData.active_tool_windows[previousZone];
              }
              initialLayoutData.active_tool_windows['bottom-left'] = 'file-explorer';
              // bottom_panel_visible used to describe the notifications bar,
              // which most layouts recorded as hidden. Honouring that here
              // would silently swallow the panel we just moved, so reveal the
              // zone once, on this migration only.
              initialLayoutData.bottom_panel_visible = true;
              try {
                global.localStorage.setItem(SFTP_BOTTOM_MIGRATION_KEY, '1');
              } catch (_) {}
            }
          }
          if (initialLayoutData.active_tool_windows && Object.keys(initialLayoutData.active_tool_windows).length > 0) {
            global.toolWindowManager.setPersistedActiveZoneWindows(initialLayoutData.active_tool_windows);
          }
          // Seeded before any register() below: a window the user had popped
          // out must never mount into its zone on the way past.
          if (initialLayoutData.tool_window_view_modes
            && typeof global.toolWindowManager.setPersistedViewModes === 'function') {
            global.toolWindowManager.setPersistedViewModes(initialLayoutData.tool_window_view_modes);
          }
          if (typeof global.toolWindowManager.setPersistedPanelVisibility === 'function') {
            global.toolWindowManager.setPersistedPanelVisibility({
              left: initialLayoutData.files_panel_visible !== false,
              right: initialLayoutData.ssh_panel_visible !== false,
              bottom: initialLayoutData.bottom_panel_visible !== false,
            });
          }
          if (initialLayoutData.left_split_ratio > 0 && initialLayoutData.left_split_ratio < 1) {
            global.toolWindowManager.setSplitRatio('left', initialLayoutData.left_split_ratio);
          }
          if (initialLayoutData.right_split_ratio > 0 && initialLayoutData.right_split_ratio < 1) {
            global.toolWindowManager.setSplitRatio('right', initialLayoutData.right_split_ratio);
          }
          if (initialLayoutData.bottom_split_ratio > 0 && initialLayoutData.bottom_split_ratio < 1) {
            global.toolWindowManager.setSplitRatio('bottom', initialLayoutData.bottom_split_ratio);
          }
        } catch (_) {}

        if (initialLayoutData) {
          global.toolWindowManager.setPanelVisibility('left', initialLayoutData.files_panel_visible !== false, { save: false });
          global.toolWindowManager.setPanelVisibility('right', initialLayoutData.ssh_panel_visible !== false, { save: false });
          global.toolWindowManager.setPanelVisibility('bottom', initialLayoutData.bottom_panel_visible !== false, { save: false });
          if (initialLayoutData.bottom_panel_height > 0 && bottomZoneWrapEl) {
            rememberRealBottomHeight(initialLayoutData.bottom_panel_height);
            bottomZoneWrapEl.style.height = initialLayoutData.bottom_panel_height + 'px';
          }
        }

        registerBuiltInToolWindows();

        if (initialLayoutData && initialLayoutData.zen_mode === true) {
          global.toolWindowManager.setPanelVisibility('left', false, { save: false });
          global.toolWindowManager.setPanelVisibility('right', false, { save: false });
          global.toolWindowManager.setPanelVisibility('bottom', false, { save: false });
        }
        refreshShortcutFallbacks();

        // Every built-in is registered by now, so the restored view modes are
        // known: re-open the hosts that were on screen when the layout was
        // saved. Plugin tool windows register later (asynchronously, below) —
        // the manager summons those as they arrive instead.
        if (typeof global.toolWindowManager.summonPendingWindowHosts === 'function') {
          global.toolWindowManager.summonPendingWindowHosts();
        }

        // Main window only — a panel host projects panels but must never
        // steal a zone tab in response to queue events; the parent window
        // does the summoning. The runtime finished hydrating above, so the
        // subscription baseline is the restored queue, not an empty store.
        if (global.termlabTransferAutoOpen && global.termlabTransferRuntime) {
          global.termlabTransferAutoOpen.init({
            runtime: global.termlabTransferRuntime,
            toolWindowManager: global.toolWindowManager,
          });
        }
      }

      global.addEventListener('resize', debouncedSaveLayout);
      global.addEventListener('beforeunload', saveLayoutNow);
      global.addEventListener('pagehide', saveLayoutNow);
      document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'hidden') saveLayoutNow();
      });

      {
        let dragging = false;
        let startY = 0;
        let startHeight = 0;
        bottomZoneResizeEl.addEventListener('dragstart', (event) => event.preventDefault());
        bottomZoneResizeEl.style.touchAction = 'none';
        bottomZoneResizeEl.addEventListener('pointerdown', (event) => {
          event.preventDefault();
          bottomZoneResizeEl.setPointerCapture(event.pointerId);
          dragging = true;
          startY = event.clientY;
          startHeight = bottomZoneWrapEl.offsetHeight;
          bottomZoneResizeEl.classList.add('dragging');
          beginResizeDrag();
          document.body.style.cursor = 'row-resize';
          document.body.style.userSelect = 'none';
        });
        bottomZoneResizeEl.addEventListener('pointermove', (event) => {
          if (!dragging) return;
          const delta = startY - event.clientY;
          const newHeight = Math.max(80, Math.min(window.innerHeight * 0.6, startHeight + delta));
          bottomZoneWrapEl.style.height = newHeight + 'px';
          debouncedFitAndResize();
        });
        bottomZoneResizeEl.addEventListener('pointerup', (event) => {
          if (!dragging) return;
          bottomZoneResizeEl.releasePointerCapture(event.pointerId);
          dragging = false;
          bottomZoneResizeEl.classList.remove('dragging');
          endResizeDrag();
          document.body.style.cursor = '';
          document.body.style.userSelect = '';
          saveLayoutNow();
        });
        bottomZoneResizeEl.addEventListener('pointercancel', () => {
          if (!dragging) return;
          dragging = false;
          bottomZoneResizeEl.classList.remove('dragging');
          endResizeDrag();
          document.body.style.cursor = '';
          document.body.style.userSelect = '';
          saveLayoutNow();
        });
      }

      if (global.vault) {
        global.vault.init({ invoke, listen: listenOnCurrentWindow });
      }

      if (global.keygen) {
        global.keygen.init({ invoke });
        listenOnCurrentWindow('keygen-open', () => global.keygen.showKeygenDialog());
      }

      if (global.tunnelManager) {
        global.tunnelManager.init({
          invoke,
          listen: listenOnCurrentWindow,
          getServerData: () => (
            global.sshPanel ? global.sshPanel.getServerData() : { folders: [], ungrouped: [], ssh_config: [] }
          ),
        });
      }

      if (global.settings) {
        global.settings.init({ invoke, listen: listenOnCurrentWindow });
      }

      listenOnCurrentWindow('settings-restart-required', () => {
        if (global.toast) global.toast.warn('Restart Required', 'Some changes require a restart to take effect.');
      });

      listenOnCurrentWindow('plugin-view-open-requested', (event) => {
        openPluginDockedViewFromRequest(event.payload).catch((error) => {
          console.error('Failed to open plugin docked view:', error);
        });
      });

      listenOnCurrentWindow('plugin-view-focus-requested', (event) => {
        const viewId = event && event.payload ? event.payload.view_id : null;
        const map = getPluginViewPaneById();
        if (!viewId || !map.has(viewId)) return;
        setFocusedPane(map.get(viewId));
      });

      listenOnCurrentWindow('plugin-view-close-requested', (event) => {
        const viewId = event && event.payload ? event.payload.view_id : null;
        const map = getPluginViewPaneById();
        if (!viewId || !map.has(viewId)) return;
        closePane(map.get(viewId));
      });

      listenOnCurrentWindow('plugin-views-removed', (event) => {
        if (global.titlebar && typeof global.titlebar.refresh === 'function') {
          global.titlebar.refresh().catch(() => {});
        }
        const viewIds = (event && event.payload && event.payload.view_ids) || [];
        const map = getPluginViewPaneById();
        for (const viewId of viewIds) {
          if (!map.has(viewId)) continue;
          closePane(map.get(viewId));
        }
      });

      if (global.pluginWidgets) {
        const applyTabTitle = (tab, title) => {
          if (!tab || !tab.button) return;
          const nextTitle = String(title || '').trim();
          if (!nextTitle) return;
          if (tab.button._labelSpan) tab.button._labelSpan.textContent = nextTitle;
          else tab.button.textContent = nextTitle;
          tab.label = nextTitle;
          tab.hasCustomTitle = true;
          tab.pluginRenamed = true;
          tab.button.title = nextTitle;
          // This mutates the tab label directly (bypassing tab-manager's
          // setTabLabel), so the window title needs an explicit resync.
          if (typeof deps.refreshWindowTitle === 'function') deps.refreshWindowTitle();
        };

        global.pluginWidgets.init({
          invoke,
          listen,
          createTab: (options) => deps.createTab(options),
          renameActiveTab: (title) => {
            const tab = deps.getCurrentTab ? deps.getCurrentTab() : null;
            applyTabTitle(tab, title);
          },
          renameTabById: (tabId, title) => {
            const tab = deps.getTabById ? deps.getTabById(tabId) : null;
            applyTabTitle(tab, title);
          },
          focusTabById: (tabId) => {
            const tryActivate = () => {
              const tab = deps.getTabById ? deps.getTabById(tabId) : null;
              if (tab && tab.id != null && typeof activateTab === 'function') {
                activateTab(tab.id);
                return true;
              }
              const asNumber = Number(String(tabId || '').trim());
              if (Number.isFinite(asNumber) && typeof activateTab === 'function') {
                activateTab(asNumber);
                return true;
              }
              return false;
            };

            if (tryActivate()) return;
            // Some plugin flows race with tab-map updates; retry once next tick.
            setTimeout(() => {
              tryActivate();
            }, 0);
          },
          writeToActivePty: (data) => {
            const pane = getCurrentPane();
            if (!pane || !pane.spawned) return;
            const cmd = pane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
            invoke(cmd, { paneId: pane.paneId, data }).catch(() => {});
          },
        });

        initPluginToolWindows();
      }

      return {
        debouncedSaveLayout: saveLayoutNow,
      };
    }

    return {
      init,
      registerAll,
    };
  }

  global.termlabToolWindowRuntime = {
    create,
  };
})(window);
