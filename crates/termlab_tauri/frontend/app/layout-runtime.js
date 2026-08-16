(function initTermLabLayoutRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const getPanes = deps.getPanes;
    const allPanesInTab = deps.allPanesInTab;
    const getCurrentTab = deps.getCurrentTab;
    const renderTree = deps.renderTree;

    // Resizing has two halves with different timing needs. The visual fit must
    // land in the same frame as the drag or the container shows through below
    // the terminal; the PTY notification only needs the size the drag settles
    // on, and firing one SIGWINCH per pointermove makes full-screen apps
    // redraw continuously. So: fit now, notify on a short trailing timer.
    const pendingResizes = new Map();
    let resizeNotifyTimer = null;

    function flushResizeNotifications() {
      resizeNotifyTimer = null;
      for (const entry of pendingResizes.values()) {
        const cmd = entry.pane.type === 'ssh' ? 'ssh_resize' : 'resize_pty';
        invoke(cmd, { paneId: entry.pane.paneId, cols: entry.cols, rows: entry.rows }).catch(() => {});
      }
      pendingResizes.clear();
    }

    function fitAndResizePane(pane) {
      if (!pane || !pane.term || !pane.fitAddon || !pane.spawned) return;
      const dims = pane.fitAddon.proposeDimensions();
      if (!dims || !dims.cols || !dims.rows) return;
      if (dims.cols === pane.lastCols && dims.rows === pane.lastRows) return;
      pane.lastCols = dims.cols;
      pane.lastRows = dims.rows;
      pane.fitAddon.fit();
      pendingResizes.set(pane.paneId, { pane, cols: dims.cols, rows: dims.rows });
      if (resizeNotifyTimer == null) resizeNotifyTimer = setTimeout(flushResizeNotifications, 60);
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

    // Coalesced to one fit per animation frame rather than debounced: a 100ms
    // debounce never fires during a continuous drag, so the terminal stayed at
    // its old size for the whole gesture.
    let fitFrame = null;
    const raf = typeof requestAnimationFrame === 'function'
      ? requestAnimationFrame
      : (cb) => setTimeout(cb, 16);
    function debouncedFitAndResize() {
      if (fitFrame != null) return;
      fitFrame = raf(() => {
        fitFrame = null;
        fitAndResizeTab(getCurrentTab());
      });
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

  global.termlabLayoutRuntime = {
    create,
  };
})(window);
