(function initTermLabAppState(global) {
  function createInitialState() {
    return {
      tabs: new Map(),
      activeTabId: null,
      nextTabId: 1,
      nextTabLabel: 1,
      panes: new Map(),
      nextPaneId: 1,
      focusedPaneId: null,
    };
  }

  global.termlabAppState = {
    createInitialState,
  };
})(window);
