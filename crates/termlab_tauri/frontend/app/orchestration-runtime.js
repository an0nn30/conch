(function initTermLabOrchestrationRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listen = deps.listen;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const layoutService = deps.layoutService;
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
    const activateTab = deps.activateTab;
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

      if (global.termlabToolWindowRuntime && global.termlabToolWindowRuntime.create) {
        const resolveTabById = (rawTabId) => {
          if (rawTabId == null) return null;
          if (tabs.has(rawTabId)) return tabs.get(rawTabId) || null;

          const text = String(rawTabId).trim();
          if (!text) return null;
          if (tabs.has(text)) return tabs.get(text) || null;

          const asNumber = Number(text);
          if (Number.isFinite(asNumber) && tabs.has(asNumber)) {
            return tabs.get(asNumber) || null;
          }

          const asInt = parseInt(text, 10);
          if (Number.isFinite(asInt) && tabs.has(asInt)) {
            return tabs.get(asInt) || null;
          }
          return null;
        };

        const toolWindowRuntime = global.termlabToolWindowRuntime.create({
          invoke,
          listen,
          listenOnCurrentWindow,
          layoutService,
          debouncedFitAndResize: () => debouncedFitAndResize(),
          getCurrentTab: () => currentTab(),
          getTabById: (tabId) => resolveTabById(tabId),
          getCurrentPane: () => currentPane(),
          createTab: (options) => createTab(options),
          createSshTab: (opts) => createSshTab(opts),
          activateTab: (tabId) => activateTab(tabId),
        });
        const runtimeResult = await toolWindowRuntime.init();
        if (runtimeResult && typeof runtimeResult.debouncedSaveLayout === 'function') {
          debouncedSaveLayout = runtimeResult.debouncedSaveLayout;
        }
      }

      if (global.termlabDragDropRuntime && global.termlabDragDropRuntime.create) {
        const dragDropRuntime = global.termlabDragDropRuntime.create({
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

  global.termlabOrchestrationRuntime = {
    create,
  };
})(window);
