(function initConchLayoutRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const getPanes = deps.getPanes;
    const allPanesInTab = deps.allPanesInTab;
    const getCurrentTab = deps.getCurrentTab;
    const renderTree = deps.renderTree;

    function fitAndResizePane(pane) {
      if (!pane || !pane.term || !pane.fitAddon || !pane.spawned) return;
      const dims = pane.fitAddon.proposeDimensions();
      if (!dims || !dims.cols || !dims.rows) return;
      if (dims.cols === pane.lastCols && dims.rows === pane.lastRows) return;
      pane.lastCols = dims.cols;
      pane.lastRows = dims.rows;
      pane.fitAddon.fit();
      const cmd = pane.type === 'ssh' ? 'ssh_resize' : 'resize_pty';
      invoke(cmd, { paneId: pane.paneId, cols: dims.cols, rows: dims.rows }).catch(() => {});
    }

    function fitAndResizeTab(tab) {
      if (!tab) return;
      const panes = getPanes();
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
