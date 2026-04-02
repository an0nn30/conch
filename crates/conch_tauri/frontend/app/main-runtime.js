    const startupRuntime = window.conchStartupRuntime && window.conchStartupRuntime.create
      ? window.conchStartupRuntime.create()
      : null;
    const fallbackTheme = {
      background: '#282a36', foreground: '#f8f8f2',
      cursor: '#f8f8f2', cursorAccent: '#282a36',
      selectionBackground: '#44475a', selectionForeground: '#f8f8f2',
      black: '#21222c', red: '#ff5555', green: '#50fa7b', yellow: '#f1fa8c',
      blue: '#bd93f9', magenta: '#ff79c6', cyan: '#8be9fd', white: '#f8f8f2',
      brightBlack: '#6272a4', brightRed: '#ff6e6e', brightGreen: '#69ff94',
      brightYellow: '#ffffa5', brightBlue: '#d6acff', brightMagenta: '#ff92df',
      brightCyan: '#a4ffff', brightWhite: '#ffffff',
    };

    const statusController = startupRuntime && startupRuntime.initStatusController
      ? startupRuntime.initStatusController()
      : {
          showStatus: (message) => console.error(message),
          hideStatus: () => {},
        };
    const showStatus = statusController.showStatus;
    window.__conchShowStatus = showStatus;
    let theme = fallbackTheme;
    const runBootstrap = window.conchBootstrap && window.conchBootstrap.run
      ? window.conchBootstrap.run
      : (startFn) => Promise.resolve().then(() => startFn());

    runBootstrap(async function start() {
      const tauri = window.__TAURI__;
      if (!startupRuntime || !startupRuntime.ensureRuntimeDependencies) {
        showStatus('Startup runtime is unavailable.');
        return;
      }
      if (!startupRuntime.ensureRuntimeDependencies(tauri, showStatus)) {
        return;
      }

      const { invoke } = tauri.core;
      const { listen } = tauri.event;
      const currentWindow = tauri.window && typeof tauri.window.getCurrentWindow === 'function'
        ? tauri.window.getCurrentWindow()
        : null;
      const listenOnCurrentWindow = (eventName, handler) => {
        if (currentWindow && typeof currentWindow.listen === 'function') {
          return currentWindow.listen(eventName, handler);
        }
        return listen(eventName, handler);
      };
      const currentWindowLabel = await invoke('current_window_label');

      const FONT_FALLBACKS = ', "Symbols Nerd Font Mono", "Symbols Nerd Font", "Menlo", "DejaVu Sans Mono", "Consolas", "Liberation Mono", monospace';
      let termFontFamily = '"JetBrains Mono", "Fira Code", "Cascadia Code"' + FONT_FALLBACKS;
      let termFontSize = 14;
      let termCursorStyle = 'block';
      let termCursorBlink = true;
      let termScrollSensitivity = 1;
      const startupTermConfigPromise = startupRuntime && startupRuntime.loadTerminalConfig
        ? startupRuntime.loadTerminalConfig(invoke, FONT_FALLBACKS)
        : Promise.resolve({
            fontFamily: termFontFamily,
            fontSize: termFontSize,
            cursorStyle: termCursorStyle,
            cursorBlink: termCursorBlink,
            scrollSensitivity: termScrollSensitivity,
          });
      const startupThemePromise = startupRuntime && startupRuntime.loadTheme
        ? startupRuntime.loadTheme(invoke, fallbackTheme)
        : Promise.resolve(fallbackTheme);
      const startupAppConfigPromise = startupRuntime && startupRuntime.applyAppConfig
        ? startupRuntime.applyAppConfig(invoke)
        : Promise.resolve({ borderlessMode: false });

      // Track webview zoom level for menu-driven zoom in/out.
      let currentZoom = 1.0;
      invoke('get_zoom_level').then(z => { currentZoom = z; }).catch(() => {});

      const shortcutDebugEnabled = true;
      const composition = window.conchComposeRuntime && window.conchComposeRuntime.create
        ? window.conchComposeRuntime.create({
            invoke,
            tauri,
            getTheme: () => theme,
            getTermFontFamily: () => termFontFamily,
            getTermFontSize: () => termFontSize,
            getTermCursorStyle: () => termCursorStyle,
            getTermCursorBlink: () => termCursorBlink,
            getTermScrollSensitivity: () => termScrollSensitivity,
            isShortcutDebugEnabled: () => shortcutDebugEnabled,
          })
        : null;
      const appEl = composition && composition.appEl ? composition.appEl : document.getElementById('app');
      const tabBarEl = composition && composition.tabBarEl ? composition.tabBarEl : document.getElementById('tabbar');
      const terminalHostEl = composition && composition.terminalHostEl ? composition.terminalHostEl : document.getElementById('terminal-host');
      const initialState = composition && composition.initialState
        ? composition.initialState
        : {
            tabs: new Map(),
            activeTabId: null,
            nextTabId: 1,
            nextTabLabel: 1,
            panes: new Map(),
            nextPaneId: 1,
            focusedPaneId: null,
          };
      const tabs = initialState.tabs;
      let activeTabId = initialState.activeTabId;
      let nextTabId = initialState.nextTabId;
      let nextTabLabel = initialState.nextTabLabel;
      const panes = initialState.panes;
      let nextPaneId = initialState.nextPaneId;
      let focusedPaneId = initialState.focusedPaneId;
      const inputRuntime = composition && composition.inputRuntime
        ? composition.inputRuntime
        : { isTextInputTarget: () => false };
      const layoutRuntime = window.conchLayoutRuntime && window.conchLayoutRuntime.create
        ? window.conchLayoutRuntime.create({
            invoke,
            getPanes: () => panes,
            allPanesInTab: (tabId) => allPanesInTab(tabId),
            getCurrentTab: () => currentTab(),
            renderTree: (treeRoot, getRoot) => window.splitPane.renderTree(treeRoot, getRoot),
          })
        : null;
      const terminalRuntime = composition && composition.terminalRuntime
        ? composition.terminalRuntime
        : {
            toDebugEscaped: (text) => String(text || ''),
            toDebugHex: () => '',
            shouldDebugKeyEvent: () => false,
            formatKeyEventForDebug: () => '{}',
            terminalMouseModeIsActive: () => false,
            setupTmuxRightClickBridge: () => () => {},
            initTerminal: () => ({ term: null, fitAddon: null }),
          };
      const managerDelegates = composition && composition.managerDelegates
        ? composition.managerDelegates
        : {
            setPaneManager: () => {},
            setTabManager: () => {},
            currentPane: () => { throw new Error('managerDelegates.currentPane is unavailable'); },
            refocusActiveTerminal: () => { throw new Error('managerDelegates.refocusActiveTerminal is unavailable'); },
            getTabForPane: () => { throw new Error('managerDelegates.getTabForPane is unavailable'); },
            allPanesInTab: () => { throw new Error('managerDelegates.allPanesInTab is unavailable'); },
            setFocusedPane: () => { throw new Error('managerDelegates.setFocusedPane is unavailable'); },
            closePane: () => { throw new Error('managerDelegates.closePane is unavailable'); },
            splitPane: () => { throw new Error('managerDelegates.splitPane is unavailable'); },
            currentTab: () => { throw new Error('managerDelegates.currentTab is unavailable'); },
            updateTabBarVisibility: () => { throw new Error('managerDelegates.updateTabBarVisibility is unavailable'); },
            renumberTabs: () => { throw new Error('managerDelegates.renumberTabs is unavailable'); },
            activateTab: () => { throw new Error('managerDelegates.activateTab is unavailable'); },
            closeTab: () => { throw new Error('managerDelegates.closeTab is unavailable'); },
            makeTabButton: () => { throw new Error('managerDelegates.makeTabButton is unavailable'); },
            setTabLabel: () => { throw new Error('managerDelegates.setTabLabel is unavailable'); },
            getTabLabel: () => { throw new Error('managerDelegates.getTabLabel is unavailable'); },
            renameActiveTab: () => { throw new Error('managerDelegates.renameActiveTab is unavailable'); },
            startTabRename: () => { throw new Error('managerDelegates.startTabRename is unavailable'); },
            createTab: () => { throw new Error('managerDelegates.createTab is unavailable'); },
            createSshTab: () => { throw new Error('managerDelegates.createSshTab is unavailable'); },
          };

      let paneDnd = null;
      const managerComposer = window.conchManagerComposeRuntime && window.conchManagerComposeRuntime.create
        ? window.conchManagerComposeRuntime.create({
            invoke,
            tauri,
            tabs,
            panes,
            appEl,
            tabBarEl,
            terminalHostEl,
            managerDelegates,
            terminalRuntime,
            layoutRuntime,
            shortcutDebugEnabled,
            currentWindowLabel,
            getActiveTabId: () => activeTabId,
            setActiveTabId: (tabId) => { activeTabId = tabId; },
            setNextTabLabel: (value) => { nextTabLabel = value; },
            allocTabId: () => nextTabId++,
            allocPaneId: () => nextPaneId++,
            allocTabLabel: () => nextTabLabel++,
            getFocusedPaneId: () => focusedPaneId,
            setFocusedPaneId: (paneId) => { focusedPaneId = paneId; },
            getPaneDnd: () => paneDnd,
            rebuildTreeDOM: (tab) => rebuildTreeDOM(tab),
            fitAndResizePane: (pane) => fitAndResizePane(pane),
            fitAndResizeTab: (tab) => fitAndResizeTab(tab),
            normalizeTabTitle: (rawTitle, fallback) => normalizeTabTitle(rawTitle, fallback),
            allPanesInTab: (tabId) => managerDelegates.allPanesInTab(tabId),
            setFocusedPane: (paneId) => managerDelegates.setFocusedPane(paneId),
            closeTabDelegate: (tabId) => managerDelegates.closeTab(tabId),
            showStatus: (message) => showStatus(message),
          })
        : null;
      const paneManager = managerComposer ? managerComposer.paneManager : null;
      const tabManager = managerComposer ? managerComposer.tabManager : null;
      const currentPane = (...args) => managerDelegates.currentPane(...args);
      const refocusActiveTerminal = (...args) => managerDelegates.refocusActiveTerminal(...args);
      const allPanesInTab = (...args) => managerDelegates.allPanesInTab(...args);
      const setFocusedPane = (...args) => managerDelegates.setFocusedPane(...args);
      const currentTab = (...args) => managerDelegates.currentTab(...args);
      const activateTab = (...args) => managerDelegates.activateTab(...args);
      const closeTab = (...args) => managerDelegates.closeTab(...args);
      const renameActiveTab = (...args) => managerDelegates.renameActiveTab(...args);
      const startTabRename = (...args) => managerDelegates.startTabRename(...args);
      const createTab = (...args) => managerDelegates.createTab(...args);
      const createSshTab = (...args) => managerDelegates.createSshTab(...args);
      const closePane = (...args) => managerDelegates.closePane(...args);
      const splitPane = (...args) => managerDelegates.splitPane(...args);

      let handleMenuAction = () => {
        throw new Error('handleMenuAction is unavailable');
      };
      const bridgeRuntime = window.conchBridgeRuntime && window.conchBridgeRuntime.create
        ? window.conchBridgeRuntime.create({
            invoke,
            showStatus: (message) => showStatus(message),
            inputRuntime,
            layoutRuntime,
            currentPane: () => currentPane(),
            currentTab: () => currentTab(),
            createSshTab: (opts) => createSshTab(opts),
            getHandleMenuAction: () => handleMenuAction,
          })
        : null;
      const fitAndResizePane = (pane) => bridgeRuntime && bridgeRuntime.fitAndResizePane ? bridgeRuntime.fitAndResizePane(pane) : undefined;
      const fitAndResizeTab = (tab) => bridgeRuntime && bridgeRuntime.fitAndResizeTab ? bridgeRuntime.fitAndResizeTab(tab) : undefined;
      const debouncedFitAndResize = () => bridgeRuntime && bridgeRuntime.debouncedFitAndResize ? bridgeRuntime.debouncedFitAndResize() : undefined;
      const normalizeTabTitle = (rawTitle, fallback) => bridgeRuntime && bridgeRuntime.normalizeTabTitle
        ? bridgeRuntime.normalizeTabTitle(rawTitle, fallback)
        : fallback;
      const rebuildTreeDOM = (tab) => bridgeRuntime && bridgeRuntime.rebuildTreeDOM ? bridgeRuntime.rebuildTreeDOM(tab) : undefined;

      let debouncedSaveLayout = () => {};

      const isTextInputTarget = (el) => bridgeRuntime && bridgeRuntime.isTextInputTarget ? bridgeRuntime.isTextInputTarget(el) : inputRuntime.isTextInputTarget(el);
      const writeTextToCurrentPane = (text) => bridgeRuntime && bridgeRuntime.writeTextToCurrentPane ? bridgeRuntime.writeTextToCurrentPane(text) : false;
      const pasteIntoCurrentPane = (explicitText) => bridgeRuntime && bridgeRuntime.pasteIntoCurrentPane
        ? bridgeRuntime.pasteIntoCurrentPane(explicitText)
        : Promise.resolve(false);
      const openCommandPalette = () => bridgeRuntime && bridgeRuntime.openCommandPalette ? bridgeRuntime.openCommandPalette() : Promise.resolve();
      const closeCommandPalette = (refocus = true) => {
        if (bridgeRuntime && bridgeRuntime.closeCommandPalette) bridgeRuntime.closeCommandPalette(refocus);
      };
      const isCommandPaletteOpen = () => bridgeRuntime && bridgeRuntime.isCommandPaletteOpen ? bridgeRuntime.isCommandPaletteOpen() : false;
      if (bridgeRuntime && bridgeRuntime.initClipboardListeners) {
        bridgeRuntime.initClipboardListeners();
      }
      let showUpdateAvailableToast = (_info) => {};
      if (window.conchEventWiringRuntime && window.conchEventWiringRuntime.create) {
        const eventWiringRuntime = window.conchEventWiringRuntime.create({
          invoke,
          listen,
          listenOnCurrentWindow,
          currentWindowLabel,
          terminalHostEl,
          tabBarEl,
          tabs,
          panes,
          getActiveTabId: () => activeTabId,
          getFocusedPaneId: () => focusedPaneId,
          getCurrentPane: () => currentPane(),
          getCurrentTab: () => currentTab(),
          closeTab: (tabId) => closeTab(tabId),
          createTab: () => createTab(),
          closePane: (paneId) => closePane(paneId),
          splitPane: (direction) => splitPane(direction),
          renameActiveTab: () => renameActiveTab(),
          setFocusedPane: (paneId) => setFocusedPane(paneId),
          startTabRename: (tab) => startTabRename(tab),
          fitAndResizeTab: (tab) => fitAndResizeTab(tab),
          debouncedSaveLayout: () => debouncedSaveLayout(),
          showStatus: (message) => showStatus(message),
          isTextInputTarget: (el) => isTextInputTarget(el),
          writeTextToCurrentPane: (text) => writeTextToCurrentPane(text),
          pasteIntoCurrentPane: (explicitText) => pasteIntoCurrentPane(explicitText),
          openCommandPalette: () => openCommandPalette(),
          closeCommandPalette: (refocus) => closeCommandPalette(refocus),
          isCommandPaletteOpen: () => isCommandPaletteOpen(),
          refocusActiveTerminal: () => refocusActiveTerminal(),
          terminalRuntime,
          shortcutDebugEnabled,
          getZoom: () => currentZoom,
          setZoom: (value) => { currentZoom = value; },
          getThemeState: () => theme,
          setThemeState: (nextTheme) => { theme = nextTheme; },
          getTermConfigState: () => ({
            fontFamily: termFontFamily,
            fontSize: termFontSize,
          }),
          setTermConfigState: (partial) => {
            if (partial && Object.prototype.hasOwnProperty.call(partial, 'fontFamily')) {
              termFontFamily = partial.fontFamily;
            }
            if (partial && Object.prototype.hasOwnProperty.call(partial, 'fontSize')) {
              termFontSize = partial.fontSize;
            }
          },
          fontFallbacks: FONT_FALLBACKS,
          activateTab: (tabId) => activateTab(tabId),
        });
        const eventWiringResult = await eventWiringRuntime.init();
        if (eventWiringResult) {
          if (typeof eventWiringResult.handleMenuAction === 'function') {
            handleMenuAction = eventWiringResult.handleMenuAction;
          }
          if (typeof eventWiringResult.showUpdateAvailableToast === 'function') {
            showUpdateAvailableToast = eventWiringResult.showUpdateAvailableToast;
          }
        }
      }

      const firstTabPromise = createTab().catch((e) => {
        showStatus('Failed to initialize first tab: ' + String(e));
      });
      try {
        await invoke('app_ready');
      } catch (e) {
        showStatus('Failed to show window: ' + String(e));
      }

      await firstTabPromise;

      startupTermConfigPromise.then((termConfig) => {
        if (!termConfig) return;
        termFontFamily = termConfig.fontFamily;
        termFontSize = termConfig.fontSize;
        termCursorStyle = termConfig.cursorStyle;
        termCursorBlink = termConfig.cursorBlink;
        termScrollSensitivity = termConfig.scrollSensitivity;
        for (const pane of panes.values()) {
          if (pane.kind !== 'terminal' || !pane.term) continue;
          pane.term.options.fontFamily = termFontFamily;
          pane.term.options.fontSize = termFontSize;
          pane.term.options.cursorStyle = termCursorStyle;
          pane.term.options.cursorBlink = termCursorBlink;
          if (pane.fitAddon) pane.fitAddon.fit();
        }
      }).catch(() => {});

      startupThemePromise.then((resolvedTheme) => {
        if (!resolvedTheme) return;
        theme = resolvedTheme;
        for (const pane of panes.values()) {
          if (pane.kind === 'terminal' && pane.term) {
            pane.term.options.theme = resolvedTheme;
          }
        }
      }).catch(() => {});

      startupAppConfigPromise.catch(() => {});

      // Finish non-critical UI work after the terminal is visible.
      setTimeout(async () => {
        if (window.conchOrchestrationRuntime && window.conchOrchestrationRuntime.create) {
          const orchestrationRuntime = window.conchOrchestrationRuntime.create({
            invoke,
            listen,
            listenOnCurrentWindow,
            terminalHostEl,
            currentWindow,
            tabs,
            panes,
            getActiveTabId: () => activeTabId,
            allocPaneId: () => nextPaneId++,
            currentPane: () => currentPane(),
            currentTab: () => currentTab(),
            setFocusedPane: (paneId) => setFocusedPane(paneId),
            closePane: (paneId) => closePane(paneId),
            createTab: () => createTab(),
            createSshTab: (opts) => createSshTab(opts),
            splitPane: (direction) => splitPane(direction),
            getPaneManager: () => paneManager,
            isDebugEnabled: () => shortcutDebugEnabled,
            debugLog: (...args) => console.log(...args),
            debouncedFitAndResize: () => {
              if (layoutRuntime && layoutRuntime.debouncedFitAndResize) return layoutRuntime.debouncedFitAndResize();
              return debouncedFitAndResize();
            },
            rebuildTreeDOM: (tab) => rebuildTreeDOM(tab),
          });
          try {
            const orchestrationResult = await orchestrationRuntime.init();
            if (orchestrationResult) {
              if (typeof orchestrationResult.debouncedSaveLayout === 'function') {
                debouncedSaveLayout = orchestrationResult.debouncedSaveLayout;
              }
              paneDnd = orchestrationResult.paneDnd || null;
            }
          } catch (error) {
            console.warn('Deferred orchestration init failed:', error);
          }
        }

        // Preload the bundled Nerd Font in the background so later glyph
        // fallback is ready without delaying first paint.
        try {
          await document.fonts.load(termFontSize + 'px "Symbols Nerd Font Mono"');
        } catch (_) { /* not fatal */ }
      }, 0);
    });
