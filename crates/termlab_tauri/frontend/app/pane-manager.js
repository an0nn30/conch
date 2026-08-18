(function initTermLabPaneManager(global) {
  function create(deps) {
    const getPanes = deps.getPanes;
    const getTabs = deps.getTabs;
    const getFocusedPaneId = deps.getFocusedPaneId;
    const setFocusedPaneId = deps.setFocusedPaneId;
    const getPaneRatio = deps.getPaneRatio;
    const setPluginViewSize = deps.setPluginViewSize;
    const rebuildTreeDOM = deps.rebuildTreeDOM;
    const onTerminalFocused = deps.onTerminalFocused;
    const unregisterPaneDnd = deps.unregisterPaneDnd;
    const notifyTerminalClosed = deps.notifyTerminalClosed;
    const refreshSshSessions = deps.refreshSshSessions;
    const notifyPluginViewClosed = deps.notifyPluginViewClosed;
    const deletePluginViewPane = deps.deletePluginViewPane;
    const closeTab = deps.closeTab;
    const initTerminal = deps.initTerminal;
    const setupTmuxRightClickBridge = deps.setupTmuxRightClickBridge;
    const createPaneResizeObserver = deps.createPaneResizeObserver;
    const fitAndResizePane = deps.fitAndResizePane;
    const onLocalTerminalData = deps.onLocalTerminalData;
    const spawnShell = deps.spawnShell;
    const allocatePaneId = deps.allocatePaneId;
    const splitLeaf = deps.splitLeaf;
    const openSshChannel = deps.openSshChannel;
    const onSplitPaneData = deps.onSplitPaneData;
    const toastError = deps.toastError;

    function currentPane() {
      const panes = getPanes();
      const focusedPaneId = getFocusedPaneId();
      return panes.get(focusedPaneId) || null;
    }

    function refocusActiveTerminal() {
      const pane = currentPane();
      // A second focus entry point, independent of setFocusedPane — it is what
      // dialog-runtime calls on Escape and after the last dialog closes. It
      // cannot delegate to setFocusedPane, which early-returns when the pane is
      // already the focused one, which is exactly this case. Without the editor
      // arm, closing Settings leaves focus on <body> while the pane keeps its
      // `.focused` border: it looks focused and swallows every keystroke.
      if (pane && pane.kind === 'editor' && pane.view) {
        pane.view.focus();
        return true;
      }
      if (pane && pane.term) {
        pane.term.focus();
        return true;
      }
      return false;
    }

    function getTabForPane(paneId) {
      const panes = getPanes();
      const tabs = getTabs();
      const pane = panes.get(paneId);
      return pane ? tabs.get(pane.tabId) : null;
    }

    function allPanesInTab(tabId) {
      const tabs = getTabs();
      const tab = tabs.get(tabId);
      if (!tab || !global.splitTree) return [];
      return global.splitTree.allLeaves(tab.treeRoot);
    }

    function rememberPluginViewSize(pane) {
      if (!pane || pane.kind !== 'plugin_view' || !pane.viewId) return;
      const tabs = getTabs();
      const tab = tabs.get(pane.tabId);
      const ratio = getPaneRatio(tab, pane.paneId);
      if (ratio == null) return;
      setPluginViewSize(pane.viewId, ratio);
    }

    function setFocusedPane(paneId) {
      const panes = getPanes();
      const tabs = getTabs();
      const focusedPaneId = getFocusedPaneId();
      if (focusedPaneId === paneId) return false;

      if (focusedPaneId != null) {
        const oldPane = panes.get(focusedPaneId);
        if (oldPane && oldPane.root) oldPane.root.classList.remove('focused');
      }

      setFocusedPaneId(paneId);

      const pane = panes.get(paneId);
      if (pane && pane.root) {
        pane.root.classList.add('focused');
        if (pane.kind === 'terminal' && pane.term) {
          pane.term.focus();
        }
        if (pane.kind === 'editor' && pane.view) {
          pane.view.focus();
        }
        const tab = tabs.get(pane.tabId);
        if (tab) tab.focusedPaneId = paneId;
        if (pane.kind === 'terminal' && typeof onTerminalFocused === 'function') {
          onTerminalFocused(paneId, pane);
        }
      }

      return true;
    }

    function movePaneByDrop(dragPaneId, targetPaneId, zone) {
      const panes = getPanes();
      const tabs = getTabs();
      const dragPane = panes.get(dragPaneId);
      if (!dragPane) return false;
      const tab = tabs.get(dragPane.tabId);
      if (!tab || !tab.treeRoot || !global.splitTree) return false;

      if (zone === 'center') {
        if (targetPaneId == null) return false;
        const targetPane = panes.get(targetPaneId);
        if (!targetPane) return false;
        if (dragPane.tabId !== targetPane.tabId) return false;
        if (!global.splitTree.allLeaves(tab.treeRoot).includes(targetPaneId)) return false;
        setFocusedPane(targetPaneId);
        return true;
      }

      if (!['left', 'right', 'top', 'bottom'].includes(zone)) return false;
      if (targetPaneId == null) return false;

      const direction = (zone === 'left' || zone === 'right') ? 'vertical' : 'horizontal';
      const placeBefore = (zone === 'left' || zone === 'top');
      const removed = global.splitTree.removeLeaf(tab.treeRoot, dragPaneId);
      if (!removed) return false;

      const targetPane = panes.get(targetPaneId);
      if (!targetPane) return false;
      if (dragPane.tabId !== targetPane.tabId) return false;
      if (dragPaneId === targetPaneId) return false;
      if (!global.splitTree.allLeaves(removed).includes(targetPaneId)) return false;

      const nextRoot = global.termlabSplitRuntime && global.termlabSplitRuntime.insertAroundLeaf
        ? global.termlabSplitRuntime.insertAroundLeaf(
            removed,
            targetPaneId,
            dragPaneId,
            direction,
            placeBefore,
          )
        : null;
      if (!nextRoot) return false;

      tab.treeRoot = nextRoot;
      rebuildTreeDOM(tab);
      setFocusedPane(dragPaneId);
      return true;
    }

    async function closePane(paneId) {
      const panes = getPanes();
      const tabs = getTabs();
      const pane = panes.get(paneId);
      if (!pane) return;
      const tab = tabs.get(pane.tabId);

      // Ask before this function destroys a modified editor. Reachable on
      // default bindings: cmd+d beside a focused editor (splitPane has no kind
      // guard), then cmd+shift+w — which lands in the split branch below
      // rather than the single-leaf hand-off to closeTab, so nothing else on
      // the path would ever ask.
      //
      // Deliberately NOT asked for the single-leaf case: that delegates to
      // closeTab, which does its own asking, and asking here as well would
      // prompt twice for one keystroke. Deliberately before every mutation
      // below, including unregisterPaneDnd, so a cancel really does leave the
      // pane exactly as it was.
      if (
        tab && global.splitTree &&
        pane.kind === 'editor' && pane.dirty &&
        global.splitTree.leafCount(tab.treeRoot) > 1
      ) {
        const service = global.termlabEditorService;
        if (!service || typeof service.confirmDirtyPanes !== 'function') {
          if (typeof toastError === 'function') {
            toastError('Cannot confirm unsaved changes; pane not closed.');
          }
          return;
        }
        let ok = false;
        try {
          ok = await service.confirmDirtyPanes([pane]);
        } catch (error) {
          if (typeof toastError === 'function') {
            toastError('Could not check for unsaved changes: ' + String(error));
          }
          return;
        }
        if (!ok) return;
        // The prompt yielded to the event loop; the pane or its tab may have
        // been torn down underneath us in the meantime.
        if (panes.get(paneId) !== pane || tabs.get(pane.tabId) !== tab) return;
      }

      unregisterPaneDnd(paneId);

      if (!tab || !global.splitTree) return;
      if (pane.kind === 'plugin_view') rememberPluginViewSize(pane);

      if (global.splitTree.leafCount(tab.treeRoot) <= 1) {
        if (pane.kind === 'plugin_view') {
          if (pane.viewId) {
            notifyPluginViewClosed(pane.viewId);
            deletePluginViewPane(pane.viewId);
          }
          if (pane.cleanupMouseBridge) pane.cleanupMouseBridge();
          if (pane.resizeObserver) pane.resizeObserver.disconnect();
          if (pane.term) pane.term.dispose();

          const paneEl = pane.root;
          paneEl.innerHTML = '';
          delete paneEl.dataset.pluginViewId;

          const { term, fitAddon } = initTerminal(paneEl);
          const nextPane = {
            paneId: pane.paneId,
            tabId: tab.id,
            kind: 'terminal',
            type: 'local',
            connectionId: null,
            term,
            fitAddon,
            root: paneEl,
            spawned: false,
            lastCols: 0,
            lastRows: 0,
            cleanupMouseBridge: setupTmuxRightClickBridge(term, paneEl),
            resizeObserver: null,
            debounceTimer: null,
          };
          panes.set(paneId, nextPane);
          nextPane.resizeObserver = createPaneResizeObserver(nextPane, fitAndResizePane);
          paneEl.addEventListener('mousedown', () => setFocusedPane(paneId));
          term.onData((data) => {
            if (!nextPane.spawned) return;
            onLocalTerminalData(paneId, data);
          });

          setFocusedPane(paneId);
          const dims = fitAddon.proposeDimensions();
          const cols = dims ? dims.cols : 80;
          const rows = dims ? dims.rows : 24;
          spawnShell(paneId, cols, rows)
            .then(() => {
              nextPane.spawned = true;
              fitAndResizePane(nextPane);
            })
            .catch((error) => {
              term.writeln('\\x1b[31mFailed to spawn shell: ' + error + '\\x1b[0m');
            });
          return;
        }
        closeTab(tab.id);
        return;
      }

      let closedSshPane = false;
      if (pane.kind === 'terminal' && pane.spawned) {
        if (pane.type === 'ssh') closedSshPane = true;
        notifyTerminalClosed(paneId, pane.type);
      } else if (pane.kind === 'plugin_view' && pane.viewId) {
        notifyPluginViewClosed(pane.viewId);
        deletePluginViewPane(pane.viewId);
      } else if (pane.kind === 'editor') {
        // The single-leaf case above hands off to closeTab, which destroys the
        // view and discards the temp file; this is the split case, where the
        // pane goes away on its own and nothing else would ever do either.
        if (pane.view && global.termlabEditorPane) {
          global.termlabEditorPane.destroyEditorView(pane.view);
        }
        pane.view = null;
        // A remote editor's file is a download in a temp directory, so closing
        // the pane is the end of its life. Same discard closeTab does — leaving
        // it out here is how a split-close leaked the temp file and its
        // directories. The service owns the path rules; it refuses anything
        // that is not one of its own temp files.
        if (pane.remote && global.termlabEditorService
            && typeof global.termlabEditorService.discardRemoteTemp === 'function') {
          global.termlabEditorService.discardRemoteTemp(pane);
        }
      }

      if (pane.cleanupMouseBridge) pane.cleanupMouseBridge();
      if (pane.resizeObserver) pane.resizeObserver.disconnect();
      if (pane.term) pane.term.dispose();
      pane.root.remove();
      panes.delete(paneId);

      tab.treeRoot = global.splitTree.removeLeaf(tab.treeRoot, paneId);
      rebuildTreeDOM(tab);

      if (getFocusedPaneId() === paneId) {
        const firstId = global.splitTree.firstLeaf(tab.treeRoot);
        setFocusedPane(firstId);
      } else if (tab.focusedPaneId === paneId) {
        tab.focusedPaneId = global.splitTree.firstLeaf(tab.treeRoot);
      }

      if (closedSshPane && typeof refreshSshSessions === 'function') {
        refreshSshSessions();
        setTimeout(() => {
          refreshSshSessions();
        }, 150);
      }
    }

    async function splitPane(direction) {
      const panes = getPanes();
      const tabs = getTabs();
      const pane = currentPane();
      if (!pane || !global.splitTree) return;

      const tab = tabs.get(pane.tabId);
      if (!tab) return;

      const newPaneId = allocatePaneId();
      tab.treeRoot = splitLeaf(tab.treeRoot, pane.paneId, newPaneId, direction);

      const newPaneEl = document.createElement('div');
      newPaneEl.className = 'terminal-pane';
      newPaneEl.dataset.paneId = newPaneId;

      const { term, fitAddon } = initTerminal(newPaneEl);

      const newPane = {
        paneId: newPaneId,
        tabId: tab.id,
        kind: 'terminal',
        type: pane.kind === 'terminal' ? pane.type : 'local',
        connectionId: pane.kind === 'terminal' ? (pane.connectionId || null) : null,
        term,
        fitAddon,
        root: newPaneEl,
        spawned: false,
        lastCols: 0,
        lastRows: 0,
        cleanupMouseBridge: setupTmuxRightClickBridge(term, newPaneEl),
        resizeObserver: null,
        debounceTimer: null,
      };
      panes.set(newPaneId, newPane);

      rebuildTreeDOM(tab);
      newPane.resizeObserver = createPaneResizeObserver(newPane, fitAndResizePane);
      newPaneEl.addEventListener('mousedown', () => setFocusedPane(newPaneId));

      const dims = fitAddon.proposeDimensions() || { cols: 80, rows: 24 };

      if (pane.kind === 'terminal' && pane.type === 'ssh' && pane.connectionId) {
        try {
          await openSshChannel(newPaneId, pane.connectionId, dims.cols, dims.rows);
          newPane.spawned = true;
        } catch (error) {
          toastError('Failed to open SSH channel: ' + error);
        }
      } else {
        try {
          await spawnShell(newPaneId, dims.cols, dims.rows);
          newPane.spawned = true;
        } catch (error) {
          toastError('Failed to spawn shell: ' + error);
        }
      }

      term.onData((data) => {
        if (!newPane.spawned) return;
        onSplitPaneData(newPane, newPaneId, data);
      });

      setFocusedPane(newPaneId);
    }

    return {
      currentPane,
      refocusActiveTerminal,
      getTabForPane,
      allPanesInTab,
      rememberPluginViewSize,
      setFocusedPane,
      movePaneByDrop,
      closePane,
      splitPane,
    };
  }

  global.termlabPaneManager = {
    create,
  };
})(window);
