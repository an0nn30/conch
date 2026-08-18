(function initTermLabTabManager(global) {
  function create(deps) {
    const getTabs = deps.getTabs;
    const getPanes = deps.getPanes;
    const getActiveTabId = deps.getActiveTabId;
    const setActiveTabId = deps.setActiveTabId;
    const getFocusedPaneId = deps.getFocusedPaneId;
    const setFocusedPaneId = deps.setFocusedPaneId;
    const setNextTabLabel = deps.setNextTabLabel;
    const appEl = deps.appEl;
    // The configured terminal font size, which editor panes share. Needed at
    // creation time: without it a new editor view renders at the inherited
    // page size until some later config change happens to reconfigure it.
    const getTermFontSize = deps.getTermFontSize;
    const setFocusedPane = deps.setFocusedPane;
    const fitAndResizeTab = deps.fitAndResizeTab;
    const onTabChanged = deps.onTabChanged;
    const allPanesInTab = deps.allPanesInTab;
    const rememberPluginViewSize = deps.rememberPluginViewSize;
    const unregisterPaneDnd = deps.unregisterPaneDnd;
    const notifyTerminalClosed = deps.notifyTerminalClosed;
    const notifyPluginViewClosed = deps.notifyPluginViewClosed;
    const deletePluginViewPane = deps.deletePluginViewPane;
    const showStatus = deps.showStatus;
    const destroyCurrentWindow = deps.destroyCurrentWindow;
    const allocateTabId = deps.allocateTabId;
    const allocatePaneId = deps.allocatePaneId;
    const allocateTabLabel = deps.allocateTabLabel;
    const tabBarEl = deps.tabBarEl;
    const terminalHostEl = deps.terminalHostEl;
    const initTerminal = deps.initTerminal;
    const setupTmuxRightClickBridge = deps.setupTmuxRightClickBridge;
    const createPaneResizeObserver = deps.createPaneResizeObserver;
    const fitAndResizePane = deps.fitAndResizePane;
    const makeLeaf = deps.makeLeaf;
    const setupDividerDrag = deps.setupDividerDrag;
    const normalizeTabTitle = deps.normalizeTabTitle;
    const onTerminalData = deps.onTerminalData;
    const spawnShell = deps.spawnShell;
    const spawnDefaultShell = deps.spawnDefaultShell;
    const onSshData = deps.onSshData;
    const connectSsh = deps.connectSsh;
    const ensureVaultUnlocked = deps.ensureVaultUnlocked;
    const getCurrentWindowLabel = deps.getCurrentWindowLabel;
    const refreshSshSessions = deps.refreshSshSessions;
    const setWindowTitle = deps.setWindowTitle;
    const getLocalPaneCwd = deps.getLocalPaneCwd;
    const getLocalPaneProcess = deps.getLocalPaneProcess;
    const getHostIdentity = deps.getHostIdentity;
    const getWorkspaceDir = deps.getWorkspaceDir;

    // Window title = "<workspace basename> – <active tab title>", falling
    // back to "TermLab" when there is no active tab.
    //
    // The workspace basename is intentionally static for the window's
    // lifetime, matching the reference app's project-name semantics (a fixed
    // label, not something that tracks the shell's current directory as the
    // user `cd`s around). It is resolved once, preferring the backend's
    // `get_workspace_dir` command (the app's launch cwd, captured once in
    // Rust — see `commands::get_workspace_dir`); if that's unavailable or
    // returns nothing, it falls back to the first local tab's initial pane
    // cwd. Do not "fix" this into live cwd tracking.
    let workspaceBasename = null;
    let workspaceFromLaunch = false; // true once the authoritative launch-cwd source has resolved
    let workspaceFallbackAttempted = false;

    function basenameOf(path) {
      if (!path) return null;
      const trimmed = String(path).replace(/[\\/]+$/, '');
      const parts = trimmed.split(/[\\/]/);
      const last = parts[parts.length - 1];
      return last || trimmed;
    }

    // ---- Tab titles -------------------------------------------------------
    // Composed by app/core/pane-title.js from a snapshot of the pane's state:
    // "dustin@mbp: ~/projects/conch — 120×30". The pieces arrive from three
    // places — user/host once at startup, cwd/foreground program from a poll on
    // the active tab, and the size from the pane's own fit — so each is cached
    // on the tab and the title is recomposed whenever any of them moves.
    let hostIdentity = { user: '', host: '' };
    let homeDir = '';

    function composeFor(tab) {
      const composer = window.termlabPaneTitle;
      if (!composer || !tab) return null;
      const pane = tab.focusedPaneId != null ? getPanes().get(tab.focusedPaneId) : null;
      return composer.composeTitle({
        user: hostIdentity.user,
        host: hostIdentity.host,
        home: homeDir,
        cwd: tab.titleCwd,
        program: tab.titleProgram,
        oscTitle: tab.oscTitle,
        cols: pane ? pane.lastCols : 0,
        rows: pane ? pane.lastRows : 0,
      });
    }

    // A composed title describes a terminal — a user@host prefix, a cwd or
    // foreground program, a PTY size. A tab whose focused pane is not a
    // terminal (an editor tab, whose label is its filename; a docked plugin
    // view) has none of that, and composing one destroys the label it does
    // have.
    function tabFocusesTerminalPane(tab) {
      if (!tab || tab.focusedPaneId == null) return false;
      const pane = getPanes().get(tab.focusedPaneId);
      return !!(pane && pane.kind === 'terminal');
    }

    // A plugin-supplied name is the user's own label and always wins.
    function refreshTabTitle(tab) {
      if (!tab || tab.pluginRenamed) return;
      // Guarded here as well as at the poll below: this is the function that
      // actually overwrites tab.label, the button and the window title, and it
      // has several callers.
      if (!tabFocusesTerminalPane(tab)) return;
      const title = composeFor(tab);
      if (!title || title === tab.label) return;
      tab.label = title;
      setTabLabel(tab.button, title);
      tab.button.title = title;
      if (tab === currentTab()) updateWindowTitle();
    }

    async function pollActiveTabProcess() {
      const tab = currentTab();
      if (!tab || typeof getLocalPaneProcess !== 'function') return;
      const paneId = tab.focusedPaneId;
      if (paneId == null) return;
      // Only a terminal pane has a process to poll. `get_local_pane_process`
      // (crates/termlab_tauri/src/pty.rs) returns
      // `PaneProcessInfo { cwd: None, program: None }` for a pane id it does
      // not know — it never errors — so without this bail-out the reply is
      // truthy, the cached cwd/program are blanked, and refreshTabTitle
      // recomposes an editor tab's filename into a bare "user@host".
      if (!tabFocusesTerminalPane(tab)) return;
      try {
        const info = await getLocalPaneProcess(paneId);
        if (!info) return;
        // Only the active tab is polled; a tab switch re-polls immediately, so
        // a stale reply for a tab that is no longer active is discarded.
        if (currentTab() !== tab) return;
        tab.titleCwd = info.cwd || '';
        tab.titleProgram = info.program || '';
        refreshTabTitle(tab);
      } catch (_) {
        // A pane that has exited, or a platform without process introspection
        // (Windows): keep whatever the title already says.
      }
    }

    function updateWindowTitle() {
      if (typeof setWindowTitle !== 'function') return;
      const tab = currentTab();
      let title;
      if (!tab) {
        title = 'TermLab';
      } else {
        // The window title mirrors the active tab exactly. It used to prefix
        // the workspace basename; the tab title now carries user@host and the
        // path itself, so the prefix was duplicating what follows it.
        title = getTabLabel(tab.button) || tab.label || 'Terminal';
      }
      try {
        Promise.resolve(setWindowTitle(title)).catch(() => {});
      } catch (_) {
        // A missing/denied Tauri window API must never break tab switching.
      }
      // In zen mode the OS titlebar is gone, so the drag strip shows the same
      // string. Harmless when not in zen: the element is hidden.
      const dragHandle = document.getElementById('drag-handle');
      if (dragHandle) dragHandle.textContent = title === 'TermLab' ? '' : title;
    }

    async function resolveWorkspaceFromLaunch() {
      if (typeof getWorkspaceDir !== 'function') return;
      try {
        const dir = await getWorkspaceDir();
        if (dir) {
          workspaceBasename = basenameOf(dir);
          workspaceFromLaunch = true;
          updateWindowTitle();
        }
      } catch (_) {
        // Leave unresolved; the pane-cwd fallback (or plain tab title) still applies.
      }
    }

    async function resolveWorkspaceBasenameFallback(paneId) {
      if (workspaceFromLaunch || workspaceFallbackAttempted || typeof getLocalPaneCwd !== 'function') return;
      workspaceFallbackAttempted = true;
      try {
        const cwd = await getLocalPaneCwd(paneId);
        // Re-check workspaceFromLaunch: the authoritative launch-cwd lookup
        // may have resolved while this fallback was in flight, and it wins.
        if (cwd && !workspaceFromLaunch) {
          workspaceBasename = basenameOf(cwd);
          updateWindowTitle();
        }
      } catch (_) {
        // Leave workspaceBasename unset; title still falls back to the tab title alone.
      }
    }

    // Kick off the authoritative (launch-cwd) resolution immediately; the
    // per-tab fallback below only fires if this hasn't produced a value yet.
    resolveWorkspaceFromLaunch();

    // user/host/home never change while the app runs, so they are fetched once
    // rather than on every poll tick.
    if (typeof getHostIdentity === 'function') {
      Promise.resolve(getHostIdentity()).then((id) => {
        if (!id) return;
        hostIdentity = { user: id.user || '', host: id.host || '' };
        homeDir = id.home || '';
        const tab = currentTab();
        if (tab) refreshTabTitle(tab);
      }).catch(() => {});
    }

    // Poll only the active tab: a background tab's cwd is not on screen, and
    // the cost of this is one IPC round trip per second regardless of how many
    // tabs are open. TITLE_POLL_MS is slow enough to be free and fast enough
    // that a `cd` shows up before the user looks away.
    const TITLE_POLL_MS = 1000;
    setInterval(pollActiveTabProcess, TITLE_POLL_MS);

    function makeTabButton(label, onClose) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'tab-btn';
      if (global.tlIcon) {
        button.appendChild(global.tlIcon.create('terminal', { size: 16, alt: '' }));
      }
      const labelSpan = document.createElement('span');
      labelSpan.className = 'tab-btn-label';
      labelSpan.textContent = label;
      const closeBtn = document.createElement('span');
      closeBtn.className = 'tab-btn-close';
      closeBtn.textContent = '\u2715';
      closeBtn.addEventListener('click', (event) => {
        event.stopPropagation();
        onClose();
      });
      button.appendChild(labelSpan);
      button.appendChild(closeBtn);
      button._labelSpan = labelSpan;
      return button;
    }

    function setTabLabel(button, text) {
      if (button._labelSpan) button._labelSpan.textContent = text;
      else button.textContent = text;
    }

    function getTabLabel(button) {
      return button._labelSpan ? button._labelSpan.textContent : button.textContent;
    }

    function startTabRename(tabId) {
      const tabs = getTabs();
      const panes = getPanes();
      const tab = tabs.get(tabId);
      if (!tab || !tab.button || !tab.button._labelSpan) return;

      const labelSpan = tab.button._labelSpan;
      const currentText = labelSpan.textContent;

      const input = document.createElement('input');
      input.type = 'text';
      input.value = currentText;
      input.className = 'tab-rename-input';
      input.style.cssText = 'width:100%; border:none; outline:none; background:transparent; color:inherit; font:inherit; padding:0; margin:0;';

      labelSpan.textContent = '';
      labelSpan.appendChild(input);
      input.focus();
      input.select();

      function refocusTabTerminal() {
        const pane = panes.get(tab.focusedPaneId);
        if (pane && pane.term) pane.term.focus();
      }

      function commit() {
        const newName = input.value.trim();
        if (input.parentNode) {
          labelSpan.removeChild(input);
        }
        if (newName && newName !== currentText) {
          labelSpan.textContent = newName;
          tab.label = newName;
          tab.hasCustomTitle = true;
          tab.button.title = newName;
          updateWindowTitle();
        } else {
          labelSpan.textContent = currentText;
        }
        refocusTabTerminal();
      }

      input.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          commit();
        } else if (event.key === 'Escape') {
          event.preventDefault();
          labelSpan.textContent = currentText;
          if (input.parentNode) labelSpan.removeChild(input);
          refocusTabTerminal();
        }
        event.stopPropagation();
      }, true);

      input.addEventListener('blur', () => {
        setTimeout(() => {
          if (input.parentNode) commit();
        }, 50);
      });
    }

    function renameActiveTab() {
      const activeTabId = getActiveTabId();
      if (activeTabId == null) return;
      startTabRename(activeTabId);
    }

    function currentTab() {
      const tabs = getTabs();
      const activeTabId = getActiveTabId();
      return activeTabId === null ? null : tabs.get(activeTabId) || null;
    }

    function updateTabBarVisibility() {
      const tabs = getTabs();
      appEl.classList.toggle('tabs-visible', tabs.size > 1);
    }

    function renumberTabs() {
      const tabs = getTabs();
      let n = 1;
      for (const tab of tabs.values()) {
        const newLabel = 'Terminal';
        tab.label = newLabel;
        if (!tab.hasCustomTitle) {
          setTabLabel(tab.button, newLabel);
          tab.button.title = newLabel;
        }
        n++;
      }
      setNextTabLabel(n);
    }

    function activateTab(tabId) {
      const tabs = getTabs();
      const panes = getPanes();
      if (!tabs.has(tabId)) return;

      setActiveTabId(tabId);
      // Re-poll straight away rather than letting the newly active tab show a
      // title up to a full poll interval stale.
      pollActiveTabProcess();
      if (global) {
        global.__termlabActiveTabId = tabId;
        try {
          global.dispatchEvent(new CustomEvent('termlab-active-tab-changed', { detail: { tabId } }));
        } catch (_) {}
      }
      for (const tab of tabs.values()) {
        const active = tab.id === tabId;
        tab.button.classList.toggle('active', active);
        tab.containerEl.classList.toggle('active', active);
      }

      const tab = tabs.get(tabId);
      if (tab.focusedPaneId != null) {
        setFocusedPane(tab.focusedPaneId);
      }
      fitAndResizeTab(tab);

      const pane = panes.get(tab.focusedPaneId);
      onTabChanged(pane || tab);
      updateWindowTitle();
    }

    async function closeTab(tabId, options = {}) {
      const tabs = getTabs();
      const panes = getPanes();
      const notifyBackend = options.notifyBackend !== false;
      const closeWindowWhenLast = options.closeWindowWhenLast !== false;
      const tab = tabs.get(tabId);
      if (!tab) return;
      let closedSshPane = false;

      // Ask before discarding edits. Skipped when the caller is a close that
      // already asked (the window-close and quit handshakes), which passes
      // skipDirtyCheck. Everything below this point destroys CodeMirror views
      // and deletes panes, so the question has to be settled first.
      if (!options.skipDirtyCheck) {
        const dirtyPanes = allPanesInTab(tabId)
          .map((pid) => panes.get(pid))
          .filter((p) => p && p.kind === 'editor' && p.dirty);
        if (dirtyPanes.length > 0) {
          const service = global.termlabEditorService;
          if (!service || typeof service.confirmDirtyPanes !== 'function') {
            // A dirty editor exists but the service that knows how to ask
            // about it does not. Keep the tab.
            showStatus('Cannot confirm unsaved changes; tab not closed.');
            return;
          }
          const ok = await service.confirmDirtyPanes(dirtyPanes);
          if (!ok) return;
          // Awaiting the prompt yielded to the event loop; the tab may have
          // been closed underneath us in the meantime.
          if (!tabs.has(tabId)) return;
        }
      }

      const paneIds = allPanesInTab(tabId);
      for (const pid of paneIds) {
        const pane = panes.get(pid);
        if (!pane) continue;
        unregisterPaneDnd(pid);
        if (pane.kind === 'plugin_view') rememberPluginViewSize(pane);
        if (notifyBackend && pane.kind === 'terminal' && pane.spawned) {
          if (pane.type === 'ssh') closedSshPane = true;
          notifyTerminalClosed(pid, pane.type);
        } else if (notifyBackend && pane.kind === 'plugin_view' && pane.viewId) {
          notifyPluginViewClosed(pane.viewId);
          deletePluginViewPane(pane.viewId);
        } else if (pane.kind === 'editor') {
          // Unconditional, unlike the two branches above: destroying the
          // CodeMirror view is local cleanup, not a backend notification, so it
          // must happen even when notifyBackend is false.
          if (pane.view && global.termlabEditorPane) {
            global.termlabEditorPane.destroyEditorView(pane.view);
          }
          pane.view = null;
          // A remote editor's file is a download in a temp directory, so
          // closing the tab is the end of its life. The dirty prompt above has
          // already run, so reaching here means the edit was either uploaded
          // or deliberately discarded. The service owns the path rules; it
          // refuses anything that is not one of its own temp files.
          if (pane.remote && global.termlabEditorService
              && typeof global.termlabEditorService.discardRemoteTemp === 'function') {
            global.termlabEditorService.discardRemoteTemp(pane);
          }
        }
        if (pane.cleanupMouseBridge) pane.cleanupMouseBridge();
        if (pane.resizeObserver) pane.resizeObserver.disconnect();
        if (pane.term) pane.term.dispose();
        panes.delete(pid);
      }

      tabs.delete(tabId);
      tab.button.remove();
      tab.containerEl.remove();
      renumberTabs();
      updateTabBarVisibility();

      if (getActiveTabId() === tabId) {
        setActiveTabId(null);
        if (getFocusedPaneId() != null) {
          setFocusedPaneId(null);
        }
        const next = tabs.values().next();
        if (!next.done) {
          activateTab(next.value.id);
        }
      }

      updateWindowTitle();

      if (tabs.size === 0 && closeWindowWhenLast) {
        try {
          await destroyCurrentWindow();
        } catch (error) {
          showStatus('Failed to close window: ' + String(error));
        }
      }

      if (closedSshPane && typeof refreshSshSessions === 'function') {
        refreshSshSessions();
        setTimeout(() => {
          refreshSshSessions();
        }, 150);
      }
    }

    async function createTab(options = {}) {
      const tabs = getTabs();
      const panes = getPanes();
      const tabId = allocateTabId();
      const paneId = allocatePaneId();
      const label = allocateTabLabel();

      const button = makeTabButton(label, () => closeTab(tabId));
      button.dataset.tabId = String(tabId);
      button.classList.add('entering');

      const containerEl = document.createElement('div');
      containerEl.className = 'tab-tree-root';

      const paneEl = document.createElement('div');
      paneEl.className = 'terminal-pane';
      paneEl.dataset.paneId = paneId;
      containerEl.appendChild(paneEl);

      tabBarEl.appendChild(button);
      terminalHostEl.appendChild(containerEl);

      const { term, fitAddon } = initTerminal(paneEl);

      const pane = {
        paneId,
        tabId,
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
      panes.set(paneId, pane);
      pane.resizeObserver = createPaneResizeObserver(pane, fitAndResizePane);

      const tab = {
        id: tabId,
        label,
        type: 'local',
        hasCustomTitle: false,
        button,
        containerEl,
        treeRoot: makeLeaf(paneId),
        focusedPaneId: paneId,
      };
      tabs.set(tabId, tab);
      setupDividerDrag(
        containerEl,
        () => tab.treeRoot,
        (newTree) => { tab.treeRoot = newTree; },
      );
      updateTabBarVisibility();

      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          button.classList.remove('entering');
        });
      });

      button.addEventListener('click', () => activateTab(tabId));
      term.onTitleChange((title) => {
        if (tab.pluginRenamed) return;
        // An explicit title from the shell or a program is a deliberate
        // statement about what the pane is doing, so it becomes the body of the
        // composed title (which still appends the size) rather than replacing
        // the whole label.
        tab.oscTitle = normalizeTabTitle(title, '');
        tab.hasCustomTitle = true;
        refreshTabTitle(tab);
      });
      term.onData((data) => {
        if (!pane.spawned) return;
        onTerminalData(pane, paneId, data);
      });

      paneEl.addEventListener('mousedown', () => setFocusedPane(paneId));

      activateTab(tabId);
      const dims = fitAddon.proposeDimensions();
      const cols = dims ? dims.cols : 80;
      const rows = dims ? dims.rows : 24;

      try {
        if (options && options.plainShell) {
          await spawnDefaultShell(paneId, cols, rows);
        } else {
          await spawnShell(paneId, cols, rows);
        }
        pane.spawned = true;
        fitAndResizePane(pane);
        resolveWorkspaceBasenameFallback(paneId);
      } catch (error) {
        term.writeln('\x1b[31mFailed to spawn shell: ' + error + '\x1b[0m');
        await closeTab(tabId, { notifyBackend: false, closeWindowWhenLast: false });
        return null;
      }
      return tabId;
    }

    // An editor tab. Mirrors createTab's DOM and tab bookkeeping exactly —
    // same button, same tree-root container, same divider wiring — so editor
    // tabs participate in splits, drag-and-drop and activation with no
    // special cases downstream. The only difference is what lives in the pane.
    function createEditorTab(options) {
      const opts = options || {};
      const tabs = getTabs();
      const panes = getPanes();
      const tabId = allocateTabId();
      const paneId = allocatePaneId();
      const fileName = String(opts.filePath || 'untitled').split('/').pop();

      const button = makeTabButton(fileName, () => closeTab(tabId));
      button.dataset.tabId = String(tabId);
      button.classList.add('entering');

      const containerEl = document.createElement('div');
      containerEl.className = 'tab-tree-root';

      const paneEl = document.createElement('div');
      paneEl.className = 'terminal-pane';
      paneEl.dataset.paneId = paneId;
      containerEl.appendChild(paneEl);

      const hostEl = document.createElement('div');
      hostEl.className = 'editor-pane-host';
      paneEl.appendChild(hostEl);

      tabBarEl.appendChild(button);
      terminalHostEl.appendChild(containerEl);

      const pane = {
        paneId,
        tabId,
        kind: 'editor',
        type: null,
        connectionId: null,
        term: null,
        fitAddon: null,
        root: paneEl,
        spawned: false,
        lastCols: 0,
        lastRows: 0,
        cleanupMouseBridge: null,
        resizeObserver: null,
        debounceTimer: null,
        filePath: opts.filePath || null,
        view: null,
        dirty: false,
        remote: opts.remote || null,
      };
      panes.set(paneId, pane);

      const tab = {
        id: tabId,
        label: fileName,
        type: 'editor',
        hasCustomTitle: true,
        button,
        containerEl,
        treeRoot: makeLeaf(paneId),
        focusedPaneId: paneId,
      };
      tabs.set(tabId, tab);
      setupDividerDrag(
        containerEl,
        () => tab.treeRoot,
        (newTree) => { tab.treeRoot = newTree; },
      );
      updateTabBarVisibility();

      requestAnimationFrame(() => {
        requestAnimationFrame(() => button.classList.remove('entering'));
      });

      button.addEventListener('click', () => activateTab(tabId));
      paneEl.addEventListener('mousedown', () => setFocusedPane(paneId));

      // The modified marker is its own element, inserted between the tab's
      // label span and its close affordance. Do NOT write button.textContent —
      // makeTabButton builds child elements (icon, label span, close span) and
      // assigning textContent destroys all of them.
      const dirtyMarker = document.createElement('span');
      dirtyMarker.className = 'tab-dirty-marker';
      dirtyMarker.textContent = '•';
      dirtyMarker.hidden = true;
      if (button._labelSpan && button._labelSpan.parentNode === button) {
        button.insertBefore(dirtyMarker, button._labelSpan.nextSibling);
      } else {
        button.appendChild(dirtyMarker);
      }

      pane.view = global.termlabEditorPane.createEditorView(hostEl, {
        doc: typeof opts.contents === 'string' ? opts.contents : '',
        filename: pane.filePath || '',
        onDirtyChange: (dirty) => {
          pane.dirty = dirty;
          dirtyMarker.hidden = !dirty;
        },
      });

      // createEditorView starts with an empty font compartment, so the view
      // would inherit the page font size and only snap to the configured one
      // on the next config-changed event. Apply it now.
      const fontSize = typeof getTermFontSize === 'function' ? getTermFontSize() : 0;
      if (pane.view && fontSize > 0) {
        global.termlabEditorPane.setFontSize(pane.view, fontSize);
      }

      activateTab(tabId);
      setFocusedPane(paneId);
      return tabId;
    }

    async function createSshTab(opts) {
      const tabs = getTabs();
      const panes = getPanes();
      const tabId = allocateTabId();
      const paneId = allocatePaneId();
      const label = opts.spec || 'SSH';

      const button = makeTabButton(label, () => closeTab(tabId));
      button.dataset.tabId = String(tabId);
      button.classList.add('entering');

      const containerEl = document.createElement('div');
      containerEl.className = 'tab-tree-root';

      const paneEl = document.createElement('div');
      paneEl.className = 'terminal-pane';
      paneEl.dataset.paneId = paneId;
      containerEl.appendChild(paneEl);

      tabBarEl.appendChild(button);
      terminalHostEl.appendChild(containerEl);

      const { term, fitAddon } = initTerminal(paneEl);

      const pane = {
        paneId,
        tabId,
        kind: 'terminal',
        type: 'ssh',
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
      panes.set(paneId, pane);
      pane.resizeObserver = createPaneResizeObserver(pane, fitAndResizePane);

      const tab = {
        id: tabId,
        label,
        type: 'ssh',
        hasCustomTitle: false,
        button,
        containerEl,
        treeRoot: makeLeaf(paneId),
        focusedPaneId: paneId,
      };
      tabs.set(tabId, tab);
      setupDividerDrag(
        containerEl,
        () => tab.treeRoot,
        (newTree) => { tab.treeRoot = newTree; },
      );
      updateTabBarVisibility();

      requestAnimationFrame(() => {
        requestAnimationFrame(() => button.classList.remove('entering'));
      });

      button.addEventListener('click', () => activateTab(tabId));
      term.onTitleChange((title) => {
        if (tab.pluginRenamed) return;
        const tabTitle = normalizeTabTitle(title, tab.label);
        tab.hasCustomTitle = true;
        setTabLabel(tab.button, tabTitle);
        tab.button.title = tabTitle;
        updateWindowTitle();
      });
      term.onData((data) => {
        if (!pane.spawned) return;
        onSshData(pane, paneId, data);
      });

      paneEl.addEventListener('mousedown', () => setFocusedPane(paneId));

      activateTab(tabId);
      const dims = fitAddon.proposeDimensions();
      const cols = dims ? dims.cols : 80;
      const rows = dims ? dims.rows : 24;

      try {
        const doConnect = async () => connectSsh(opts, paneId, cols, rows);

        try {
          await doConnect();
        } catch (error) {
          if (String(error).includes('VAULT_LOCKED')) {
            await ensureVaultUnlocked(doConnect);
          } else {
            throw error;
          }
        }

        pane.spawned = true;
        pane.connectionId = 'conn:' + getCurrentWindowLabel() + ':' + paneId;
        tab.label = getTabLabel(button) || label;
        fitAndResizePane(pane);
        refreshSshSessions();
        onTabChanged(pane);
      } catch (error) {
        term.writeln('\x1b[31mSSH connection failed: ' + error + '\x1b[0m');
        term.writeln('\x1b[90mPress any key to close this tab.\x1b[0m');
        term.onData(() => {
          closeTab(tabId, { notifyBackend: false });
        });
      }
    }

    return {
      currentTab,
      updateTabBarVisibility,
      renumberTabs,
      activateTab,
      closeTab,
      createTab,
      createEditorTab,
      createSshTab,
      makeTabButton,
      setTabLabel,
      getTabLabel,
      renameActiveTab,
      startTabRename,
      // Exposed so callers outside this module that mutate a tab's label
      // directly (e.g. plugin-driven renames in tool-window-runtime.js) can
      // resync the window title afterward.
      refreshWindowTitle: updateWindowTitle,
    };
  }

  global.termlabTabManager = {
    create,
  };
})(window);
