(function initConchPaneManager(global) {
  function create(deps) {
    const getPanes = deps.getPanes;
    const getTabs = deps.getTabs;
    const getFocusedPaneId = deps.getFocusedPaneId;
    const setFocusedPaneId = deps.setFocusedPaneId;
    const rebuildTreeDOM = deps.rebuildTreeDOM;
    const onTerminalFocused = deps.onTerminalFocused;
    const unregisterPaneDnd = deps.unregisterPaneDnd;
    const notifyTerminalClosed = deps.notifyTerminalClosed;
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

      const nextRoot = global.conchSplitRuntime && global.conchSplitRuntime.insertAroundLeaf
        ? global.conchSplitRuntime.insertAroundLeaf(
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

    function closePane(paneId) {
      const panes = getPanes();
      const tabs = getTabs();
      const pane = panes.get(paneId);
      if (!pane) return;
      unregisterPaneDnd(paneId);

      const tab = tabs.get(pane.tabId);
      if (!tab || !global.splitTree) return;
      if (global.splitTree.leafCount(tab.treeRoot) <= 1) {
        closeTab(tab.id);
        return;
      }

      if (pane.kind === 'terminal' && pane.type === 'tmux') {
        const tmuxPaneId = global.tmuxIdMap ? global.tmuxIdMap.getTmuxForPane(paneId) : null;
        if (tmuxPaneId != null && global.backendRouter) {
          global.backendRouter.closePane(tmuxPaneId).catch((error) => {
            toastError('Failed to close tmux pane: ' + error);
          });
        }
        return;
      }

      if (pane.kind === 'terminal' && pane.spawned) {
        notifyTerminalClosed(paneId, pane.type);
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
    }

    async function splitPane(direction) {
      const panes = getPanes();
      const tabs = getTabs();
      const pane = currentPane();
      if (!pane || !global.splitTree) return;

      const tab = tabs.get(pane.tabId);
      if (!tab) return;

      if (pane.kind === 'terminal' && pane.type === 'tmux') {
        const tmuxPaneId = global.tmuxIdMap ? global.tmuxIdMap.getTmuxForPane(pane.paneId) : null;
        if (tmuxPaneId == null || !global.backendRouter) return;
        try {
          if (direction === 'vertical') {
            await global.backendRouter.splitVertical(tmuxPaneId);
          } else {
            await global.backendRouter.splitHorizontal(tmuxPaneId);
          }
        } catch (error) {
          toastError('Failed to split tmux pane: ' + error);
        }
        return;
      }

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
      setFocusedPane,
      movePaneByDrop,
      closePane,
      splitPane,
    };
  }

  global.conchPaneManager = {
    create,
  };
})(window);
