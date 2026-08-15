(function initTermLabEventWiringRuntime(global) {
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
      if (global.termlabContextMenuRuntime && global.termlabContextMenuRuntime.init) {
        global.termlabContextMenuRuntime.init({
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
      if (global.termlabWindowEventsRuntime && global.termlabWindowEventsRuntime.create) {
        const windowEventsRuntime = global.termlabWindowEventsRuntime.create({
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

      const dialogRuntime = global.termlabDialogRuntime && global.termlabDialogRuntime.create
        ? global.termlabDialogRuntime.create({
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

      const menuActionsRuntime = global.termlabMenuActions && global.termlabMenuActions.create
        ? global.termlabMenuActions.create({
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
      if (global.termlabShortcutRuntime && global.termlabShortcutRuntime.create) {
        const shortcutRuntime = global.termlabShortcutRuntime.create({
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

      if (global.termlabConfigRuntime && global.termlabConfigRuntime.create) {
        const configRuntime = global.termlabConfigRuntime.create({
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

      return {
        handleMenuAction,
        showUpdateAvailableToast,
      };
    }

    return {
      init,
    };
  }

  global.termlabEventWiringRuntime = {
    create,
  };
})(window);
