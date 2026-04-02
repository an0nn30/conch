(function initConchPluginRuntime(global) {
  function create(deps) {
    const getPanes = deps.getPanes;
    const getTabs = deps.getTabs;
    const getActiveTabId = deps.getActiveTabId;
    const getPluginViewPaneById = deps.getPluginViewPaneById;
    const getPluginViewSizeMemory = deps.getPluginViewSizeMemory;
    const currentPane = deps.currentPane;
    const setFocusedPane = deps.setFocusedPane;
    const closePane = deps.closePane;
    const allocatePaneId = deps.allocatePaneId;
    const splitLeaf = deps.splitLeaf;
    const findParent = deps.findParent;
    const firstLeaf = deps.firstLeaf;
    const rebuildTreeDOM = deps.rebuildTreeDOM;
    const createPaneResizeObserver = deps.createPaneResizeObserver;
    const registerDraggablePaneHeader = deps.registerDraggablePaneHeader;
    const invoke = deps.invoke;
    const renderPluginWidgets = deps.renderPluginWidgets;

    async function openPluginDockedViewFromRequest(payload) {
      const panes = getPanes();
      const tabs = getTabs();
      const pluginViewPaneById = getPluginViewPaneById();
      const pluginViewSizeMemory = getPluginViewSizeMemory();
      const plugin = payload && payload.plugin;
      const viewId = payload && payload.view_id;
      if (!plugin || !viewId) return;
      if (pluginViewPaneById.has(viewId)) {
        setFocusedPane(pluginViewPaneById.get(viewId));
        return;
      }

      let req = {};
      try {
        req = payload.request_json ? JSON.parse(payload.request_json) : {};
      } catch (_) {
        req = {};
      }
      const dock = req.dock || {};
      const direction = dock.direction === 'vertical' ? 'vertical' : 'horizontal';
      const ratioRaw = Number(dock.ratio);
      const rememberedRatio = pluginViewSizeMemory.get(viewId);
      const ratio = Number.isFinite(rememberedRatio)
        ? Math.max(0.1, Math.min(0.9, rememberedRatio))
        : (Number.isFinite(ratioRaw) ? Math.max(0.1, Math.min(0.9, ratioRaw)) : 0.35);

      let anchor = currentPane();
      if (!anchor && getActiveTabId() != null) {
        const activeTabId = getActiveTabId();
        const tab = tabs.get(activeTabId);
        if (tab && tab.treeRoot) {
          const firstId = firstLeaf(tab.treeRoot);
          anchor = panes.get(firstId) || null;
        }
      }
      if (!anchor) return;

      const tab = tabs.get(anchor.tabId);
      if (!tab) return;

      const newPaneId = allocatePaneId();
      tab.treeRoot = splitLeaf(tab.treeRoot, anchor.paneId, newPaneId, direction);
      const parent = findParent(tab.treeRoot, anchor.paneId);
      if (parent && parent.parent && parent.parent.type === 'split') {
        parent.parent.ratio = ratio;
      }

      const paneEl = document.createElement('div');
      paneEl.className = 'terminal-pane';
      paneEl.dataset.paneId = newPaneId;
      paneEl.dataset.pluginViewId = viewId;

      const header = document.createElement('div');
      header.className = 'ssh-section-header';
      header.style.display = 'flex';
      header.style.alignItems = 'center';
      header.style.justifyContent = 'space-between';
      const titleEl = document.createElement('span');
      titleEl.textContent = (req && req.title) ? String(req.title) : plugin;
      const actionsEl = document.createElement('span');
      actionsEl.style.display = 'inline-flex';
      actionsEl.style.gap = '8px';
      const minBtn = document.createElement('button');
      minBtn.className = 'bottom-panel-action-btn';
      minBtn.textContent = '−';
      minBtn.title = 'Minimize';
      const closeBtn = document.createElement('button');
      closeBtn.className = 'bottom-panel-action-btn';
      closeBtn.textContent = '×';
      closeBtn.title = 'Close';
      actionsEl.appendChild(minBtn);
      actionsEl.appendChild(closeBtn);
      header.appendChild(titleEl);
      header.appendChild(actionsEl);
      paneEl.appendChild(header);

      const widgetContainer = document.createElement('div');
      widgetContainer.className = 'plugin-panel-content';
      widgetContainer.dataset.pluginViewId = viewId;
      paneEl.appendChild(widgetContainer);
      let minimized = false;
      minBtn.addEventListener('click', (event) => {
        event.stopPropagation();
        minimized = !minimized;
        widgetContainer.style.display = minimized ? 'none' : '';
        minBtn.textContent = minimized ? '+' : '−';
      });
      closeBtn.addEventListener('click', (event) => {
        event.stopPropagation();
        closePane(newPaneId);
      });
      registerDraggablePaneHeader(newPaneId, header, 'plugin_view');

      const pane = {
        paneId: newPaneId,
        tabId: tab.id,
        kind: 'plugin_view',
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
        viewId,
        pluginName: plugin,
        widgetContainer,
      };
      panes.set(newPaneId, pane);
      pluginViewPaneById.set(viewId, newPaneId);

      rebuildTreeDOM(tab);
      pane.resizeObserver = createPaneResizeObserver(pane, () => {});
      paneEl.addEventListener('mousedown', () => setFocusedPane(newPaneId));

      invoke('register_plugin_view_binding', {
        viewId,
        paneId: newPaneId,
        tabId: tab.id,
      }).catch(() => {});

      setFocusedPane(newPaneId);

      try {
        const result = await invoke('request_plugin_view_render', {
          pluginName: plugin,
          viewId,
        });
        renderPluginWidgets(widgetContainer, result, plugin, viewId);
      } catch (error) {
        widgetContainer.textContent = 'Plugin view render failed: ' + String(error);
      }
    }

    return {
      openPluginDockedViewFromRequest,
    };
  }

  global.conchPluginRuntime = {
    create,
  };
})(window);
