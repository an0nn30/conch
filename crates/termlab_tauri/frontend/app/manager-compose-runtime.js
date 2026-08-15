(function initTermLabManagerComposeRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const tauri = deps.tauri;
    const tabs = deps.tabs;
    const panes = deps.panes;
    const appEl = deps.appEl;
    const tabBarEl = deps.tabBarEl;
    const terminalHostEl = deps.terminalHostEl;
    const pluginViewPaneById = deps.pluginViewPaneById;
    const pluginViewSizeMemory = deps.pluginViewSizeMemory;
    const managerDelegates = deps.managerDelegates;
    const terminalRuntime = deps.terminalRuntime;
    const layoutRuntime = deps.layoutRuntime;
    const shortcutDebugEnabled = deps.shortcutDebugEnabled;
    const currentWindowLabel = deps.currentWindowLabel;

    const getActiveTabId = deps.getActiveTabId;
    const setActiveTabId = deps.setActiveTabId;
    const allocTabLabel = deps.allocTabLabel;
    const setNextTabLabel = deps.setNextTabLabel;
    const allocTabId = deps.allocTabId;
    const allocPaneId = deps.allocPaneId;
    const getFocusedPaneId = deps.getFocusedPaneId;
    const setFocusedPaneId = deps.setFocusedPaneId;
    const getPaneDnd = deps.getPaneDnd;

    const rebuildTreeDOM = deps.rebuildTreeDOM;
    const fitAndResizePane = deps.fitAndResizePane;
    const fitAndResizeTab = deps.fitAndResizeTab;
    const normalizeTabTitle = deps.normalizeTabTitle;
    const allPanesInTab = deps.allPanesInTab;
    const rememberPluginViewSize = deps.rememberPluginViewSize;
    const setFocusedPane = deps.setFocusedPane;
    const closeTabDelegate = deps.closeTabDelegate;
    const showStatus = deps.showStatus;

    const paneManager = global.termlabPaneManager && global.termlabPaneManager.create
      ? global.termlabPaneManager.create({
          getPanes: () => panes,
          getTabs: () => tabs,
          getFocusedPaneId: () => getFocusedPaneId(),
          setFocusedPaneId: (paneId) => setFocusedPaneId(paneId),
          getPaneRatio: (tab, paneId) => (
            global.termlabSplitRuntime && global.termlabSplitRuntime.paneRatioInTree
              ? global.termlabSplitRuntime.paneRatioInTree(tab, paneId)
              : null
          ),
          setPluginViewSize: (viewId, ratio) => pluginViewSizeMemory.set(viewId, ratio),
          rebuildTreeDOM: (tab) => {
            if (layoutRuntime && layoutRuntime.rebuildTreeDOM) return layoutRuntime.rebuildTreeDOM(tab);
            return rebuildTreeDOM(tab);
          },
          onTerminalFocused: (paneId, pane) => {
            if (global.filesPanel) global.filesPanel.onTabChanged(pane);
            invoke('set_active_pane', { paneId }).catch(() => {});
          },
          unregisterPaneDnd: (paneId) => {
            const paneDnd = getPaneDnd();
            if (paneDnd) paneDnd.unregisterPane(paneId);
          },
          notifyTerminalClosed: (paneId, paneType) => {
            const cmd = paneType === 'ssh' ? 'ssh_disconnect' : 'close_pty';
            invoke(cmd, { paneId }).catch(() => {});
          },
          refreshSshSessions: () => {
            if (global.sshPanel) global.sshPanel.refreshSessions();
          },
          notifyPluginViewClosed: (viewId) => {
            invoke('plugin_view_closed', { viewId }).catch(() => {});
          },
          deletePluginViewPane: (viewId) => {
            pluginViewPaneById.delete(viewId);
          },
          closeTab: (tabId) => closeTabDelegate(tabId),
          initTerminal: (root) => terminalRuntime.initTerminal(root),
          setupTmuxRightClickBridge: (term, terminalRoot) => terminalRuntime.setupTmuxRightClickBridge(term, terminalRoot),
          createPaneResizeObserver: (pane, fitCb) => global.splitPane.createPaneResizeObserver(pane, fitCb),
          fitAndResizePane: (pane) => {
            if (layoutRuntime && layoutRuntime.fitAndResizePane) return layoutRuntime.fitAndResizePane(pane);
            return fitAndResizePane(pane);
          },
          onLocalTerminalData: (paneId, data) => {
            if (shortcutDebugEnabled) {
              console.log(
                `[termlab-keydbg] xterm.onData pane=${paneId} len=${data.length} esc=${data.includes('\x1b')}`,
                JSON.stringify({ escaped: terminalRuntime.toDebugEscaped(data), hex: terminalRuntime.toDebugHex(data) })
              );
            }
            invoke('write_to_pty', { paneId, data }).catch((event) => {
              console.error('write_to_pty error:', event);
            });
          },
          spawnShell: (paneId, cols, rows) => invoke('spawn_shell', { paneId, cols, rows }),
          spawnDefaultShell: (paneId, cols, rows) => invoke('spawn_default_shell', { paneId, cols, rows }),
          allocatePaneId: () => allocPaneId(),
          splitLeaf: (treeRoot, sourcePaneId, newPaneId, direction) => (
            global.splitTree.splitLeaf(treeRoot, sourcePaneId, newPaneId, direction)
          ),
          openSshChannel: (paneId, connectionId, cols, rows) => invoke('ssh_open_channel', {
            paneId,
            connectionId,
            cols,
            rows,
          }),
          onSplitPaneData: (pane, paneId, data) => {
            const cmd = pane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
            invoke(cmd, { paneId, data }).catch((event) => {
              console.error(cmd + ' error:', event);
            });
          },
          toastError: (message) => {
            if (global.toast && typeof global.toast.error === 'function') {
              global.toast.error(message);
            }
          },
        })
      : null;
    if (managerDelegates && managerDelegates.setPaneManager) {
      managerDelegates.setPaneManager(paneManager);
    }

    const tabManager = global.termlabTabManager && global.termlabTabManager.create
      ? global.termlabTabManager.create({
          getTabs: () => tabs,
          getPanes: () => panes,
          getActiveTabId: () => getActiveTabId(),
          setActiveTabId: (tabId) => setActiveTabId(tabId),
          getFocusedPaneId: () => getFocusedPaneId(),
          setFocusedPaneId: (paneId) => setFocusedPaneId(paneId),
          setNextTabLabel: (value) => setNextTabLabel(value),
          appEl,
          setFocusedPane: (paneId) => setFocusedPane(paneId),
          fitAndResizeTab: (tab) => {
            if (layoutRuntime && layoutRuntime.fitAndResizeTab) return layoutRuntime.fitAndResizeTab(tab);
            return fitAndResizeTab(tab);
          },
          onTabChanged: (target) => {
            if (global.filesPanel) global.filesPanel.onTabChanged(target);
          },
          allPanesInTab: (tabId) => allPanesInTab(tabId),
          rememberPluginViewSize: (pane) => rememberPluginViewSize(pane),
          unregisterPaneDnd: (paneId) => {
            const paneDnd = getPaneDnd();
            if (paneDnd) paneDnd.unregisterPane(paneId);
          },
          notifyTerminalClosed: (paneId, paneType) => {
            const cmd = paneType === 'ssh' ? 'ssh_disconnect' : 'close_pty';
            invoke(cmd, { paneId }).catch(() => {});
          },
          notifyPluginViewClosed: (viewId) => {
            invoke('plugin_view_closed', { viewId }).catch(() => {});
          },
          deletePluginViewPane: (viewId) => {
            pluginViewPaneById.delete(viewId);
          },
          showStatus: (message) => showStatus(message),
          destroyCurrentWindow: async () => {
            const windowApi = tauri.window;
            if (windowApi && typeof windowApi.getCurrentWindow === 'function') {
              const win = windowApi.getCurrentWindow();
              await win.destroy();
            }
          },
          setWindowTitle: async (title) => {
            try {
              const windowApi = tauri.window;
              if (windowApi && typeof windowApi.getCurrentWindow === 'function') {
                await windowApi.getCurrentWindow().setTitle(title);
              }
            } catch (_) {
              // Missing/denied Tauri window API must never break tab switching.
            }
          },
          getLocalPaneCwd: (paneId) => invoke('get_local_pane_cwd', { paneId }).catch(() => null),
          allocateTabId: () => allocTabId(),
          allocatePaneId: () => allocPaneId(),
          allocateTabLabel: () => 'Tab ' + allocTabLabel(),
          tabBarEl,
          terminalHostEl,
          initTerminal: (root) => terminalRuntime.initTerminal(root),
          setupTmuxRightClickBridge: (term, terminalRoot) => terminalRuntime.setupTmuxRightClickBridge(term, terminalRoot),
          createPaneResizeObserver: (pane, fitCb) => global.splitPane.createPaneResizeObserver(pane, fitCb),
          fitAndResizePane: (pane) => {
            if (layoutRuntime && layoutRuntime.fitAndResizePane) return layoutRuntime.fitAndResizePane(pane);
            return fitAndResizePane(pane);
          },
          makeLeaf: (paneId) => global.splitTree.makeLeaf(paneId),
          setupDividerDrag: (containerEl, getTree, setTree) => global.splitPane.setupDividerDrag(containerEl, getTree, setTree),
          normalizeTabTitle: (rawTitle, fallback) => {
            if (layoutRuntime && layoutRuntime.normalizeTabTitle) return layoutRuntime.normalizeTabTitle(rawTitle, fallback);
            return normalizeTabTitle(rawTitle, fallback);
          },
          onTerminalData: (pane, paneId, data) => {
            if (shortcutDebugEnabled) {
              console.log(
                `[termlab-keydbg] xterm.onData pane=${paneId} len=${data.length} esc=${data.includes('\x1b')}`,
                JSON.stringify({ escaped: terminalRuntime.toDebugEscaped(data), hex: terminalRuntime.toDebugHex(data) })
              );
            }
            const cmd = pane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
            invoke(cmd, { paneId, data }).catch((event) => {
              console.error(cmd + ' error:', event);
            });
          },
          spawnShell: (paneId, cols, rows) => invoke('spawn_shell', { paneId, cols, rows }),
          spawnDefaultShell: (paneId, cols, rows) => invoke('spawn_default_shell', { paneId, cols, rows }),
          onSshData: (_pane, paneId, data) => {
            if (shortcutDebugEnabled) {
              console.log(
                `[termlab-keydbg] xterm.onData pane=${paneId} len=${data.length} esc=${data.includes('\x1b')}`,
                JSON.stringify({ escaped: terminalRuntime.toDebugEscaped(data), hex: terminalRuntime.toDebugHex(data) })
              );
            }
            invoke('ssh_write', { paneId, data }).catch((event) => {
              console.error('ssh_write error:', event);
            });
          },
          connectSsh: async (opts, paneId, cols, rows) => {
            if (opts.serverId) {
              return invoke('ssh_connect', {
                paneId, serverId: opts.serverId, cols, rows, password: opts.password || null,
              });
            }
            if (opts.spec) {
              return invoke('ssh_quick_connect', {
                paneId, spec: opts.spec, cols, rows, password: opts.password || null,
              });
            }
            throw new Error('Missing SSH target');
          },
          ensureVaultUnlocked: async (resumeConnect) => {
            if (!global.vault) throw new Error('VAULT_LOCKED');
            return new Promise((resolve, reject) => {
              global.vault.ensureUnlocked(() => {
                resumeConnect().then(resolve, reject);
              });
            });
          },
          getCurrentWindowLabel: () => currentWindowLabel,
          refreshSshSessions: () => {
            if (global.sshPanel) global.sshPanel.refreshSessions();
          },
        })
      : null;
    if (managerDelegates && managerDelegates.setTabManager) {
      managerDelegates.setTabManager(tabManager);
    }

    return {
      paneManager,
      tabManager,
    };
  }

  global.termlabManagerComposeRuntime = {
    create,
  };
})(window);
