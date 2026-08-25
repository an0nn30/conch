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
    const getTermFontSize = deps.getTermFontSize;
    const getEditorVimMode = deps.getEditorVimMode;

    const getActiveTabId = deps.getActiveTabId;
    const setActiveTabId = deps.setActiveTabId;
    const allocTabLabel = deps.allocTabLabel;
    const setNextTabLabel = deps.setNextTabLabel;
    const allocTabId = deps.allocTabId;
    const allocPaneId = deps.allocPaneId;
    const getFocusedPaneId = deps.getFocusedPaneId;
    const setFocusedPaneId = deps.setFocusedPaneId;
    const getPaneDnd = deps.getPaneDnd;

    // The parent half of the pop-out event bridge (app/core/panel-host-
    // bridge.js): publishes the pane/tab the files panel just switched to so
    // any live panel-host window can re-run the same onTabChanged a docked
    // panel gets for free. Null when the bridge script has not loaded (a
    // window that skipped index.html's normal script graph), in which case
    // publishActivePaneChanged below is a no-op — a pop-out that can never
    // exist has nothing to hear the broadcast anyway.
    const panelHostBridge = global.termlabPanelHostBridge && global.termlabPanelHostBridge.create
      ? global.termlabPanelHostBridge.create({ invoke })
      : null;

    // `target` is either a PANE object (paneId, type, spawned — see
    // pane-manager.js) or a TAB object (id, type, focusedPaneId — see
    // tab-manager.js); files-panel.js's onTabChanged already duck-types
    // between the two (files-panel.js:177-213). Only the primitive fields it
    // actually reads are serialized — target is a live object with methods,
    // DOM refs and xterm instances, none of which survives (or should
    // attempt to survive) an IPC hop to another window.
    function publishActivePaneChanged(target) {
      if (!panelHostBridge || !target) return;
      panelHostBridge.publish(global.termlabPanelHostBridge.ACTIVE_PANE_CHANGED_EVENT, {
        type: target.type,
        spawned: target.spawned,
        paneId: target.paneId,
        focusedPaneId: target.focusedPaneId,
        id: target.id,
      });
    }

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
            publishActivePaneChanged(pane);
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
          getTermFontSize: () => (typeof getTermFontSize === 'function' ? getTermFontSize() : 0),
          getEditorVimMode: () => (typeof getEditorVimMode === 'function' ? getEditorVimMode() === true : false),
          setFocusedPane: (paneId) => setFocusedPane(paneId),
          fitAndResizeTab: (tab) => {
            if (layoutRuntime && layoutRuntime.fitAndResizeTab) return layoutRuntime.fitAndResizeTab(tab);
            return fitAndResizeTab(tab);
          },
          onTabChanged: (target) => {
            if (global.filesPanel) global.filesPanel.onTabChanged(target);
            publishActivePaneChanged(target);
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
          // PRECONDITION: only call this when every editor in this window is
          // already gone.
          //
          // destroy() sends a raw WindowMessage::Destroy, which
          // tauri-runtime-wry routes to on_window_close WITHOUT emitting
          // CloseRequested — so the unsaved-changes guard in
          // crates/termlab_tauri/src/close_guard.rs never runs and never can.
          // This is the one window-teardown path the guard is structurally
          // blind to.
          //
          // It is safe today because of its single caller: tab-manager.js's
          // closeTab, under `if (tabs.size === 0 && closeWindowWhenLast)`.
          // That branch is reached only after a closeTab that already asked
          // about the closing tab's editors emptied the last tab, so by
          // construction there is nothing left to lose. Calling this from
          // anywhere else silently discards unsaved work; use
          // `win.close()` instead, which does raise CloseRequested.
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
          getLocalPaneProcess: (paneId) => invoke('get_local_pane_process', { paneId }).catch(() => null),
          getHostIdentity: () => invoke('get_host_identity').catch(() => null),
          getWorkspaceDir: () => invoke('get_workspace_dir').catch(() => null),
          allocateTabId: () => allocTabId(),
          allocatePaneId: () => allocPaneId(),
          // Tabs are named 'Terminal' like the reference app; the ordinal lives in
          // the Cmd+N shortcut, not the label.
          allocateTabLabel: () => { allocTabLabel(); return 'Terminal'; },
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

    // The composed tabManager instance lives inside main-runtime's closure and
    // has no window handle. `global.termlabTabManager` is the FACTORY ({create}),
    // not an instance, so it cannot be used to open an editor tab. Publish the
    // one entry point that callers outside the closure need — the editor
    // service and the file-open command — under its own name, alongside the
    // other `__termlab*` escape hatches this app already uses.
    if (tabManager && typeof tabManager.createEditorTab === 'function') {
      global.__termlabCreateEditorTab = (options) => tabManager.createEditorTab(options);
    }

    // Same problem, one level further out: editor-service.js is a plain script
    // with no way into this closure, and it needs to ask which pane is focused,
    // walk every pane, and focus an already-open editor. `panes` is the live
    // Map main-runtime owns, and paneManager/tabManager are the composed
    // instances — none of the three has a window handle. Publish read/focus
    // access under one name rather than letting callers guess at
    // `global.paneManager`, which does not exist.
    if (paneManager && tabManager) {
      global.__termlabPaneAccess = {
        currentPane: () => paneManager.currentPane(),
        allPanes: () => panes,
        setFocusedPane: (paneId) => paneManager.setFocusedPane(paneId),
        activateTab: (tabId) => tabManager.activateTab(tabId),
        // Save As rebinds a pane to a new file, so its tab has to say so. The
        // caption is composed by features/editor/tab-label.js (the same
        // function createEditorTab uses); this only applies it, through the
        // tab manager's own setTabLabel rather than by writing textContent —
        // the button holds an icon, a label span and a close affordance, and
        // assigning textContent would destroy all three. `tab.label` is
        // updated too because the window title reads it. Returns whether the
        // tab was found.
        setTabLabel: (tabId, label, tooltip) => {
          const tab = tabs.get(tabId);
          if (!tab || !tab.button) return false;
          tab.label = label;
          tabManager.setTabLabel(tab.button, label);
          tab.button.title = tooltip || '';
          if (typeof tabManager.refreshWindowTitle === 'function') {
            tabManager.refreshWindowTitle();
          }
          return true;
        },
      };
      if (global.termlabLspBridge && typeof global.termlabLspBridge.configure === 'function') {
        global.termlabLspBridge.configure({
          windowLabel: currentWindowLabel,
          paneAccess: global.__termlabPaneAccess,
          onReservationFailed: (canonicalPath) => {
            const editor = global.termlabEditorService;
            if (editor && typeof editor.openLocalFile === 'function') {
              return editor.openLocalFile(canonicalPath);
            }
            return null;
          },
        });
      }
    }

    // vim's `:w` and `:q` have to mean what Cmd+S and closing the tab mean, and
    // this is the only scope where all three accessors exist at once:
    // savePane is a plain global (editor-service.js), closeTab lives on
    // managerDelegates, and currentPane comes off the composed paneManager.
    // Registering here rather than inside vim-mode.js is what keeps that
    // module from having to guess at any of them.
    //
    // Unconditional: the ex commands are registered against the vim engine
    // itself, not against a view, so they are in place before the first editor
    // pane exists and stay correct when the setting is toggled later. Cheap
    // when vim_mode is off, because nothing can type `:` at a vim prompt that
    // is not there.
    if (
      paneManager
      && managerDelegates
      && global.termlabVimMode
      && typeof global.termlabVimMode.registerExCommands === 'function'
    ) {
      global.termlabVimMode.registerExCommands({
        savePane: (pane) => {
          const service = global.termlabEditorService;
          if (!service || typeof service.savePane !== 'function') {
            // Rejects rather than silently doing nothing: vim-mode turns this
            // into a "Save Failed" toast, so `:w` never looks like it worked.
            return Promise.reject(new Error('editor service unavailable'));
          }
          return service.savePane(pane);
        },
        // The GUARDED close — a dirty editor still gets its Save/Don't
        // Save/Cancel prompt. closePane and view.destroy() do not ask.
        closeTab: (tabId) => managerDelegates.closeTab(tabId),
        currentPane: () => paneManager.currentPane(),
      });
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
