(function initConchAppState(global) {
  function createInitialState() {
    return {
      tabs: new Map(),
      activeTabId: null,
      nextTabId: 1,
      nextTabLabel: 1,
      panes: new Map(),
      pluginViewPaneById: new Map(),
      pluginViewSizeMemory: new Map(),
      nextPaneId: 1,
      focusedPaneId: null,
    };
  }

  global.conchAppState = {
    createInitialState,
  };
})(window);
