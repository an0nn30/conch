// Terminal tool window — a real shell in the bottom zone.
//
// This file hosts an xterm and owns its PTY's lifecycle, and nothing else.
// Docking, resizing, hiding and pop-out are the tool window manager's; the
// registration and the boot decision are app/tool-window-runtime.js's.
//
// The PTY path is the SAME one terminal TABS use — spawn_shell / write_to_pty
// / pty-output / pty-exit / close_pty, keyed by a pane id from the window's
// own allocator (crates/termlab_tauri/src/pty.rs's `session_key` is
// "<window label>:<pane id>"). The pane it builds joins the window's shared
// `panes` map for that reason and one more: every window-wide walk over
// panes — the startup theme/font apply in main-runtime.js, the config-changed
// re-apply in config-runtime.js, the pty-output writer in
// window-events-runtime.js — then reaches this terminal for free instead of
// needing a second copy of each. Its `tabId` is null, which is what keeps the
// tab-shaped teardown paths off it: pane-manager's closePane and splitPane
// both bail on `tabs.get(pane.tabId)`, and allPanesInTab walks a tab's split
// tree, which this pane is not in.
//
// Three lifecycle rules, in the order they bite:
//   * The spawn is LAZY. renderFn runs on first activation (the manager's
//     ensureWindowElement), so a window that never opens the panel never
//     starts a shell.
//   * Hiding the panel does NOT touch the shell. The manager hides by setting
//     `display: none` on an element this module does not watch, so there is
//     nothing here to react — deliberately. The shell keeps running.
//   * The shell dies with the WINDOW, not with this module: Tauri's
//     `Destroyed` hook reaps every PTY keyed to the window label
//     (crates/termlab_tauri/src/cleanup.rs). destroy() below is for the
//     narrower case of the tool window being unregistered.
(function initTermLabTerminalPanel(global) {
  'use strict';

  // One live panel per window: the tool window is registered once and its
  // renderFn runs at most once per dock cycle. A module-level handle is what
  // lets the keyboard router ask "does the panel terminal have focus?"
  // without threading an instance out through three runtimes.
  let live = null;

  function el(tag, className) {
    const node = global.document.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  // A panel host window (src/panel_host.rs) has no pane registry, no terminal
  // runtime and no PTY of its own. The Terminal tool window is registered
  // `poppable: false` precisely so a host can never be asked to mount this,
  // but the registration itself is shared verbatim with the host's
  // registrations-only boot — so say what happened rather than throwing out
  // of a renderFn.
  function unavailable(panelEl, message) {
    const note = el('div', 'tl-terminal-panel__unavailable');
    note.textContent = message;
    panelEl.appendChild(note);
    return { destroy() {} };
  }

  function init(options) {
    const opts = options || {};
    const panelEl = opts.panelEl;
    if (!panelEl) throw new Error('Terminal panel requires panelEl');

    const invoke = typeof opts.invoke === 'function' ? opts.invoke : null;
    const initTerminal = typeof opts.initTerminal === 'function' ? opts.initTerminal : null;
    const paneId = opts.paneId;
    if (!invoke || !initTerminal || typeof paneId !== 'number') {
      return unavailable(panelEl, 'The terminal is only available in its own window.');
    }

    const listen = typeof opts.listen === 'function' ? opts.listen : null;
    const cwd = opts.cwd || null;
    const panes = opts.panes && typeof opts.panes.set === 'function' ? opts.panes : null;
    const setupBridge = typeof opts.setupTmuxRightClickBridge === 'function'
      ? opts.setupTmuxRightClickBridge
      : null;
    const createPaneResizeObserver = typeof opts.createPaneResizeObserver === 'function'
      ? opts.createPaneResizeObserver
      : null;
    const fitPane = typeof opts.fitAndResizePane === 'function' ? opts.fitAndResizePane : () => {};

    // ---- DOM ---------------------------------------------------------------
    const root = el('div', 'tl-terminal-panel');
    const surface = el('div', 'tl-terminal-panel__surface');
    // Same stamp terminal panes carry, for the key-debug logging in
    // terminal-runtime.js's custom key handler.
    surface.dataset.paneId = String(paneId);
    root.appendChild(surface);

    const exitEl = el('div', 'tl-terminal-panel__exit');
    exitEl.hidden = true;
    const exitText = el('span', 'tl-terminal-panel__exit-text');
    exitEl.appendChild(exitText);
    const restartBtn = el('button', 'tl-btn tl-terminal-panel__restart');
    restartBtn.type = 'button';
    restartBtn.textContent = 'Restart';
    exitEl.appendChild(restartBtn);
    root.appendChild(exitEl);
    panelEl.appendChild(root);

    const created = initTerminal(surface);
    const term = created ? created.term : null;
    const fitAddon = created ? created.fitAddon : null;
    if (!term) return unavailable(panelEl, 'The terminal could not be created in this window.');

    // ---- the pane ----------------------------------------------------------
    const pane = {
      paneId,
      // Null on purpose — see the file header. Do not "fix" this by inventing
      // a tab id: every tab-shaped path is guarded on it being unresolvable.
      tabId: null,
      kind: 'terminal',
      type: 'local',
      connectionId: null,
      term,
      fitAddon,
      root: surface,
      spawned: false,
      lastCols: 0,
      lastRows: 0,
      cleanupMouseBridge: setupBridge ? setupBridge(term, surface) : null,
      resizeObserver: null,
      debounceTimer: null,
      // Marks this pane as the tool window's rather than a tab's, for any
      // future reader of the shared map that needs to tell them apart.
      toolWindowId: 'terminal',
    };
    if (panes) panes.set(paneId, pane);
    // The zone divider, the bottom-zone resize handle, the window resize and
    // the panel being shown again all move this element; the manager's own
    // fit hook only ever fits the active TAB, so the panel keeps its own.
    if (createPaneResizeObserver) {
      pane.resizeObserver = createPaneResizeObserver(pane, fitPane);
    }

    term.onData((data) => {
      if (!pane.spawned) return;
      invoke('write_to_pty', { paneId, data }).catch((error) => {
        console.error('write_to_pty error:', error);
      });
    });

    // ---- spawn / exit / respawn -------------------------------------------
    let disposed = false;
    let exited = false;
    let spawnInFlight = false;

    function showExitState(message) {
      exited = true;
      pane.spawned = false;
      exitText.textContent = message;
      exitEl.hidden = false;
    }

    function clearExitState() {
      exited = false;
      exitEl.hidden = true;
    }

    function spawn() {
      if (disposed || spawnInFlight) return Promise.resolve();
      spawnInFlight = true;
      const dims = fitAddon && typeof fitAddon.proposeDimensions === 'function'
        ? fitAddon.proposeDimensions()
        : null;
      const cols = dims && dims.cols ? dims.cols : 80;
      const rows = dims && dims.rows ? dims.rows : 24;
      return Promise.resolve(invoke('spawn_shell', { paneId, cols, rows, cwd }))
        .then(() => {
          spawnInFlight = false;
          if (disposed) return;
          pane.spawned = true;
          clearExitState();
          fitPane(pane);
        })
        .catch((error) => {
          spawnInFlight = false;
          if (disposed) return;
          showExitState('The shell could not be started: ' + String(error));
        });
    }

    function restart() {
      if (disposed || !exited) return Promise.resolve();
      if (typeof term.reset === 'function') term.reset();
      return spawn();
    }

    restartBtn.addEventListener('click', (event) => {
      if (event && typeof event.stopPropagation === 'function') event.stopPropagation();
      restart();
    });
    // The whole exit banner is a target, so "click to restart" is literally
    // true. The `exited` guard in restart() makes the bubbled button click a
    // no-op rather than a second spawn.
    exitEl.addEventListener('click', () => { restart(); });

    let unlisten = null;
    if (listen) {
      Promise.resolve(listen('pty-exit', (event) => {
        const payload = (event && event.payload) || {};
        // Rust emits pty-exit with emit_to(window_label), so this listener
        // only ever hears this window's panes; the id check is what separates
        // the panel's pane from the tabs'.
        if (payload.pane_id !== paneId) return;
        showExitState('The shell exited. Click here or press Enter to start a new one.');
      })).then((fn) => {
        if (typeof fn !== 'function') return;
        if (disposed) fn();
        else unlisten = fn;
      }).catch(() => {});
    }

    // ---- focus -------------------------------------------------------------
    surface.addEventListener('mousedown', () => {
      if (typeof term.focus === 'function') term.focus();
    });

    const textarea = term.textarea || null;
    if (textarea && typeof textarea.addEventListener === 'function') {
      textarea.addEventListener('focus', () => root.classList.add('is-focused'));
      textarea.addEventListener('blur', () => root.classList.remove('is-focused'));
    }

    // Read from the live document rather than from a cached flag: the answer
    // has to be right at the instant a keystroke is routed, and WebKit does
    // not always fire blur on a detached-while-focused element.
    function hasFocus() {
      const doc = global.document;
      const active = doc ? doc.activeElement : null;
      if (!active || typeof root.contains !== 'function') return false;
      return root.contains(active);
    }

    function focus() {
      if (typeof term.focus === 'function') term.focus();
    }

    function fit() {
      fitPane(pane);
    }

    function destroy() {
      if (disposed) return;
      disposed = true;
      if (live === api) live = null;
      if (unlisten) {
        try { unlisten(); } catch (_) {}
        unlisten = null;
      }
      if (pane.resizeObserver) pane.resizeObserver.disconnect();
      if (pane.cleanupMouseBridge) pane.cleanupMouseBridge();
      if (panes) panes.delete(paneId);
      if (pane.spawned) invoke('close_pty', { paneId }).catch(() => {});
      pane.spawned = false;
      if (typeof term.dispose === 'function') term.dispose();
    }

    const api = {
      element: root,
      paneId,
      focus,
      fit,
      hasFocus,
      restart,
      isExited: () => exited,
      destroy,
    };
    live = api;

    // First show IS first spawn. The zone is still mid-layout at this point,
    // so proposeDimensions can come back empty; the 80x24 fallback matches
    // tab-manager's createTab, and the rAF below plus the resize observer
    // correct it as soon as the element has a real size.
    spawn();
    if (typeof global.requestAnimationFrame === 'function') {
      global.requestAnimationFrame(() => {
        if (!disposed) fitPane(pane);
      });
    }

    return api;
  }

  global.termlabTerminalPanel = {
    init,
    // Asked by app/shortcut-runtime.js on every core-shortcut hit: an
    // editor-scoped binding (cmd+s and friends) must be DROPPED while this
    // terminal has the keyboard, exactly as it is for a focused terminal tab.
    hasFocus: () => !!(live && live.hasFocus()),
    focus: () => { if (live) live.focus(); },
  };
})(window);
