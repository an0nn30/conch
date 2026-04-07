(function initConchContextMenuRuntime(global) {
  function init(deps) {
    const terminalHostEl = deps.terminalHostEl;
    const tabBarEl = deps.tabBarEl;
    const getPanes = deps.getPanes;
    const getTabs = deps.getTabs;
    const terminalMouseModeIsActive = deps.terminalMouseModeIsActive;
    const setFocusedPane = deps.setFocusedPane;
    const splitPane = deps.splitPane;
    const startTabRenameById = deps.startTabRenameById;
    const closeTab = deps.closeTab;

    const termContextMenu = document.getElementById('terminal-context-menu');
    const tabContextMenu = document.getElementById('tab-context-menu');
    let tabContextMenuTabId = null;
    const hideContextMenus = () => {
      if (termContextMenu.style.display !== 'none') termContextMenu.style.display = 'none';
      if (tabContextMenu.style.display !== 'none') tabContextMenu.style.display = 'none';
      tabContextMenuTabId = null;
    };

    terminalHostEl.addEventListener('contextmenu', (event) => {
      const paneEl = event.target.closest('.terminal-pane');
      if (!paneEl) return;
      const paneId = parseInt(paneEl.dataset.paneId, 10);
      const pane = getPanes().get(paneId);
      if (!pane) return;
      if (pane.kind === 'terminal' && terminalMouseModeIsActive(pane.term)) return;

      event.preventDefault();
      event.stopPropagation();

      termContextMenu.style.left = event.clientX + 'px';
      termContextMenu.style.top = event.clientY + 'px';
      termContextMenu.style.display = 'block';
      setFocusedPane(paneId);
    });

    termContextMenu.addEventListener('click', (event) => {
      const btn = event.target.closest('.context-item');
      if (!btn) return;
      hideContextMenus();
      const action = btn.dataset.action;
      if (action === 'split-vertical') splitPane('vertical');
      if (action === 'split-horizontal') splitPane('horizontal');
    });

    tabBarEl.addEventListener('contextmenu', (event) => {
      const btn = event.target.closest('.tab-btn');
      if (!btn) return;
      event.preventDefault();

      for (const [id, tab] of getTabs()) {
        if (tab.button === btn) {
          tabContextMenuTabId = id;
          break;
        }
      }

      tabContextMenu.style.left = event.clientX + 'px';
      tabContextMenu.style.top = event.clientY + 'px';
      tabContextMenu.style.display = 'block';
    });

    tabContextMenu.addEventListener('click', (event) => {
      const btn = event.target.closest('.context-item');
      if (!btn) return;
      hideContextMenus();
      const action = btn.dataset.action;
      if (action === 'rename-tab' && tabContextMenuTabId != null) {
        startTabRenameById(tabContextMenuTabId);
      }
      if (action === 'close-tab' && tabContextMenuTabId != null) {
        closeTab(tabContextMenuTabId);
      }
    });

    document.addEventListener('mousedown', (event) => {
      if (termContextMenu.style.display !== 'none' && !termContextMenu.contains(event.target)) hideContextMenus();
      if (tabContextMenu.style.display !== 'none' && !tabContextMenu.contains(event.target)) hideContextMenus();
    });

    const keyboardRouter = global.conchKeyboardRouter;
    if (keyboardRouter && typeof keyboardRouter.register === 'function') {
      keyboardRouter.register({
        name: 'context-menu-dismiss',
        priority: 180,
        isActive: () => termContextMenu.style.display !== 'none' || tabContextMenu.style.display !== 'none',
        onKeyDown: (event) => {
          if (event.key !== 'Escape') return false;
          hideContextMenus();
          return true;
        },
      });
    } else {
      console.warn('context-menu-runtime: keyboard router unavailable, Escape dismiss handler not registered');
    }
  }

  global.conchContextMenuRuntime = {
    init,
  };
})(window);
