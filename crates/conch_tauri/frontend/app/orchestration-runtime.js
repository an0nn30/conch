(function initConchOrchestrationRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const terminalHostEl = deps.terminalHostEl;
    const currentWindow = deps.currentWindow;
    const tabs = deps.tabs;
    const panes = deps.panes;
    const pluginViewPaneById = deps.pluginViewPaneById;
    const pluginViewSizeMemory = deps.pluginViewSizeMemory;
    const getActiveTabId = deps.getActiveTabId;
    const allocPaneId = deps.allocPaneId;
    const currentPane = deps.currentPane;
    const currentTab = deps.currentTab;
    const setFocusedPane = deps.setFocusedPane;
    const closePane = deps.closePane;
    const createSshTab = deps.createSshTab;
    const splitPane = deps.splitPane;
    const getPaneManager = deps.getPaneManager;
    const isDebugEnabled = deps.isDebugEnabled;
    const debugLog = deps.debugLog;
    const debouncedFitAndResize = deps.debouncedFitAndResize;
    const rebuildTreeDOM = deps.rebuildTreeDOM;

    let paneDnd = null;
    let pluginRuntime = null;
    let debouncedSaveLayout = () => {};

    async function init() {
      pluginRuntime = global.conchPluginRuntime && global.conchPluginRuntime.create
        ? global.conchPluginRuntime.create({
            getPanes: () => panes,
            getTabs: () => tabs,
            getActiveTabId: () => getActiveTabId(),
            getPluginViewPaneById: () => pluginViewPaneById,
            getPluginViewSizeMemory: () => pluginViewSizeMemory,
            currentPane: () => currentPane(),
            setFocusedPane: (paneId) => setFocusedPane(paneId),
            closePane: (paneId) => closePane(paneId),
            allocatePaneId: () => allocPaneId(),
            splitLeaf: (treeRoot, sourcePaneId, newPaneId, direction) => (
              global.splitTree.splitLeaf(treeRoot, sourcePaneId, newPaneId, direction)
            ),
            findParent: (treeRoot, paneId) => global.splitTree.findParent(treeRoot, paneId),
            firstLeaf: (treeRoot) => global.splitTree.firstLeaf(treeRoot),
            rebuildTreeDOM: (tab) => rebuildTreeDOM(tab),
            createPaneResizeObserver: (pane, fitCb) => global.splitPane.createPaneResizeObserver(pane, fitCb),
            registerDraggablePaneHeader: (paneId, headerEl, kind) => {
              if (paneDnd) paneDnd.registerDraggablePaneHeader(paneId, headerEl, kind);
            },
            invoke,
            renderPluginWidgets: (container, result, pluginName, viewId) => {
              if (result && global.pluginWidgets) {
                global.pluginWidgets.renderWidgets(container, result, pluginName, viewId);
              }
            },
          })
        : null;

      paneDnd = global.paneDnd && global.paneDnd.initPaneDnd
        ? global.paneDnd.initPaneDnd({
            getActiveTabId: () => getActiveTabId(),
            getPaneById: (paneId) => panes.get(paneId) || null,
            getActiveCanvasRect: () => {
              const tab = currentTab();
              if (!tab || !tab.containerEl) return null;
              return tab.containerEl.getBoundingClientRect();
            },
            getActiveContainerEl: () => {
              const tab = currentTab();
              return tab ? tab.containerEl : null;
            },
            movePaneByDrop: (dragPaneId, targetPaneId, zone) => {
              const paneManager = getPaneManager();
              if (!paneManager || !paneManager.movePaneByDrop) return false;
              return paneManager.movePaneByDrop(dragPaneId, targetPaneId, zone);
            },
            onFocusPane: (paneId) => setFocusedPane(paneId),
            isDebugEnabled: () => isDebugEnabled(),
            debugLog: (...args) => debugLog(...args),
          })
        : null;

      if (global.conchToolWindowRuntime && global.conchToolWindowRuntime.create) {
        const toolWindowRuntime = global.conchToolWindowRuntime.create({
          invoke,
          listenOnCurrentWindow,
          debouncedFitAndResize: () => debouncedFitAndResize(),
          getCurrentTab: () => currentTab(),
          getCurrentPane: () => currentPane(),
          createSshTab: (opts) => createSshTab(opts),
          openPluginDockedViewFromRequest: async (payload) => {
            if (!pluginRuntime || !pluginRuntime.openPluginDockedViewFromRequest) {
              throw new Error('pluginRuntime.openPluginDockedViewFromRequest is unavailable');
            }
            return pluginRuntime.openPluginDockedViewFromRequest(payload);
          },
          setFocusedPane: (paneId) => setFocusedPane(paneId),
          closePane: (paneId) => closePane(paneId),
          getPluginViewPaneById: () => pluginViewPaneById,
        });
        const runtimeResult = await toolWindowRuntime.init();
        if (runtimeResult && typeof runtimeResult.debouncedSaveLayout === 'function') {
          debouncedSaveLayout = runtimeResult.debouncedSaveLayout;
        }
      }

      if (global.conchDragDropRuntime && global.conchDragDropRuntime.create) {
        const dragDropRuntime = global.conchDragDropRuntime.create({
          terminalHostEl,
          currentWindow,
          getCurrentPane: () => currentPane(),
          invoke,
        });
        dragDropRuntime.init();
      }

      return {
        paneDnd,
        pluginRuntime,
        debouncedSaveLayout,
      };
    }

    return {
      init,
    };
  }

  global.conchOrchestrationRuntime = {
    create,
  };
})(window);
