(function initConchComposeRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const tauri = deps.tauri;
    const getTheme = deps.getTheme;
    const getTermFontFamily = deps.getTermFontFamily;
    const getTermFontSize = deps.getTermFontSize;
    const getTermCursorStyle = deps.getTermCursorStyle;
    const getTermCursorBlink = deps.getTermCursorBlink;
    const getTermScrollSensitivity = deps.getTermScrollSensitivity;
    const isShortcutDebugEnabled = deps.isShortcutDebugEnabled;

    const appEl = document.getElementById('app');
    const tabBarEl = document.getElementById('tabbar');
    const terminalHostEl = document.getElementById('terminal-host');

    const initialState = global.conchAppState && global.conchAppState.createInitialState
      ? global.conchAppState.createInitialState()
      : {
          tabs: new Map(),
          activeTabId: null,
          nextTabId: 1,
          nextTabLabel: 1,
          panes: new Map(),
          nextPaneId: 1,
          focusedPaneId: null,
        };

    const inputRuntime = global.conchInputRuntime && global.conchInputRuntime.create
      ? global.conchInputRuntime.create()
      : { isTextInputTarget: () => false };

    const terminalRuntime = global.conchTerminalRuntime && global.conchTerminalRuntime.create
      ? global.conchTerminalRuntime.create({
          tauriOpen: tauri.shell && typeof tauri.shell.open === 'function'
            ? (uri) => tauri.shell.open(uri)
            : null,
          getTheme: () => getTheme(),
          getFontFamily: () => getTermFontFamily(),
          getFontSize: () => getTermFontSize(),
          getCursorStyle: () => getTermCursorStyle(),
          getCursorBlink: () => getTermCursorBlink(),
          getScrollSensitivity: () => getTermScrollSensitivity(),
          isShortcutDebugEnabled: () => isShortcutDebugEnabled(),
        })
      : {
          toDebugEscaped: (text) => String(text || ''),
          toDebugHex: () => '',
          shouldDebugKeyEvent: () => false,
          formatKeyEventForDebug: () => '{}',
          terminalMouseModeIsActive: () => false,
          setupTmuxRightClickBridge: () => () => {},
          initTerminal: (root) => {
            const term = new global.Terminal({
              theme: getTheme(),
              fontFamily: getTermFontFamily(),
              fontSize: getTermFontSize(),
              cursorStyle: getTermCursorStyle(),
              cursorBlink: getTermCursorBlink(),
              scrollSensitivity: getTermScrollSensitivity(),
              macOptionIsMeta: true,
              allowProposedApi: true,
            });
            const fitAddon = new global.FitAddon.FitAddon();
            term.loadAddon(fitAddon);
            term.loadAddon(new global.WebLinksAddon.WebLinksAddon((_event, uri) => {
              if (tauri.shell && tauri.shell.open) tauri.shell.open(uri);
              else window.open(uri, '_blank');
            }));
            if (typeof global.Unicode11Addon !== 'undefined') {
              term.loadAddon(new global.Unicode11Addon.Unicode11Addon());
              term.unicode.activeVersion = '11';
            }
            term.open(root);
            return { term, fitAddon };
          },
        };

    const managerDelegates = global.conchManagerDelegatesRuntime && global.conchManagerDelegatesRuntime.create
      ? global.conchManagerDelegatesRuntime.create()
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
          createTmuxTab: () => { throw new Error('managerDelegates.createTmuxTab is unavailable'); },
        };

    return {
      appEl,
      tabBarEl,
      terminalHostEl,
      initialState,
      inputRuntime,
      terminalRuntime,
      managerDelegates,
    };
  }

  global.conchComposeRuntime = {
    create,
  };
})(window);
