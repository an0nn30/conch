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
    const rebuildTreeDOM = deps.rebuildTreeDOM;
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
          if (tab.type === 'tmux' && tab.tmuxWindowId != null && global.backendRouter) {
            global.backendRouter.renameTab(tab.tmuxWindowId, newName).catch((error) => {
              showStatus('Failed to rename tmux window: ' + String(error));
            });
          }
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

    function buildTmuxTree(paneSpecs) {
      if (!paneSpecs || paneSpecs.length === 0 || !global.splitTree) return null;

      function clampRatio(value) {
        if (!Number.isFinite(value)) return 0.5;
        return Math.max(0.15, Math.min(0.85, value));
      }

      function normalizeSpec(spec) {
        const left = Number(spec.left);
        const top = Number(spec.top);
        const width = Number(spec.width) || 80;
        const height = Number(spec.height) || 24;
        return {
          paneId: spec.paneId,
          left: Number.isFinite(left) ? left : 0,
          top: Number.isFinite(top) ? top : 0,
          width,
          height,
          right: (Number.isFinite(left) ? left : 0) + width,
          bottom: (Number.isFinite(top) ? top : 0) + height,
        };
      }

      function findAxisSplit(specs, axis) {
        if (!specs || specs.length < 2) return null;

        const startKey = axis === 'x' ? 'left' : 'top';
        const endKey = axis === 'x' ? 'right' : 'bottom';
        const direction = axis === 'x' ? 'vertical' : 'horizontal';
        const starts = specs.map((spec) => spec[startKey]);
        const ends = specs.map((spec) => spec[endKey]);
        const minStart = Math.min.apply(null, starts);
        const maxEnd = Math.max.apply(null, ends);
        const totalSpan = Math.max(1, maxEnd - minStart);
        const candidates = Array.from(new Set(ends)).filter((edge) => edge > minStart && edge < maxEnd).sort((a, b) => a - b);

        let best = null;
        for (const cut of candidates) {
          const first = [];
          const second = [];
          let valid = true;
          for (const spec of specs) {
            const start = spec[startKey];
            const end = spec[endKey];
            if (end <= cut) {
              first.push(spec);
            } else if (start >= cut) {
              second.push(spec);
            } else {
              valid = false;
              break;
            }
          }
          if (!valid || first.length === 0 || second.length === 0) continue;

          const firstMaxEnd = Math.max.apply(null, first.map((spec) => spec[endKey]));
          const secondMinStart = Math.min.apply(null, second.map((spec) => spec[startKey]));
          const gap = secondMinStart - firstMaxEnd;
          const ratio = clampRatio((firstMaxEnd - minStart) / totalSpan);
          const balancePenalty = Math.abs(first.length - second.length) * 0.01;
          const score = gap - balancePenalty;
          if (!best || score > best.score) {
            best = { direction, first, second, ratio, score };
          }
        }
        return best;
      }

      function buildFromSpecs(specs) {
        if (!specs || specs.length === 0) return null;
        if (specs.length === 1) return makeLeaf(specs[0].paneId);

        const verticalSplit = findAxisSplit(specs, 'x');
        const horizontalSplit = findAxisSplit(specs, 'y');
        const chosen = (!verticalSplit && !horizontalSplit)
          ? null
          : (!horizontalSplit
            ? verticalSplit
            : (!verticalSplit ? horizontalSplit : (verticalSplit.score >= horizontalSplit.score ? verticalSplit : horizontalSplit)));

        if (!chosen) {
          let tree = makeLeaf(specs[0].paneId);
          for (let i = 1; i < specs.length; i++) {
            const spec = specs[i];
            const direction = spec.width >= spec.height ? 'vertical' : 'horizontal';
            tree = global.splitTree.makeSplit(direction, 0.5, [tree, makeLeaf(spec.paneId)]);
          }
          return tree;
        }

        const first = chosen.first.slice().sort((a, b) => (chosen.direction === 'vertical' ? a.left - b.left : a.top - b.top));
        const second = chosen.second.slice().sort((a, b) => (chosen.direction === 'vertical' ? a.left - b.left : a.top - b.top));
        const firstTree = buildFromSpecs(first);
        const secondTree = buildFromSpecs(second);
        return global.splitTree.makeSplit(chosen.direction, chosen.ratio, [firstTree, secondTree]);
      }

      return buildFromSpecs(paneSpecs.map(normalizeSpec));
    }

    function parseTmuxNumericId(value) {
      var numeric = Number(value);
      if (Number.isFinite(numeric)) return numeric;
      if (typeof value === 'string') {
        var normalized = value.replace(/^[%@]/, '');
        numeric = Number(normalized);
        if (Number.isFinite(numeric)) return numeric;
      }
      return null;
    }

    function writeTmuxPaneSnapshot(pane, content) {
      if (!pane || !pane.term) return;
      if (pane._tmuxLiveActive) return;
      var snapshot = typeof content === 'string' ? content : '';
      if (!snapshot && pane._lastTmuxSnapshot) return;
      if (pane._lastTmuxSnapshot === snapshot) return;
      if (pane._tmuxSnapshotTimer) {
        clearTimeout(pane._tmuxSnapshotTimer);
      }
      // Delay snapshot hydration slightly so that live tmux output can win when available.
      pane._tmuxSnapshotTimer = setTimeout(function () {
        pane._tmuxSnapshotTimer = null;
        if (pane._tmuxLiveActive || !pane.term) return;
        if (pane._lastTmuxSnapshot === snapshot) return;
        pane.term.reset();
        if (snapshot) {
          pane.term.write(snapshot);
        }
        pane._lastTmuxSnapshot = snapshot;
        pane._tmuxSnapshotAppliedAt = Date.now();
      }, 180);
    }

    function syncExistingTmuxTab(tab, paneSpecs) {
      const panes = getPanes();
      const paneByTmuxId = new Map();
      const paneIds = allPanesInTab(tab.id);
      for (const paneId of paneIds) {
        const pane = panes.get(paneId);
        if (!pane) continue;
        const tmuxPaneId = pane.tmuxPaneId != null
          ? pane.tmuxPaneId
          : (global.tmuxIdMap ? global.tmuxIdMap.getTmuxForPane(paneId) : null);
        if (tmuxPaneId != null) {
          paneByTmuxId.set(Number(tmuxPaneId), pane);
        }
      }

      for (const spec of paneSpecs) {
        const tmuxPaneId = parseTmuxNumericId(spec.id);
        if (tmuxPaneId == null) continue;
        const pane = paneByTmuxId.get(tmuxPaneId);
        if (!pane) continue;
        pane.tmuxPaneId = tmuxPaneId;
        writeTmuxPaneSnapshot(pane, spec.content);
        if (global.tmuxIdMap) {
          global.tmuxIdMap.addPane(tmuxPaneId, pane.paneId);
        }
        if (spec.active) {
          tab.focusedPaneId = pane.paneId;
        }
      }
    }

    function disposePaneLocal(paneId, options) {
      const panes = getPanes();
      const pane = panes.get(paneId);
      if (!pane) return;
      const opts = options || {};
      unregisterPaneDnd(paneId);
      if (global.tmuxIdMap) {
        global.tmuxIdMap.removePaneByFrontend(paneId);
      }
      if (pane.cleanupMouseBridge) pane.cleanupMouseBridge();
      if (pane.resizeObserver) pane.resizeObserver.disconnect();
      if (pane._tmuxSnapshotTimer) {
        clearTimeout(pane._tmuxSnapshotTimer);
        pane._tmuxSnapshotTimer = null;
      }
      if (pane.term && !opts.keepTerminal) pane.term.dispose();
      if (pane.root && pane.root.parentNode) pane.root.parentNode.removeChild(pane.root);
      panes.delete(paneId);
    }

    function updateTabBarVisibility() {
      const tabs = getTabs();
      appEl.classList.toggle('tabs-visible', tabs.size > 1);
    }

    function scheduleTabResize(tab) {
      if (!tab) return;
      if (tab._resizeRaf) return;
      tab._resizeRaf = requestAnimationFrame(() => {
        tab._resizeRaf = null;
        fitAndResizeTab(tab);
      });
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
      if (pane && pane.kind === 'terminal' && pane.term) {
        pane.term.focus();
      }
      onTabChanged(pane || tab);
    }

    async function closeTab(tabId, options = {}) {
      const tabs = getTabs();
      const panes = getPanes();
      const notifyBackend = options.notifyBackend !== false;
      const closeWindowWhenLast = options.closeWindowWhenLast !== false;
      const tab = tabs.get(tabId);
      if (!tab) return;

      if (notifyBackend && tab.type === 'tmux' && tab.tmuxWindowId != null && global.backendRouter) {
        try {
          await global.backendRouter.closeTab(tab.tmuxWindowId);
        } catch (error) {
          var message = String(error || '');
          if (!/can't find window|no such window/i.test(message)) {
            showStatus('Failed to close tmux window: ' + message);
          }
        }
      }

      const paneIds = allPanesInTab(tabId);
      for (const pid of paneIds) {
        const pane = panes.get(pid);
        if (!pane) continue;
        unregisterPaneDnd(pid);
        if (notifyBackend && pane.kind === 'terminal' && pane.spawned && pane.type !== 'tmux') {
          notifyTerminalClosed(pid, pane.type);
        }
        if (global.tmuxIdMap) {
          global.tmuxIdMap.removePaneByFrontend(pid);
        }
        if (pane.cleanupMouseBridge) pane.cleanupMouseBridge();
        if (pane.resizeObserver) pane.resizeObserver.disconnect();
        if (pane._tmuxSnapshotTimer) {
          clearTimeout(pane._tmuxSnapshotTimer);
          pane._tmuxSnapshotTimer = null;
        }
        if (pane.term) pane.term.dispose();
        panes.delete(pid);
      }

      if (tab.type === 'tmux' && global.tmuxIdMap) {
        global.tmuxIdMap.removeWindowByTab(tabId);
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

    async function createTmuxTab(options = {}) {
      console.info('[tmux] createTmuxTab start', options);
      const tabs = getTabs();
      const panes = getPanes();
      const tmuxWindowId = Number(options.windowId);
      const paneSpecs = Array.isArray(options.panes) ? options.panes : [];
      const existingTabId = global.tmuxIdMap ? global.tmuxIdMap.getTabForWindow(tmuxWindowId) : null;

      if (existingTabId != null) {
        const existingTab = tabs.get(existingTabId);
        if (existingTab) {
          console.info('[tmux] createTmuxTab existing tab found', {
            existingTabId: existingTabId,
            tmuxWindowId: tmuxWindowId,
          });
          const existingTmuxPaneIds = allPanesInTab(existingTabId)
            .map((paneId) => (global.tmuxIdMap ? global.tmuxIdMap.getTmuxForPane(paneId) : null))
            .filter((paneId) => paneId != null)
            .map(Number)
            .sort(function (a, b) { return a - b; });
          const nextTmuxPaneIds = paneSpecs
            .map(function (spec) { return parseTmuxNumericId(spec.id); })
            .filter(function (paneId) { return paneId != null; })
            .sort(function (a, b) { return a - b; });
          const samePaneSet = existingTmuxPaneIds.length === nextTmuxPaneIds.length
            && existingTmuxPaneIds.every(function (paneId, index) { return paneId === nextTmuxPaneIds[index]; });
          existingTab.label = options.name || existingTab.label;
          existingTab.hasCustomTitle = true;
          setTabLabel(existingTab.button, existingTab.label);
          existingTab.button.title = existingTab.label;
          if (samePaneSet) {
            console.info('[tmux] createTmuxTab reusing existing tab', {
              existingTabId: existingTabId,
              tmuxWindowId: tmuxWindowId,
            });
            syncExistingTmuxTab(existingTab, paneSpecs);
            return existingTab;
          }
          console.info('[tmux] createTmuxTab updating existing tab in-place', {
            existingTabId: existingTabId,
            tmuxWindowId: tmuxWindowId,
          });

          const paneByTmuxId = new Map();
          const existingPaneIds = allPanesInTab(existingTabId);
          for (const paneId of existingPaneIds) {
            const pane = panes.get(paneId);
            if (!pane) continue;
            const tmuxPaneId = pane.tmuxPaneId != null
              ? pane.tmuxPaneId
              : (global.tmuxIdMap ? global.tmuxIdMap.getTmuxForPane(paneId) : null);
            if (tmuxPaneId != null) {
              paneByTmuxId.set(Number(tmuxPaneId), pane);
            }
          }

          const nextPaneIds = new Set();
          const createdPaneInfos = [];
          for (const spec of paneSpecs) {
            const tmuxPaneId = parseTmuxNumericId(spec.id);
            if (tmuxPaneId == null) continue;
            nextPaneIds.add(tmuxPaneId);

            let pane = paneByTmuxId.get(tmuxPaneId);
            if (!pane) {
              const paneId = allocatePaneId();
              const paneEl = document.createElement('div');
              paneEl.className = 'terminal-pane';
              paneEl.dataset.paneId = paneId;
              const initialized = initTerminal(paneEl);
              const term = initialized.term;
              const fitAddon = initialized.fitAddon;
              pane = {
                paneId,
                tabId: existingTab.id,
                kind: 'terminal',
                type: 'tmux',
                tmuxPaneId: tmuxPaneId,
                connectionId: null,
                term,
                fitAddon,
                root: paneEl,
                spawned: true,
                lastCols: 0,
                lastRows: 0,
                cleanupMouseBridge: setupTmuxRightClickBridge(term, paneEl),
                resizeObserver: null,
                debounceTimer: null,
              };
              writeTmuxPaneSnapshot(pane, spec.content);
              panes.set(paneId, pane);
              pane.resizeObserver = createPaneResizeObserver(pane, fitAndResizePane);
              paneEl.addEventListener('mousedown', () => setFocusedPane(paneId));
              term.onData((data) => {
                if (!pane.spawned) return;
                onTerminalData(pane, paneId, data);
              });
              if (global.tmuxIdMap) {
                global.tmuxIdMap.addPane(tmuxPaneId, paneId);
              }
            } else {
              pane.tmuxPaneId = tmuxPaneId;
              writeTmuxPaneSnapshot(pane, spec.content);
              if (global.tmuxIdMap) {
                global.tmuxIdMap.addPane(tmuxPaneId, pane.paneId);
              }
            }

            createdPaneInfos.push({
              paneId: pane.paneId,
              width: Number(spec.width) || 80,
              height: Number(spec.height) || 24,
              left: Number(spec.left) || 0,
              top: Number(spec.top) || 0,
              tmuxPaneId: tmuxPaneId,
              active: !!spec.active,
            });
          }

          for (const existingPaneId of existingPaneIds) {
            const pane = panes.get(existingPaneId);
            if (!pane) continue;
            const tmuxPaneId = pane.tmuxPaneId != null
              ? pane.tmuxPaneId
              : (global.tmuxIdMap ? global.tmuxIdMap.getTmuxForPane(existingPaneId) : null);
            if (tmuxPaneId == null || !nextPaneIds.has(Number(tmuxPaneId))) {
              disposePaneLocal(existingPaneId);
            }
          }

          existingTab.label = options.name || existingTab.label;
          existingTab.hasCustomTitle = true;
          setTabLabel(existingTab.button, existingTab.label);
          existingTab.button.title = existingTab.label;
          existingTab.treeRoot = buildTmuxTree(createdPaneInfos);
          const activePaneInfo = createdPaneInfos.find((info) => info.active) || createdPaneInfos[0];
          existingTab.focusedPaneId = activePaneInfo ? activePaneInfo.paneId : existingTab.focusedPaneId;
          rebuildTreeDOM({ treeRoot: existingTab.treeRoot, containerEl: existingTab.containerEl });

          if (options.activate !== false) {
            activateTab(existingTab.id);
          } else {
            fitAndResizeTab(existingTab);
          }

          return existingTab;
        }
      }

      const tabId = allocateTabId();
      const label = options.name || ('Window ' + tmuxWindowId);
      const button = makeTabButton(label, () => closeTab(tabId));
      button.classList.add('entering');

      const containerEl = document.createElement('div');
      containerEl.className = 'tab-tree-root';

      const createdPaneIds = [];
      const paneInfos = paneSpecs.length > 0 ? paneSpecs : [{ id: 0, active: true, width: 80, height: 24 }];
      for (const spec of paneInfos) {
        const paneId = allocatePaneId();
        const paneEl = document.createElement('div');
        paneEl.className = 'terminal-pane';
        paneEl.dataset.paneId = paneId;

        const { term, fitAddon } = initTerminal(paneEl);
        const pane = {
          paneId,
          tabId,
          kind: 'terminal',
          type: 'tmux',
          tmuxPaneId: parseTmuxNumericId(spec.id),
          connectionId: null,
          term,
          fitAddon,
          root: paneEl,
          spawned: true,
          lastCols: 0,
          lastRows: 0,
          cleanupMouseBridge: setupTmuxRightClickBridge(term, paneEl),
          resizeObserver: null,
          debounceTimer: null,
        };
        writeTmuxPaneSnapshot(pane, spec.content);
        panes.set(paneId, pane);
        console.info('[tmux] createTmuxTab pane created', {
          frontendPaneId: paneId,
          tmuxPaneId: pane.tmuxPaneId == null ? null : pane.tmuxPaneId,
          type: pane.type,
        });
        pane.resizeObserver = createPaneResizeObserver(pane, fitAndResizePane);
        paneEl.addEventListener('mousedown', () => setFocusedPane(paneId));
        term.onData((data) => {
          if (!pane.spawned) return;
          onTerminalData(pane, paneId, data);
        });
        createdPaneIds.push({
          paneId,
          width: Number(spec.width) || 80,
          height: Number(spec.height) || 24,
          left: Number(spec.left) || 0,
          top: Number(spec.top) || 0,
          tmuxPaneId: pane.tmuxPaneId,
          active: !!spec.active,
        });
      }

      tabBarEl.appendChild(button);
      terminalHostEl.appendChild(containerEl);

      const initialFocused = createdPaneIds.find((paneInfo) => paneInfo.active) || createdPaneIds[0];
      const tab = {
        id: tabId,
        label,
        type: 'tmux',
        hasCustomTitle: true,
        button,
        containerEl,
        treeRoot: buildTmuxTree(createdPaneIds),
        focusedPaneId: initialFocused ? initialFocused.paneId : null,
        tmuxWindowId,
      };
      tabs.set(tabId, tab);
      console.info('[tmux] createTmuxTab tab created', {
        tabId: tabId,
        tmuxWindowId: tmuxWindowId,
        focusedPaneId: tab.focusedPaneId,
      });

      if (global.tmuxIdMap) {
        global.tmuxIdMap.addWindow(tmuxWindowId, tabId);
        for (const paneInfo of createdPaneIds) {
          if (paneInfo.tmuxPaneId != null) {
            global.tmuxIdMap.addPane(paneInfo.tmuxPaneId, paneInfo.paneId);
            console.info('[tmux] createTmuxTab mapped pane', {
              frontendPaneId: paneInfo.paneId,
              tmuxPaneId: paneInfo.tmuxPaneId,
            });
          }
        }
      }

      setupDividerDrag(
        containerEl,
        () => tab.treeRoot,
        (newTree) => { tab.treeRoot = newTree; },
        () => scheduleTabResize(tab),
      );
      updateTabBarVisibility();
      button.addEventListener('click', () => activateTab(tabId));
      rebuildTreeDOM({ treeRoot: tab.treeRoot, containerEl });

      requestAnimationFrame(() => {
        requestAnimationFrame(() => button.classList.remove('entering'));
      });

      if (options.activate !== false) {
        console.info('[tmux] createTmuxTab activating tab', { tabId: tabId });
        activateTab(tabId);
      } else {
        fitAndResizeTab(tab);
      }

      console.info('[tmux] createTmuxTab done', {
        tabId: tabId,
        tmuxWindowId: tmuxWindowId,
      });
      return tab;
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
        () => scheduleTabResize(tab),
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
      }
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
        () => scheduleTabResize(tab),
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
      createTmuxTab,
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
