(function initConchManagerComposeRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const tauri = deps.tauri;
    const tabs = deps.tabs;
    const panes = deps.panes;
    const appEl = deps.appEl;
    const tabBarEl = deps.tabBarEl;
    const terminalHostEl = deps.terminalHostEl;
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
    const setFocusedPane = deps.setFocusedPane;
    const closeTabDelegate = deps.closeTabDelegate;
    const showStatus = deps.showStatus;

    function queueTmuxWrite(pane, frontendPaneId, tmuxPaneId, data, source) {
      if (!pane || tmuxPaneId == null || !global.backendRouter) return false;
      if (!pane._tmuxWriteQueue) {
        pane._tmuxWriteQueue = {
          tmuxPaneId,
          buffer: '',
          timer: null,
          inFlight: false,
        };
      }
      const queue = pane._tmuxWriteQueue;
      if (queue.tmuxPaneId !== tmuxPaneId) {
        queue.tmuxPaneId = tmuxPaneId;
        queue.buffer = '';
        if (queue.timer) {
          clearTimeout(queue.timer);
          queue.timer = null;
        }
      }

      queue.buffer += data;
      if (queue.timer) return true;

      const flush = () => {
        queue.timer = null;
        if (queue.inFlight) {
          queue.timer = setTimeout(flush, 12);
          return;
        }
        const chunk = queue.buffer;
        if (!chunk) return;
        queue.buffer = '';
        queue.inFlight = true;

        global.backendRouter.writeToPane(queue.tmuxPaneId, chunk).then(() => {
          console.info('[tmux] write success (' + source + ')', {
            frontendPaneId,
            tmuxPaneId: queue.tmuxPaneId,
            len: chunk.length,
          });
        }).catch((event) => {
          console.error('tmux_write error:', event);
        }).finally(() => {
          queue.inFlight = false;
          if (queue.buffer && !queue.timer) {
            queue.timer = setTimeout(flush, 0);
          }
        });
      };

      // Batch fast keypress bursts into fewer tmux send operations.
      queue.timer = setTimeout(flush, 12);
      return true;
    }

    const paneManager = global.conchPaneManager && global.conchPaneManager.create
      ? global.conchPaneManager.create({
          getPanes: () => panes,
          getTabs: () => tabs,
          getFocusedPaneId: () => getFocusedPaneId(),
          setFocusedPaneId: (paneId) => setFocusedPaneId(paneId),
          getPaneRatio: (tab, paneId) => (
            global.conchSplitRuntime && global.conchSplitRuntime.paneRatioInTree
              ? global.conchSplitRuntime.paneRatioInTree(tab, paneId)
              : null
          ),
          rebuildTreeDOM: (tab) => {
            if (layoutRuntime && layoutRuntime.rebuildTreeDOM) return layoutRuntime.rebuildTreeDOM(tab);
            return rebuildTreeDOM(tab);
          },
          onTerminalFocused: (paneId, pane) => {
            if (global.filesPanel) global.filesPanel.onTabChanged(pane);
            if (pane && pane.type === 'tmux') {
              const tmuxPaneId = pane.tmuxPaneId != null
                ? pane.tmuxPaneId
                : (global.tmuxIdMap ? global.tmuxIdMap.getTmuxForPane(paneId) : null);
              if (tmuxPaneId != null && global.backendRouter) {
                global.backendRouter.selectPane(tmuxPaneId).catch(() => {});
              }
              return;
            }
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
          closeTab: (tabId) => closeTabDelegate(tabId),
          initTerminal: (root) => terminalRuntime.initTerminal(root),
          setupTmuxRightClickBridge: (term, terminalRoot) => terminalRuntime.setupTmuxRightClickBridge(term, terminalRoot),
          createPaneResizeObserver: (pane, fitCb) => global.splitPane.createPaneResizeObserver(pane, fitCb),
          fitAndResizePane: (pane) => {
            if (layoutRuntime && layoutRuntime.fitAndResizePane) return layoutRuntime.fitAndResizePane(pane);
            return fitAndResizePane(pane);
          },
          onLocalTerminalData: (paneId, data) => {
            const pane = panes.get(paneId);
            if (shortcutDebugEnabled) {
              console.log(
                `[conch-keydbg] xterm.onData pane=${paneId} len=${data.length} esc=${data.includes('\x1b')}`,
                JSON.stringify({ escaped: terminalRuntime.toDebugEscaped(data), hex: terminalRuntime.toDebugHex(data) })
              );
            }
            if (pane && pane.type === 'tmux' && global.tmuxIdMap && global.backendRouter) {
              const tmuxPaneId = pane.tmuxPaneId != null
                ? pane.tmuxPaneId
                : global.tmuxIdMap.getTmuxForPane(paneId);
              if (tmuxPaneId != null) {
                console.info('[tmux] write request (local path)', {
                  frontendPaneId: paneId,
                  tmuxPaneId: tmuxPaneId,
                  len: data.length,
                  liveActive: !!pane._tmuxLiveActive,
                });
                queueTmuxWrite(pane, paneId, tmuxPaneId, data, 'local path');
                return;
              }
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

    const tabManager = global.conchTabManager && global.conchTabManager.create
      ? global.conchTabManager.create({
          getTabs: () => tabs,
          getPanes: () => panes,
          getActiveTabId: () => getActiveTabId(),
          setActiveTabId: (tabId) => setActiveTabId(tabId),
          getFocusedPaneId: () => getFocusedPaneId(),
          setFocusedPaneId: (paneId) => setFocusedPaneId(paneId),
          setNextTabLabel: (value) => setNextTabLabel(value),
          appEl,
          setFocusedPane: (paneId) => setFocusedPane(paneId),
          rebuildTreeDOM: (tab) => {
            if (layoutRuntime && layoutRuntime.rebuildTreeDOM) return layoutRuntime.rebuildTreeDOM(tab);
            return rebuildTreeDOM(tab);
          },
          fitAndResizeTab: (tab) => {
            if (layoutRuntime && layoutRuntime.fitAndResizeTab) return layoutRuntime.fitAndResizeTab(tab);
            return fitAndResizeTab(tab);
          },
          onTabChanged: (target) => {
            if (global.filesPanel) global.filesPanel.onTabChanged(target);
          },
          allPanesInTab: (tabId) => allPanesInTab(tabId),
          unregisterPaneDnd: (paneId) => {
            const paneDnd = getPaneDnd();
            if (paneDnd) paneDnd.unregisterPane(paneId);
          },
          notifyTerminalClosed: (paneId, paneType) => {
            const cmd = paneType === 'ssh' ? 'ssh_disconnect' : 'close_pty';
            invoke(cmd, { paneId }).catch(() => {});
          },
          showStatus: (message) => showStatus(message),
          destroyCurrentWindow: async () => {
            const windowApi = tauri.window;
            if (windowApi && typeof windowApi.getCurrentWindow === 'function') {
              const win = windowApi.getCurrentWindow();
              await win.destroy();
            }
          },
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
                `[conch-keydbg] xterm.onData pane=${paneId} len=${data.length} esc=${data.includes('\x1b')}`,
                JSON.stringify({ escaped: terminalRuntime.toDebugEscaped(data), hex: terminalRuntime.toDebugHex(data) })
              );
            }
            if (pane.type === 'tmux' && global.tmuxIdMap && global.backendRouter) {
              const tmuxPaneId = pane.tmuxPaneId != null
                ? pane.tmuxPaneId
                : global.tmuxIdMap.getTmuxForPane(paneId);
              console.info('[tmux] onTerminalData routing', {
                frontendPaneId: paneId,
                paneType: pane.type,
                tmuxPaneId: tmuxPaneId == null ? null : tmuxPaneId,
                liveActive: !!pane._tmuxLiveActive,
              });
              if (tmuxPaneId != null) {
                queueTmuxWrite(pane, paneId, tmuxPaneId, data, 'runtime path');
                return;
              }
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
                `[conch-keydbg] xterm.onData pane=${paneId} len=${data.length} esc=${data.includes('\x1b')}`,
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

  global.conchManagerComposeRuntime = {
    create,
  };
})(window);
