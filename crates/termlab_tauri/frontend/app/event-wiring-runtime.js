(function initTermLabEventWiringRuntime(global) {
  // Wire the CLI/second-instance "open this path" drain for THIS window (see
  // src/open_path.rs's per-window pending-open queue and the
  // `take_pending_open_paths` command).
  //
  // Extracted from init() and exported so this seam is testable on its own:
  // it is a guard clause over two globals composed by completely different
  // modules, and a rename on either side would silently turn `termlab
  // notes.md` into a no-op with every routing test still passing.
  //
  // Returns null — meaning "nothing to drain here" — when either global is
  // absent. That is the normal state for a panel-host window: main-runtime.js
  // branches to panelHostRuntime.boot() before ever constructing this
  // runtime, so a popped-out tool window (no editor, no tabs) never attempts
  // to drain a queue it was never given anything on.
  //
  // `g` is passed in rather than closed over so the globals can be swapped
  // per test case.
  function wirePendingOpenDrain(g, deps) {
    const routing = g.termlabOpenPathRouting;
    const editor = g.termlabEditorService;
    if (!routing || typeof routing.create !== 'function') return null;
    if (!editor || typeof editor.openLocalFile !== 'function') return null;
    return routing.create({
      invoke: deps.invoke,
      openLocalFile: (filePath) => g.termlabEditorService.openLocalFile(filePath),
      openProject: (dirPath) => {
        const opened = deps.invoke('project_open', { path: dirPath });
        // Recorded in recent_projects (project::recents::remember) — refresh
        // the native File menu so it reflects the new order without a
        // restart (fix round 1, F6; same fire-and-forget shape as
        // menu-actions.js's open-folder/open-recent-project handlers).
        Promise.resolve(opened).then(() => deps.invoke('rebuild_menu').catch(() => {})).catch(() => {});
        return opened;
      },
      toastError: (title, body) => { if (g.toast) g.toast.error(title, body); },
    });
  }

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
    const setEditorVimMode = deps.setEditorVimMode;
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

      // Unsaved-changes handshake. The webview owns the answer to "is anything
      // unsaved?", so Rust stops the close, emits one of these, and waits to
      // be told.
      //
      // One prompt at a time: a request that arrives while another is being
      // answered is refused rather than allowed to stack a second dialog on
      // top of the first. Refused, though — not dropped. Dropping it strands
      // the sender: Rust's quit poll waits on a vote that never comes and,
      // because request_quit early-returns while a quit is pending, Cmd+Q is
      // then dead for the rest of the session. A "no" costs at most one extra
      // keystroke; silence costs the feature.
      {
        let answering = false;
        const answerCloseRequest = async (confirmCommand) => {
          if (answering) {
            invoke(confirmCommand, { allow: false }).catch(() => {});
            return;
          }
          answering = true;
          try {
            const service = global.termlabEditorService;
            // No editor service means no editors, and therefore nothing to
            // lose — go ahead rather than wedging a window that can never be
            // closed.
            const ok = service && typeof service.confirmAllDirty === 'function'
              ? await service.confirmAllDirty()
              : true;
            await invoke(confirmCommand, { allow: ok });
          } catch (error) {
            // Whether anything is unsaved is now unknown, so do not consent.
            // Rust is told "no" explicitly rather than left waiting.
            showStatus('Could not check for unsaved changes: ' + String(error));
            try {
              await invoke(confirmCommand, { allow: false });
            } catch (_) {}
          } finally {
            answering = false;
          }
        };

        await listenOnCurrentWindow('window-close-requested', () => {
          answerCloseRequest('confirm_window_close');
        });
        await listenOnCurrentWindow('app-quit-requested', () => {
          answerCloseRequest('quit_vote');
        });
        // Only armed windows get their close prevented. A window whose
        // frontend never reaches this line (bundle missing, script error, the
        // settings window's separate document) keeps the ordinary close
        // behaviour instead of becoming unclosable.
        try {
          await invoke('window_close_guard_arm');
        } catch (error) {
          showStatus('Unsaved-changes guard unavailable: ' + String(error));
        }
      }

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

      if (global.termlabTabSwitcherRuntime && global.termlabTabSwitcherView && global.termlabTabMru) {
        let tabSwitcherRuntime = null;
        const tabSwitcherView = global.termlabTabSwitcherView.create({
          getTabContainerEl: (tabId) => {
            const tab = tabs.get(tabId);
            return tab ? tab.containerEl : null;
          },
          // Every tab's tree fills the terminal host, so the host's size is
          // the natural size of any preview clone — including hidden tabs,
          // whose own rects measure 0.
          getStageSize: () => {
            const host = document.getElementById('terminal-host');
            return host ? host.getBoundingClientRect() : null;
          },
          onPick: (index) => {
            if (tabSwitcherRuntime) tabSwitcherRuntime.commitIndex(index);
          },
        });
        tabSwitcherRuntime = global.termlabTabSwitcherRuntime.create({
          getTabItems: () => Array.from(tabs.values()).map((tab) => ({
            id: tab.id,
            label: tab.label,
            kind: tab.type,
          })),
          activateTab: (tabId) => deps.activateTab(tabId),
          view: tabSwitcherView,
        });
        tabSwitcherRuntime.init();
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
          setEditorVimMode: (value) => {
            if (typeof setEditorVimMode === 'function') setEditorVimMode(value);
          },
        });
        configRuntime.init();
      }

      // Build (but do NOT run) the pending-open drain. Editor service
      // composition (manager-compose-runtime's __termlabPaneAccess) has
      // already run by this point in main-runtime.js's boot sequence, so
      // termlabEditorService.openLocalFile is safe to bind here.
      //
      // Running it is main-runtime.js's job, and it must happen AFTER the
      // first tab is created: createEditorTab activates its own tab
      // synchronously, so a drain racing createTab() lands the file the user
      // asked for behind an empty terminal roughly half the time.
      const pendingOpenDrain = wirePendingOpenDrain(global, { invoke });

      // A CLI open-path aimed at THIS already-running window arrives as an
      // event after Rust seeded the window's queue (see
      // open_path.rs::open_in_running_app) — the same take-based drain the
      // boot path uses, so a racing boot drain and this listener can both
      // fire without double-opening: whoever pulls first gets the paths.
      if (pendingOpenDrain && typeof listenOnCurrentWindow === 'function') {
        listenOnCurrentWindow('open-paths-pending', () => {
          pendingOpenDrain.drainPendingOpens();
        });
      }

      return {
        handleMenuAction,
        showUpdateAvailableToast,
        // Null when this window has nothing to drain (see
        // wirePendingOpenDrain).
        drainPendingOpens: pendingOpenDrain
          ? () => pendingOpenDrain.drainPendingOpens()
          : null,
        // Routes an already-pulled path list; main-runtime pulls the queue
        // itself, early, to decide the window layout before any tab exists.
        routePendingPaths: pendingOpenDrain
          ? (paths) => pendingOpenDrain.routePaths(paths)
          : null,
      };
    }

    return {
      init,
    };
  }

  global.termlabEventWiringRuntime = {
    create,
    wirePendingOpenDrain,
  };
})(window);
