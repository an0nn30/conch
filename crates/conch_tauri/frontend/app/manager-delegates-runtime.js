(function initConchManagerDelegatesRuntime(global) {
  function create() {
    let paneManager = null;
    let tabManager = null;

    function requirePaneManager(method) {
      if (!paneManager || typeof paneManager[method] !== 'function') {
        throw new Error(`paneManager.${method} is unavailable`);
      }
      return paneManager[method].bind(paneManager);
    }

    function requireTabManager(method) {
      if (!tabManager || typeof tabManager[method] !== 'function') {
        throw new Error(`tabManager.${method} is unavailable`);
      }
      return tabManager[method].bind(tabManager);
    }

    return {
      setPaneManager(nextPaneManager) {
        paneManager = nextPaneManager;
      },
      setTabManager(nextTabManager) {
        tabManager = nextTabManager;
      },
      currentPane() {
        return requirePaneManager('currentPane')();
      },
      refocusActiveTerminal() {
        return requirePaneManager('refocusActiveTerminal')();
      },
      getTabForPane(paneId) {
        return requirePaneManager('getTabForPane')(paneId);
      },
      allPanesInTab(tabId) {
        return requirePaneManager('allPanesInTab')(tabId);
      },
      setFocusedPane(paneId) {
        return requirePaneManager('setFocusedPane')(paneId);
      },
      closePane(paneId) {
        return requirePaneManager('closePane')(paneId);
      },
      splitPane(direction) {
        return requirePaneManager('splitPane')(direction);
      },
      currentTab() {
        return requireTabManager('currentTab')();
      },
      updateTabBarVisibility() {
        return requireTabManager('updateTabBarVisibility')();
      },
      renumberTabs() {
        return requireTabManager('renumberTabs')();
      },
      activateTab(tabId) {
        return requireTabManager('activateTab')(tabId);
      },
      closeTab(tabId, options) {
        return requireTabManager('closeTab')(tabId, options);
      },
      makeTabButton(label, onClose) {
        return requireTabManager('makeTabButton')(label, onClose);
      },
      setTabLabel(button, text) {
        return requireTabManager('setTabLabel')(button, text);
      },
      getTabLabel(button) {
        return requireTabManager('getTabLabel')(button);
      },
      renameActiveTab() {
        return requireTabManager('renameActiveTab')();
      },
      startTabRename(tab) {
        if (!tab) return;
        return requireTabManager('startTabRename')(tab.id);
      },
      createTab(options) {
        return requireTabManager('createTab')(options);
      },
      createSshTab(opts) {
        return requireTabManager('createSshTab')(opts);
      },
      createTmuxTab(opts) {
        return requireTabManager('createTmuxTab')(opts);
      },
    };
  }

  global.conchManagerDelegatesRuntime = {
    create,
  };
})(window);
