(function initConchLayoutRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const getPanes = deps.getPanes;
    const getTabs = deps.getTabs;
    const allPanesInTab = deps.allPanesInTab;
    const getCurrentTab = deps.getCurrentTab;
    const renderTree = deps.renderTree;

    function getAnyTmuxPaneIdForTab(tab, panes) {
      if (!tab || !window.tmuxIdMap) return null;
      const paneIds = allPanesInTab(tab.id);
      for (const paneId of paneIds) {
        const tmuxPaneId = window.tmuxIdMap.getTmuxForPane(paneId);
        if (tmuxPaneId != null) return tmuxPaneId;
      }
      return null;
    }

    function getPaneCharDims(pane) {
      if (!pane) return null;
      const cols = Number(pane.term && pane.term.cols) || Number(pane.lastCols) || 0;
      const rows = Number(pane.term && pane.term.rows) || Number(pane.lastRows) || 0;
      if (cols > 0 && rows > 0) {
        return { cols, rows };
      }
      if (!pane.fitAddon) return null;
      const proposed = pane.fitAddon.proposeDimensions();
      if (!proposed || !proposed.cols || !proposed.rows) return null;
      return { cols: proposed.cols, rows: proposed.rows };
    }

    function computeTreeCharDims(node, panes) {
      if (!node) return null;
      if (node.type === 'leaf') {
        return getPaneCharDims(panes.get(node.paneId));
      }
      if (!node.children || node.children.length !== 2) return null;
      const first = computeTreeCharDims(node.children[0], panes);
      const second = computeTreeCharDims(node.children[1], panes);
      if (!first && !second) return null;
      if (!first) return second;
      if (!second) return first;

      if (node.direction === 'vertical') {
        return {
          cols: first.cols + second.cols,
          rows: Math.max(first.rows, second.rows),
        };
      }
      return {
        cols: Math.max(first.cols, second.cols),
        rows: first.rows + second.rows,
      };
    }

    function resizeTmuxClientForTab(tab) {
      if (!tab || tab.type !== 'tmux' || !window.backendRouter || !window.tmuxIdMap) return;
      const panes = getPanes();
      let cols = 0;
      let rows = 0;
      const treeDims = computeTreeCharDims(tab.treeRoot, panes);
      if (treeDims && treeDims.cols > 0 && treeDims.rows > 0) {
        cols = treeDims.cols;
        rows = treeDims.rows;
      } else {
        const focused = panes.get(tab.focusedPaneId);
        const pane = focused || panes.get(allPanesInTab(tab.id)[0]);
        if (!pane || !pane.fitAddon || !pane.root || !tab.containerEl) return;
        const dims = pane.fitAddon.proposeDimensions();
        if (!dims || !dims.cols || !dims.rows) return;

        const paneWidth = pane.root.clientWidth || 0;
        const paneHeight = pane.root.clientHeight || 0;
        if (!paneWidth || !paneHeight) return;

        const cellWidth = paneWidth / dims.cols;
        const cellHeight = paneHeight / dims.rows;
        if (!Number.isFinite(cellWidth) || !Number.isFinite(cellHeight) || cellWidth <= 0 || cellHeight <= 0) return;

        const containerWidth = tab.containerEl.clientWidth || paneWidth;
        const containerHeight = tab.containerEl.clientHeight || paneHeight;
        cols = Math.floor(containerWidth / cellWidth);
        rows = Math.floor(containerHeight / cellHeight);
      }

      cols = Math.max(2, Math.floor(Number(cols) || 0));
      rows = Math.max(2, Math.floor(Number(rows) || 0));
      if (!cols || !rows) return;

      if (tab._tmuxClientCols === cols && tab._tmuxClientRows === rows) return;
      tab._tmuxClientCols = cols;
      tab._tmuxClientRows = rows;

      if (typeof window.backendRouter.resizeClient === 'function') {
        window.backendRouter.resizeClient(cols, rows).catch(() => {});
      } else {
        const tmuxPaneId = getAnyTmuxPaneIdForTab(tab, panes);
        if (tmuxPaneId == null) return;
        window.backendRouter.resizePane(tmuxPaneId, cols, rows).catch(() => {});
      }
    }

    function scheduleTmuxClientResize(tab, delayMs) {
      if (!tab || tab.type !== 'tmux') return;
      if (tab._tmuxResizeTimer) {
        clearTimeout(tab._tmuxResizeTimer);
      }
      tab._tmuxResizeTimer = setTimeout(() => {
        tab._tmuxResizeTimer = null;
        resizeTmuxClientForTab(tab);
      }, typeof delayMs === 'number' ? delayMs : 30);
    }

    function fitAndResizePane(pane) {
      if (!pane || !pane.term || !pane.fitAddon || !pane.spawned) return;
      const proposed = pane.fitAddon.proposeDimensions();
      if (!proposed || !proposed.cols || !proposed.rows) return;
      pane.fitAddon.fit();
      const cols = Number(pane.term.cols) || proposed.cols;
      const rows = Number(pane.term.rows) || proposed.rows;
      if (!cols || !rows) return;
      if (cols === pane.lastCols && rows === pane.lastRows) return;
      pane.lastCols = cols;
      pane.lastRows = rows;
      if (pane.type === 'tmux' && window.tmuxIdMap && window.backendRouter) {
        const tmuxPaneId = window.tmuxIdMap.getTmuxForPane(pane.paneId);
        if (tmuxPaneId != null) {
          window.backendRouter.resizePane(tmuxPaneId, cols, rows).catch(() => {});
        }
        // Also keep the overall tmux client synced with container dimensions.
        const tabs = typeof getTabs === 'function' ? getTabs() : null;
        const tab = tabs && pane.tabId != null ? tabs.get(pane.tabId) : getCurrentTab();
        if (tab) scheduleTmuxClientResize(tab, 10);
        return;
      }
      const cmd = pane.type === 'ssh' ? 'ssh_resize' : 'resize_pty';
      invoke(cmd, { paneId: pane.paneId, cols, rows }).catch(() => {});
    }

    function fitAndResizeTab(tab) {
      if (!tab) return;
      const panes = getPanes();
      if (tab.type === 'tmux') {
        for (const id of allPanesInTab(tab.id)) {
          const pane = panes.get(id);
          if (pane) fitAndResizePane(pane);
        }
        scheduleTmuxClientResize(tab, 0);
        return;
      }
      const paneId = tab.focusedPaneId;
      if (paneId != null) {
        const pane = panes.get(paneId);
        if (pane) {
          fitAndResizePane(pane);
          return;
        }
      }
      for (const id of allPanesInTab(tab.id)) {
        const pane = panes.get(id);
        if (pane) fitAndResizePane(pane);
      }
    }

    let fitDebounceTimer = null;
    function debouncedFitAndResize() {
      clearTimeout(fitDebounceTimer);
      fitDebounceTimer = setTimeout(() => {
        fitAndResizeTab(getCurrentTab());
      }, 100);
    }

    function normalizeTabTitle(rawTitle, fallback) {
      const cleaned = String(rawTitle || '').replace(/\s+/g, ' ').trim();
      if (!cleaned) return fallback;
      return cleaned;
    }

    function rebuildTreeDOM(tab) {
      const panes = getPanes();
      const containerEl = tab.containerEl;
      while (containerEl.firstChild) {
        containerEl.removeChild(containerEl.firstChild);
      }
      const rendered = renderTree(tab.treeRoot, (id) => panes.get(id).root);
      containerEl.appendChild(rendered);
    }

    return {
      fitAndResizePane,
      fitAndResizeTab,
      debouncedFitAndResize,
      normalizeTabTitle,
      rebuildTreeDOM,
    };
  }

  global.conchLayoutRuntime = {
    create,
  };
})(window);
