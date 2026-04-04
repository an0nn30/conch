(function initConchTabManager(global) {
  function create(deps) {
    const getTabs = deps.getTabs;
    const getPanes = deps.getPanes;
    const getActiveTabId = deps.getActiveTabId;
    const setActiveTabId = deps.setActiveTabId;
    const getFocusedPaneId = deps.getFocusedPaneId;
    const setFocusedPaneId = deps.setFocusedPaneId;
    const setNextTabLabel = deps.setNextTabLabel;
    const appEl = deps.appEl;
    const setFocusedPane = deps.setFocusedPane;
    const fitAndResizeTab = deps.fitAndResizeTab;
    const onTabChanged = deps.onTabChanged;
    const allPanesInTab = deps.allPanesInTab;
    const unregisterPaneDnd = deps.unregisterPaneDnd;
    const notifyTerminalClosed = deps.notifyTerminalClosed;
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

    function makeTabButton(label, onClose) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'tab-btn';
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
        const newLabel = 'Tab ' + n;
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
    }

    async function closeTab(tabId, options = {}) {
      const tabs = getTabs();
      const panes = getPanes();
      const notifyBackend = options.notifyBackend !== false;
      const closeWindowWhenLast = options.closeWindowWhenLast !== false;
      const tab = tabs.get(tabId);
      if (!tab) return;

      const paneIds = allPanesInTab(tabId);
      for (const pid of paneIds) {
        const pane = panes.get(pid);
        if (!pane) continue;
        unregisterPaneDnd(pid);
        if (notifyBackend && pane.kind === 'terminal' && pane.spawned) {
          notifyTerminalClosed(pid, pane.type);
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

      if (tabs.size === 0 && closeWindowWhenLast) {
        try {
          await destroyCurrentWindow();
        } catch (error) {
          showStatus('Failed to close window: ' + String(error));
        }
      }
    }

    async function createTab(options = {}) {
      const tabs = getTabs();
      const panes = getPanes();
      const tabId = allocateTabId();
      const paneId = allocatePaneId();
      const label = allocateTabLabel();

      const button = makeTabButton(label, () => closeTab(tabId));
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
        const tabTitle = normalizeTabTitle(title, tab.label);
        tab.hasCustomTitle = true;
        setTabLabel(tab.button, tabTitle);
        tab.button.title = tabTitle;
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
      } catch (error) {
        term.writeln('\x1b[31mFailed to spawn shell: ' + error + '\x1b[0m');
        await closeTab(tabId, { notifyBackend: false, closeWindowWhenLast: false });
        return null;
      }
      return tabId;
    }

    async function createSshTab(opts) {
      const tabs = getTabs();
      const panes = getPanes();
      const tabId = allocateTabId();
      const paneId = allocatePaneId();
      const label = opts.spec || 'SSH';

      const button = makeTabButton(label, () => closeTab(tabId));
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
        const tabTitle = normalizeTabTitle(title, tab.label);
        tab.hasCustomTitle = true;
        setTabLabel(tab.button, tabTitle);
        tab.button.title = tabTitle;
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
      createSshTab,
      makeTabButton,
      setTabLabel,
      getTabLabel,
      renameActiveTab,
      startTabRename,
    };
  }

  global.conchTabManager = {
    create,
  };
})(window);
