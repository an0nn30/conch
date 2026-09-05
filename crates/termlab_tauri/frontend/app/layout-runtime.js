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

    // Re-verify a pane's geometry this long after the last applied resize.
    // A fit during a live window drag can measure a transient layout, and a
    // direct fitAddon.fit() elsewhere (font changes) bypasses this module
    // entirely — either way xterm's grid or the PTY's size drifts from the
    // settled layout and a fullscreen app (tmux) sticks at the wrong size
    // until the window is nudged again. The settle pass re-runs the same
    // fit-and-notify; the live-state dedupe makes it a no-op when nothing
    // drifted, and each applied change re-arms it, so it converges.
    const SETTLE_VERIFY_DELAY_MS = 250;

    function notifyPaneResize(pane, cols, rows) {
      pane.lastNotifiedCols = cols;
      pane.lastNotifiedRows = rows;
      const cmd = pane.type === 'ssh' ? 'ssh_resize' : 'resize_pty';
      return invoke(cmd, { paneId: pane.paneId, cols, rows }).catch(() => {});
    }

    function flushResizeNotifications() {
      resizeNotifyTimer = null;
      for (const entry of pendingResizes.values()) {
        notifyPaneResize(entry.pane, entry.cols, entry.rows);
      }
      pendingResizes.clear();
    }

    function scheduleSettleVerify(pane) {
      if (pane.settleTimer != null) clearTimeout(pane.settleTimer);
      pane.settleTimer = setTimeout(() => {
        pane.settleTimer = null;
        fitAndResizePane(pane);
      }, SETTLE_VERIFY_DELAY_MS);
    }

    function fitAndResizePane(pane) {
      if (!pane || !pane.term || !pane.fitAddon || !pane.spawned) return;
      let dims = null;
      try {
        dims = pane.fitAddon.proposeDimensions();
      } catch (error) {
        // A disposed or detached terminal (a settle verify racing a pane
        // close) has nothing to measure.
        return;
      }
      // Mid-drag and mid-layout the container can transiently measure to
      // nothing; resizing xterm to a sub-2-cell grid is pure reflow stress
      // for a size that never survives the gesture. Skip; the settled size
      // arrives on a later frame.
      if (!dims || !Number.isFinite(dims.cols) || !Number.isFinite(dims.rows)) return;
      const cols = Math.floor(dims.cols);
      const rows = Math.floor(dims.rows);
      if (cols < 2 || rows < 2) return;
      // Informational only — the tab-title composer displays these. The
      // dedupe below deliberately never reads them.
      pane.lastCols = cols;
      pane.lastRows = rows;
      // Dedupe against LIVE state — xterm's actual grid and the size the PTY
      // was last told — never a private cache. A cache can go stale (a
      // mid-drag fit that measured a transient layout, a direct fit()
      // elsewhere) and then vetoes the very correction the settle pass is
      // here to make.
      if (cols === pane.term.cols && rows === pane.term.rows
          && cols === pane.lastNotifiedCols && rows === pane.lastNotifiedRows) return;
      try {
        pane.fitAddon.fit();
      } catch (error) {
        // xterm's resize/reflow can throw while a fullscreen app is
        // repainting mid-drag; an escaped throw used to leave the terminal
        // wedged (no rendering, no input) until an app restart. Reset
        // rebuilds the buffers in a consistent state, then fit again.
        console.error('terminal fit failed; resetting the terminal to recover', error);
        try {
          pane.term.reset();
          pane.fitAddon.fit();
        } catch (resetError) {
          console.error('terminal reset after a failed fit also failed', resetError);
          return;
        }
        // The reset blanked the screen and a fullscreen app repaints only on
        // a size CHANGE. Nudge the PTY one column narrower immediately; the
        // real size follows on the trailing notify below and the resulting
        // SIGWINCH forces a full repaint.
        notifyPaneResize(pane, Math.max(2, cols - 1), rows);
      }
      pendingResizes.set(pane.paneId, { pane, cols, rows });
      if (resizeNotifyTimer == null) resizeNotifyTimer = setTimeout(flushResizeNotifications, 60);
      scheduleSettleVerify(pane);
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
      // WebKit never fires `blur` on an element detached while focused, so
      // detaching a focused xterm textarea leaves the terminal's internal
      // focus flag stuck true — its cursor keeps rendering (and blinking) as
      // focused forever. Blur while still attached so the event really fires,
      // and refocus after the reattach so a rebuild is focus-neutral.
      const doc = containerEl.ownerDocument;
      const active = doc ? doc.activeElement : null;
      const activeWasInside = !!(active && containerEl.contains(active)
        && typeof active.blur === 'function');
      if (activeWasInside) active.blur();
      while (containerEl.firstChild) {
        containerEl.removeChild(containerEl.firstChild);
      }
      const rendered = renderTree(tab.treeRoot, (id) => panes.get(id).root);
      containerEl.appendChild(rendered);
      if (activeWasInside && active.isConnected && typeof active.focus === 'function') {
        active.focus();
      }
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
