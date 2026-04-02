(function initConchOrchestrationRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listen = deps.listen;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const terminalHostEl = deps.terminalHostEl;
    const currentWindow = deps.currentWindow;
    const tabs = deps.tabs;
    const panes = deps.panes;
    const getActiveTabId = deps.getActiveTabId;
    const allocPaneId = deps.allocPaneId;
    const currentPane = deps.currentPane;
    const currentTab = deps.currentTab;
    const setFocusedPane = deps.setFocusedPane;
    const closePane = deps.closePane;
    const createTab = deps.createTab;
    const createSshTab = deps.createSshTab;
    const splitPane = deps.splitPane;
    const getPaneManager = deps.getPaneManager;
    const isDebugEnabled = deps.isDebugEnabled;
    const debugLog = deps.debugLog;
    const debouncedFitAndResize = deps.debouncedFitAndResize;
    const rebuildTreeDOM = deps.rebuildTreeDOM;

    let paneDnd = null;
    let debouncedSaveLayout = () => {};

    async function init() {
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
          listen,
          listenOnCurrentWindow,
          debouncedFitAndResize: () => debouncedFitAndResize(),
          getCurrentTab: () => currentTab(),
          getCurrentPane: () => currentPane(),
          createTab: () => createTab(),
          createSshTab: (opts) => createSshTab(opts),
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
