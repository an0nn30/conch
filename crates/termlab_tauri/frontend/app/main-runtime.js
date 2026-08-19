    const startupRuntime = window.termlabStartupRuntime && window.termlabStartupRuntime.create
      ? window.termlabStartupRuntime.create()
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
    window.__termlabShowStatus = showStatus;
    let theme = fallbackTheme;
    const runBootstrap = window.termlabBootstrap && window.termlabBootstrap.run
      ? window.termlabBootstrap.run
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

      const tauriClient = window.termlabTauriClient && typeof window.termlabTauriClient.create === 'function'
        ? window.termlabTauriClient.create({ tauri })
        : null;
      if (!tauriClient) {
        showStatus('Tauri client is unavailable.');
        return;
      }
      const invoke = (command, payload) => tauriClient.invoke(command, payload);
      const listen = (eventName, handler) => tauriClient.listen(eventName, handler);
      const listenOnCurrentWindow = (eventName, handler) => tauriClient.listenOnCurrentWindow(eventName, handler);
      const currentWindow = tauriClient.currentWindow || null;
      const layoutService = window.termlabLayoutService && typeof window.termlabLayoutService.create === 'function'
        ? window.termlabLayoutService.create({ invoke })
        : null;
      window.termlabServices = Object.assign({}, window.termlabServices || {}, {
        tauriClient,
        layoutService,
      });
      const currentWindowLabel = await invoke('current_window_label');

      const FONT_FALLBACKS = ', "Symbols Nerd Font Mono", "Symbols Nerd Font", "Menlo", "DejaVu Sans Mono", "Consolas", "Liberation Mono", monospace';
      // The editor theme reads this global so editor and terminal share one font source.
      let termFontFamily = '"JetBrains Mono", "Fira Code", "Cascadia Code"' + FONT_FALLBACKS;
      window.__termlabTermFontFamily = termFontFamily;
      let termFontSize = 14;
      let termCursorStyle = 'block';
      let termCursorBlink = true;
      let termScrollSensitivity = 1;
      // Vim keybindings for the editor ([editor] vim_mode). Owned here for the
      // same reason termFontSize is: two consumers need it at different times
      // — tab-manager reads it when an editor pane is created, config-runtime
      // rewrites it when the setting changes — and neither can see the other.
      let editorVimMode = false;
      const startupTermConfigPromise = startupRuntime && startupRuntime.loadTerminalConfig
        ? startupRuntime.loadTerminalConfig(invoke, FONT_FALLBACKS)
        : Promise.resolve({
            fontFamily: termFontFamily,
            fontSize: termFontSize,
            cursorStyle: termCursorStyle,
            cursorBlink: termCursorBlink,
            scrollSensitivity: termScrollSensitivity,
          });
      const startupAppConfigPromise = startupRuntime && startupRuntime.applyAppConfig
        ? startupRuntime.applyAppConfig(invoke)
        : Promise.resolve({ borderlessMode: false, vimMode: false });
      // Chained onto the app-config promise, not run alongside it: the theme
      // fetch has to carry the RESOLVED appearance, and applyAppConfig is
      // what resolves it (termlabAppearance.apply from appearance_mode). Under
      // the default `auto` theme, fetching first would paint the dark built-in
      // on a light install. The theme is applied to already-created panes far
      // later (see startupThemePromise.then below) and the window reveal
      // already awaits startupAppConfigPromise, so this costs no visible time.
      const startupThemePromise = startupRuntime && startupRuntime.loadTheme
        ? startupAppConfigPromise
            .catch(() => {})
            .then(() => startupRuntime.loadTheme(invoke, fallbackTheme))
        : Promise.resolve(fallbackTheme);
      // A second consumer of the same promise: the block further down awaits
      // it for layout, this one only needs the flag. Editor tabs are opened by
      // the user, long after startup, so getEditorVimMode() below never reads
      // the seed value.
      startupAppConfigPromise.then((appResult) => {
        editorVimMode = !!(appResult && appResult.vimMode);
      }).catch(() => {});

      // Track webview zoom level for menu-driven zoom in/out.
      let currentZoom = 1.0;
      invoke('get_zoom_level').then(z => { currentZoom = z; }).catch(() => {});

      const shortcutDebugEnabled = true;
      const composition = window.termlabComposeRuntime && window.termlabComposeRuntime.create
        ? window.termlabComposeRuntime.create({
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
            pluginViewPaneById: new Map(),
            pluginViewSizeMemory: new Map(),
            nextPaneId: 1,
            focusedPaneId: null,
          };
      const tabs = initialState.tabs;
      let activeTabId = initialState.activeTabId;
      let nextTabId = initialState.nextTabId;
      let nextTabLabel = initialState.nextTabLabel;
      const panes = initialState.panes;
      const pluginViewPaneById = initialState.pluginViewPaneById;
      const pluginViewSizeMemory = initialState.pluginViewSizeMemory;
      let nextPaneId = initialState.nextPaneId;
      let focusedPaneId = initialState.focusedPaneId;
      const inputRuntime = composition && composition.inputRuntime
        ? composition.inputRuntime
        : { isTextInputTarget: () => false };
      const layoutRuntime = window.termlabLayoutRuntime && window.termlabLayoutRuntime.create
        ? window.termlabLayoutRuntime.create({
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
            rememberPluginViewSize: () => { throw new Error('managerDelegates.rememberPluginViewSize is unavailable'); },
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
            createEditorTab: () => { throw new Error('managerDelegates.createEditorTab is unavailable'); },
            createSshTab: () => { throw new Error('managerDelegates.createSshTab is unavailable'); },
            refreshWindowTitle: () => { throw new Error('managerDelegates.refreshWindowTitle is unavailable'); },
          };

      let paneDnd = null;
      const managerComposer = window.termlabManagerComposeRuntime && window.termlabManagerComposeRuntime.create
        ? window.termlabManagerComposeRuntime.create({
            invoke,
            tauri,
            tabs,
            panes,
            appEl,
            tabBarEl,
            terminalHostEl,
            pluginViewPaneById,
            pluginViewSizeMemory,
            managerDelegates,
            terminalRuntime,
            layoutRuntime,
            shortcutDebugEnabled,
            currentWindowLabel,
            getTermFontSize: () => termFontSize,
            getEditorVimMode: () => editorVimMode,
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
            rememberPluginViewSize: (pane) => managerDelegates.rememberPluginViewSize(pane),
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
      const refreshWindowTitle = (...args) => managerDelegates.refreshWindowTitle(...args);

      let handleMenuAction = () => {
        throw new Error('handleMenuAction is unavailable');
      };
      const bridgeRuntime = window.termlabBridgeRuntime && window.termlabBridgeRuntime.create
        ? window.termlabBridgeRuntime.create({
            invoke,
            listenOnCurrentWindow,
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
      if (window.termlabEventWiringRuntime && window.termlabEventWiringRuntime.create) {
        const eventWiringRuntime = window.termlabEventWiringRuntime.create({
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
          createTab: (options) => createTab(options),
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
              window.__termlabTermFontFamily = termFontFamily;
            }
            if (partial && Object.prototype.hasOwnProperty.call(partial, 'fontSize')) {
              termFontSize = partial.fontSize;
            }
          },
          setEditorVimMode: (value) => { editorVimMode = value === true; },
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

      // Apply persisted layout before reveal to avoid visible startup rearrange.
      try {
        await startupAppConfigPromise;
      } catch (_) {}

      // Initialize orchestration/tool-window runtime before show so sidebars,
      // zones, and panel visibility are already in final restored state.
      if (window.termlabOrchestrationRuntime && window.termlabOrchestrationRuntime.create) {
        const orchestrationRuntime = window.termlabOrchestrationRuntime.create({
          invoke,
          listen,
          listenOnCurrentWindow,
          layoutService,
          terminalHostEl,
          currentWindow,
          tabs,
          panes,
          pluginViewPaneById,
          pluginViewSizeMemory,
          getActiveTabId: () => activeTabId,
          allocPaneId: () => nextPaneId++,
          currentPane: () => currentPane(),
          currentTab: () => currentTab(),
          setFocusedPane: (paneId) => setFocusedPane(paneId),
          closePane: (paneId) => closePane(paneId),
          createTab: (options) => createTab(options),
          createSshTab: (opts) => createSshTab(opts),
          activateTab: (tabId) => activateTab(tabId),
          splitPane: (direction) => splitPane(direction),
          refreshWindowTitle: () => refreshWindowTitle(),
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
          console.warn('Orchestration init failed:', error);
        }
      }

      const firstTabPromise = createTab().catch((e) => {
        showStatus('Failed to initialize first tab: ' + String(e));
      });
      await firstTabPromise;
      // Show the window (secondary windows are created hidden). This runs as
      // early as possible: the window already opened at the exact size for the
      // configured columns x lines, computed in Rust from the cell metrics
      // persisted by the last run — so there is nothing to wait for. Do NOT
      // move this behind requestAnimationFrame or the sizing pass below:
      // WKWebView does not fire rAF while the window is hidden, so a show
      // scheduled there deadlocks until Rust's rescue timer fires.
      try {
        await invoke('app_ready');
      } catch (e) {
        showStatus('Failed to show window: ' + String(e));
      }

      // Measure the settled window and persist the cell/chrome metrics, so
      // the NEXT launch opens at exactly the configured columns x lines.
      //
      // This function must NEVER resize the window. Rust already opened it at
      // final size — from a previous launch's measurement, or from native
      // metrics parsed out of the bundled font file. A launch whose stored
      // measurement was missing or stale opens slightly off and STAYS there;
      // the fresh measurement below makes the next one exact. Every design
      // that corrected the window after show animated on screen, in three
      // separate attempts, and is not to be reintroduced.
      async function measureAndPersistWindowMetrics() {
        const sizer = window.termlabWindowSize;
        const tauriWin = window.__TAURI__ && window.__TAURI__.window
          ? window.__TAURI__.window.getCurrentWindow()
          : null;
        if (!sizer || !tauriWin) return;

        // The measurement is only meaningful once the configured terminal
        // font is applied and loaded — a cell measured under the fallback
        // font poisons every future launch. Both are real conditions, not
        // timers: the config promise resolves after its .then below applied
        // the font, and fonts.ready resolves once the @font-face files are in.
        await startupTermConfigPromise.catch(() => {});
        if (document.fonts && document.fonts.ready) {
          await document.fonts.ready.catch(() => {});
        }
        debouncedFitAndResize();
        await new Promise((resolve) => requestAnimationFrame(resolve));
        await new Promise((resolve) => requestAnimationFrame(resolve));

        const pane = currentPane();
        const host = document.getElementById('terminal-host');
        if (!pane || !pane.term || !host) return;

        const inner = await tauriWin.innerSize();
        const scale = await tauriWin.scaleFactor();
        const rendererCell = sizer.rendererCellSize(pane.term) || {};
        const metrics = sizer.metricsFor(
          {
            cols: pane.term.cols,
            rows: pane.term.rows,
            width: host.clientWidth,
            height: host.clientHeight,
            cellWidth: rendererCell.width,
            cellHeight: rendererCell.height,
          },
          { width: inner.width / scale, height: inner.height / scale },
        );
        if (!metrics) return;
        invoke('save_window_metrics', {
          cellWidth: metrics.cellWidth,
          cellHeight: metrics.cellHeight,
          chromeWidth: metrics.chromeWidth,
          chromeHeight: metrics.chromeHeight,
        }).catch((err) => {
          // Loud on purpose: a silently-failed save means every future launch
          // opens from the fallback estimate, with nothing saying why.
          console.error('Failed to persist window metrics:', err);
        });
      }

      // Tell the user why this window has no panels — otherwise a window that
      // opens bare looks broken rather than deliberate. Only for windows that
      // got zen from the setting, never for one that inherited it from the
      // saved layout.
      if (window.__termlabZenIsSessionDefault === true && window.toast) {
        window.toast.info('Zen mode', 'New windows open in zen mode by default. Change this in Settings → Appearance.');
      }

      // The restored bottom-zone height and sidebar widths are applied after
      // the terminal's first fit, so xterm keeps its pre-restore rows and
      // leaves a gap ("chin") under the terminal until something forces a
      // refit — which is why resizing the window cleared it. Re-fit once the
      // window is shown and layout has settled, with a late pass for slow
      // first paints.
      requestAnimationFrame(() => {
        debouncedFitAndResize();
        setTimeout(() => {
          debouncedFitAndResize();
          // Only now are the font, the bottom-zone height and the sidebar
          // widths all applied, so this is the first moment the terminal's
          // measurements reflect what the user will actually see.
          measureAndPersistWindowMetrics().catch((err) => {
            // Deliberately not silent: an earlier version referenced an
            // identifier that does not exist in this scope, and a bare catch
            // hid the ReferenceError so the measurement simply never happened
            // while appearing to work.
            console.error('Failed to measure window metrics:', err);
          });
        }, 250);
      });

      startupTermConfigPromise.then((termConfig) => {
        if (!termConfig) return;
        termFontFamily = termConfig.fontFamily;
        window.__termlabTermFontFamily = termFontFamily;
        termFontSize = termConfig.fontSize;
        termCursorStyle = termConfig.cursorStyle;
        termCursorBlink = termConfig.cursorBlink;
        termScrollSensitivity = termConfig.scrollSensitivity;
        for (const pane of panes.values()) {
          // Font size only for editor panes: cursor style/blink are xterm
          // options, and there is no fitAddon to fit — CodeMirror reflows
          // itself when its font compartment is reconfigured.
          if (pane.kind === 'editor' && pane.view && window.termlabEditorPane) {
            window.termlabEditorPane.setFontSize(pane.view, termFontSize);
            continue;
          }
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
          // resolvedTheme is an xterm theme object and means nothing to
          // CodeMirror; the editor rebuilds from the --tl-* variables the same
          // startup path has just applied.
          if (pane.kind === 'editor' && pane.view && window.termlabEditorPane) {
            window.termlabEditorPane.refreshTheme(pane.view);
          }
        }
      }).catch(() => {});

      // Finish non-critical UI work after the terminal is visible.
      setTimeout(async () => {
        // Preload the bundled Nerd Font in the background so later glyph
        // fallback is ready without delaying first paint.
        try {
          await document.fonts.load(termFontSize + 'px "Symbols Nerd Font Mono"');
        } catch (_) { /* not fatal */ }
      }, 0);
    });
