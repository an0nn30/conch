(function initConchEventWiringRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listen = deps.listen;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const currentWindowLabel = deps.currentWindowLabel;
    const terminalHostEl = deps.terminalHostEl;
    const tabBarEl = deps.tabBarEl;
    const tabs = deps.tabs;
    const panes = deps.panes;
    const getActiveTabId = deps.getActiveTabId;
    const getFocusedPaneId = deps.getFocusedPaneId;
    const getCurrentPane = deps.getCurrentPane;
    const getCurrentTab = deps.getCurrentTab;
    const closeTab = deps.closeTab;
    const createTab = deps.createTab;
    const createTmuxTab = deps.createTmuxTab;
    const closePane = deps.closePane;
    const splitPane = deps.splitPane;
    const renameActiveTab = deps.renameActiveTab;
    const setFocusedPane = deps.setFocusedPane;
    const startTabRename = deps.startTabRename;
    const fitAndResizeTab = deps.fitAndResizeTab;
    const debouncedSaveLayout = deps.debouncedSaveLayout;
    const showStatus = deps.showStatus;
    const isTextInputTarget = deps.isTextInputTarget;
    const writeTextToCurrentPane = deps.writeTextToCurrentPane;
    const pasteIntoCurrentPane = deps.pasteIntoCurrentPane;
    const openCommandPalette = deps.openCommandPalette;
    const closeCommandPalette = deps.closeCommandPalette;
    const isCommandPaletteOpen = deps.isCommandPaletteOpen;
    const refocusActiveTerminal = deps.refocusActiveTerminal;
    const terminalRuntime = deps.terminalRuntime;
    const shortcutDebugEnabled = deps.shortcutDebugEnabled;
    const getZoom = deps.getZoom;
    const setZoom = deps.setZoom;
    const getThemeState = deps.getThemeState;
    const setThemeState = deps.setThemeState;
    const getTermConfigState = deps.getTermConfigState;
    const setTermConfigState = deps.setTermConfigState;
    const fontFallbacks = deps.fontFallbacks;

    async function init() {
      if (global.conchContextMenuRuntime && global.conchContextMenuRuntime.init) {
        global.conchContextMenuRuntime.init({
          terminalHostEl,
          tabBarEl,
          getPanes: () => panes,
          getTabs: () => tabs,
          terminalMouseModeIsActive: (term) => terminalRuntime.terminalMouseModeIsActive(term),
          setFocusedPane: (paneId) => setFocusedPane(paneId),
          splitPane: (direction) => splitPane(direction),
          startTabRenameById: (tabId) => {
            const tab = tabs.get(tabId);
            if (tab) startTabRename(tab);
          },
          closeTab: (tabId) => closeTab(tabId),
        });
      }

      let showUpdateAvailableToast = (_info) => {};
      if (global.conchWindowEventsRuntime && global.conchWindowEventsRuntime.create) {
        const windowEventsRuntime = global.conchWindowEventsRuntime.create({
          invoke,
          listenOnCurrentWindow,
          listen,
          currentWindowLabel,
          getPanes: () => panes,
          closePane: (paneId) => closePane(paneId),
          refreshSshSessions: () => {
            if (global.sshPanel) global.sshPanel.refreshSessions();
          },
          esc: (text) => global.utils.esc(text),
        });
        const runtimeResult = await windowEventsRuntime.init();
        if (runtimeResult && typeof runtimeResult.showUpdateAvailableToast === 'function') {
          showUpdateAvailableToast = runtimeResult.showUpdateAvailableToast;
        }
      }

      const dialogRuntime = global.conchDialogRuntime && global.conchDialogRuntime.create
        ? global.conchDialogRuntime.create({
            invoke,
            esc: (text) => global.utils.esc(text),
            refocusActiveTerminal: () => refocusActiveTerminal(),
            isCommandPaletteOpen: () => isCommandPaletteOpen(),
          })
        : null;
      if (dialogRuntime && typeof dialogRuntime.initOverlayFocusHandlers === 'function') {
        dialogRuntime.initOverlayFocusHandlers();
      }
      const showAboutDialog = () => {
        if (dialogRuntime && typeof dialogRuntime.showAboutDialog === 'function') {
          return dialogRuntime.showAboutDialog();
        }
        return Promise.resolve();
      };

      const menuActionsRuntime = global.conchMenuActions && global.conchMenuActions.create
        ? global.conchMenuActions.create({
            invoke,
            getCurrentPane: () => getCurrentPane(),
            isTextInputTarget: (el) => isTextInputTarget(el),
            createTab: () => createTab(),
            createPlainShellTab: () => createTab({ plainShell: true }),
            showStatus: (message) => showStatus(message),
            pasteIntoCurrentPane: () => pasteIntoCurrentPane(),
            openCommandPalette: () => openCommandPalette(),
            closeCommandPalette: () => closeCommandPalette(),
            isCommandPaletteOpen: () => isCommandPaletteOpen(),
            getActiveTabId: () => getActiveTabId(),
            closeTab: (tabId) => closeTab(tabId),
            debouncedSaveLayout: () => debouncedSaveLayout(),
            getZoom: () => getZoom(),
            setZoom: (value) => setZoom(value),
            splitPane: (direction) => splitPane(direction),
            getFocusedPaneId: () => getFocusedPaneId(),
            closePane: (paneId) => closePane(paneId),
            renameActiveTab: () => renameActiveTab(),
            fitAndResizeCurrentTab: () => fitAndResizeTab(getCurrentTab()),
            showAboutDialog: () => showAboutDialog(),
            showUpdateAvailableToast: (info) => showUpdateAvailableToast(info),
          })
        : null;

      function handleMenuAction(action) {
        if (!menuActionsRuntime || !menuActionsRuntime.handleMenuAction) {
          throw new Error('menuActionsRuntime.handleMenuAction is unavailable');
        }
        menuActionsRuntime.handleMenuAction(action);
      }

      await listenOnCurrentWindow('menu-action', (event) => {
        const payload = event.payload || {};
        const windowLabel = payload.window_label;
        const action = payload.action;
        if (typeof windowLabel !== 'string' || windowLabel !== currentWindowLabel) {
          return;
        }
        handleMenuAction(action);
      });

      if (global._initTitlebarPending && global.titlebar) {
        global.titlebar.init(handleMenuAction);
        delete global._initTitlebarPending;
      }

      let refreshKeyboardShortcutFallbacks = async () => {};
      if (global.conchShortcutRuntime && global.conchShortcutRuntime.create) {
        const shortcutRuntime = global.conchShortcutRuntime.create({
          invoke,
          isMacPlatform: /mac/i.test(navigator.platform || ''),
          isTextInputTarget: (el) => isTextInputTarget(el),
          handleMenuAction: (action) => handleMenuAction(action),
          shouldDebugKeyEvent: (event) => terminalRuntime.shouldDebugKeyEvent(event),
          formatKeyEventForDebug: (event) => terminalRuntime.formatKeyEventForDebug(event),
          shortcutDebugEnabled,
          openCommandPalette: () => openCommandPalette(),
          closeCommandPalette: () => closeCommandPalette(),
          isCommandPaletteOpen: () => isCommandPaletteOpen(),
          getTabIds: () => Array.from(tabs.keys()),
          activateTab: (tabId) => deps.activateTab(tabId),
          getCurrentPane: () => getCurrentPane(),
          writeTextToCurrentPane: (text) => writeTextToCurrentPane(text),
          getActiveTab: () => tabs.get(getActiveTabId()) || null,
          getFocusedPaneId: () => getFocusedPaneId(),
          setFocusedPane: (paneId) => setFocusedPane(paneId),
          findAdjacentPane: (paneId, dir, containerEl) => global.splitPane.findAdjacentPane(paneId, dir, containerEl),
        });
        const shortcutRuntimeResult = await shortcutRuntime.init();
        if (shortcutRuntimeResult && typeof shortcutRuntimeResult.refreshKeyboardShortcutFallbacks === 'function') {
          refreshKeyboardShortcutFallbacks = shortcutRuntimeResult.refreshKeyboardShortcutFallbacks;
        }
      }

      if (global.conchConfigRuntime && global.conchConfigRuntime.create) {
        const configRuntime = global.conchConfigRuntime.create({
          invoke,
          listenOnCurrentWindow,
          refreshKeyboardShortcutFallbacks: () => refreshKeyboardShortcutFallbacks(),
          getPanes: () => panes,
          setTheme: (nextTheme) => setThemeState(nextTheme),
          getFontFallbacks: () => fontFallbacks,
          setTermFontFamily: (value) => setTermConfigState({ fontFamily: value }),
          setTermFontSize: (value) => setTermConfigState({ fontSize: value }),
        });
        configRuntime.init();
      }

      function wireTmuxEvents() {
        if (global.__conchTmuxEventsWired) return;
        global.__conchTmuxEventsWired = true;
        var currentTmuxSession = null;
        var tmuxSyncTimer = null;

        function syncTmuxSession(sessionName) {
          if (!sessionName || !createTmuxTab) return Promise.resolve();
          currentTmuxSession = sessionName;
          console.info('[tmux] syncTmuxSession start', { sessionName: sessionName });
          return invoke('tmux_list_windows', { sessionName: sessionName }).then(function (windows) {
            console.info('[tmux] syncTmuxSession windows', { sessionName: sessionName, windows: windows });
            var windowIds = new Set();
            (windows || []).forEach(function (windowInfo) {
              windowIds.add(Number(windowInfo.id));
              console.info('[tmux] syncTmuxSession createTmuxTab', {
                windowId: Number(windowInfo.id),
                paneCount: Array.isArray(windowInfo.panes) ? windowInfo.panes.length : 0,
                active: !!windowInfo.active,
              });
              createTmuxTab({
                windowId: Number(windowInfo.id),
                name: windowInfo.name,
                panes: Array.isArray(windowInfo.panes) ? windowInfo.panes : [],
                activate: !!windowInfo.active,
              });
            });

            if (global.tmuxIdMap) {
              Array.from(tabs.values()).forEach(function (tab) {
                if (!tab || tab.type !== 'tmux') return;
                if (!windowIds.has(Number(tab.tmuxWindowId))) {
                  console.info('[tmux] syncTmuxSession removing stale tmux tab', {
                    tabId: tab.id,
                    tmuxWindowId: tab.tmuxWindowId,
                  });
                  closeTab(tab.id, { notifyBackend: false, closeWindowWhenLast: false });
                }
              });
            }
            var switchState = window.__conchTmuxSwitchState || null;
            if (switchState && switchState.connectedSession === sessionName) {
              switchState.syncedAt = Date.now();
              switchState.suppressDisconnectsUntil = Date.now() + 1500;
              window.__conchTmuxSwitchState = switchState;
            }
          }).catch(function (error) {
            console.error('[tmux] failed to sync session:', error);
          });
        }

        function scheduleTmuxSync() {
          if (!currentTmuxSession) return;
          clearTimeout(tmuxSyncTimer);
          tmuxSyncTimer = setTimeout(function () {
            syncTmuxSession(currentTmuxSession);
          }, 60);
        }

        function forceSyncSession(sessionName, attempt) {
          if (!sessionName) return;
          var tries = Number(attempt) || 0;
          syncTmuxSession(sessionName).then(function () {
            var hasTmuxTab = Array.from(tabs.values()).some(function (tab) {
              return tab && tab.type === 'tmux';
            });
            if (hasTmuxTab) return;
            if (tries >= 3) return;
            var retryDelays = [120, 300, 700];
            setTimeout(function () {
              forceSyncSession(sessionName, tries + 1);
            }, retryDelays[tries] || 300);
          });
        }

        global.__conchTmuxRequestSyncSoon = function (delayMs) {
          if (!currentTmuxSession) return;
          clearTimeout(tmuxSyncTimer);
          tmuxSyncTimer = setTimeout(function () {
            syncTmuxSession(currentTmuxSession);
          }, typeof delayMs === 'number' ? delayMs : 80);
        };
        global.__conchTmuxForceSyncSession = function (sessionName) {
          forceSyncSession(sessionName, 0);
        };

        listenOnCurrentWindow('tmux-connected', function (event) {
          var payload = event.payload || {};
          var switchState = window.__conchTmuxSwitchState || null;
          console.info('[tmux] connected event', {
            session: payload.session || null,
            switchState: switchState,
          });
          if (switchState) {
            switchState.connectedAt = Date.now();
            switchState.connectedSession = payload.session || null;
            switchState.suppressDisconnectsUntil = Date.now() + 3000;
            window.__conchTmuxSwitchState = switchState;
          }
          var targetSession = payload.session || currentTmuxSession;
          if (targetSession) {
            forceSyncSession(targetSession, 0);
          } else {
            invoke('tmux_get_last_session').then(function (lastSession) {
              if (lastSession) forceSyncSession(lastSession, 0);
            }).catch(function () {});
          }
        });

        listenOnCurrentWindow('tmux-output', function (event) {
          var payload = event.payload || {};
          if (!global.tmuxIdMap) return;
          var frontendPaneId = global.tmuxIdMap.getPaneForTmux(payload.pane_id);
          if (frontendPaneId != null) {
            var pane = panes.get(frontendPaneId);
            if (pane && pane.term) {
              var dataText = typeof payload.data === 'string' ? payload.data : '';
              if (pane._tmuxSnapshotTimer) {
                clearTimeout(pane._tmuxSnapshotTimer);
                pane._tmuxSnapshotTimer = null;
              }
              if (!pane._tmuxLiveActive) {
                console.info('[tmux] pane marked live from tmux-output', {
                  frontendPaneId: frontendPaneId,
                  tmuxPaneId: payload.pane_id,
                });
                var snapshotAgeMs = pane._tmuxSnapshotAppliedAt
                  ? (Date.now() - Number(pane._tmuxSnapshotAppliedAt))
                  : Number.POSITIVE_INFINITY;
                var looksLikeScreenPaint = dataText.indexOf('\n') >= 0 || dataText.indexOf('\x1b') >= 0;
                if (snapshotAgeMs < 1500 && looksLikeScreenPaint) {
                  // Replace freshly hydrated snapshot content with authoritative live stream.
                  pane.term.reset();
                }
              }
              pane._tmuxLiveActive = true;
              pane.term.write(payload.data);

              // When full-screen TUIs (htop/vim/etc) exit, tmux emits alt-screen restore
              // sequences. Force a quick snapshot resync to avoid stale ghost frames.
              if (typeof dataText === 'string' && (
                dataText.indexOf('\x1b[?1049l') !== -1 ||
                dataText.indexOf('\x1b[?47l') !== -1 ||
                dataText.indexOf('\x1b[?1047l') !== -1
              )) {
                pane._tmuxLiveActive = false;
                if (typeof global.__conchTmuxRequestSyncSoon === 'function') {
                  global.__conchTmuxRequestSyncSoon(40);
                }
              }
            }
          }
        });

        listenOnCurrentWindow('tmux-window-add', function (event) {
          scheduleTmuxSync();
        });

        listenOnCurrentWindow('tmux-window-close', function (event) {
          scheduleTmuxSync();
        });

        listenOnCurrentWindow('tmux-window-renamed', function (event) {
          scheduleTmuxSync();
        });

        listenOnCurrentWindow('tmux-layout-change', function () {
          scheduleTmuxSync();
        });

        // `tmux-pane-changed` can fire very frequently (including during typing).
        // Full snapshot sync on each event causes severe redraw/focus churn.
        // Keep structural sync driven by window/layout events instead.
        listenOnCurrentWindow('tmux-pane-changed', function () {});

        listenOnCurrentWindow('tmux-disconnected', function (event) {
          var switchState = window.__conchTmuxSwitchState || null;
          var reason = event.payload && event.payload.reason;
          var now = Date.now();
          var suppressForSwitch = !!(switchState && now <= Number(switchState.suppressDisconnectsUntil || 0));
          console.info('[tmux] disconnected event', {
            reason: reason || null,
            suppressForSwitch: suppressForSwitch,
            switchState: switchState,
          });
          if (suppressForSwitch) {
            return;
          }
          if (!reason) {
            console.info('[tmux] disconnected event ignored; snapshot sync remains active', {
              session: currentTmuxSession,
            });
            return;
          }
          currentTmuxSession = null;
          window.__conchTmuxSwitchState = null;
          if (global.toast) {
            global.toast.warn(reason ? 'Tmux disconnected: ' + reason : 'Tmux session ended');
          }
        });
      }

      return {
        handleMenuAction,
        showUpdateAvailableToast,
        wireTmuxEvents,
      };
    }

    return {
      init,
    };
  }

  global.conchEventWiringRuntime = {
    create,
  };
})(window);
