// Terminal-pane and tab-strip right-click menus. Both render through the
// shared window.tlMenu component (styles/design-system/components/menu.css,
// app/ui/tl-menu.js) — previously these were static <div id="..."> elements
// baked into index.html and toggled via style.display; tlMenu now owns
// positioning, outside-click/Escape dismissal, and single-instance behavior,
// so this module only needs to build each item list at click time.
(function initTermLabContextMenuRuntime(global) {
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

    if (!global.tlMenu || typeof global.tlMenu.open !== 'function') {
      console.error('context-menu-runtime: window.tlMenu is unavailable');
      return;
    }

    terminalHostEl.addEventListener('contextmenu', (event) => {
      const paneEl = event.target.closest('.terminal-pane');
      if (!paneEl) return;
      const paneId = parseInt(paneEl.dataset.paneId, 10);
      const pane = getPanes().get(paneId);
      if (!pane) return;
      if (pane.kind === 'terminal' && terminalMouseModeIsActive(pane.term)) return;

      event.preventDefault();
      event.stopPropagation();

      setFocusedPane(paneId);
      global.tlMenu.open({
        x: event.clientX,
        y: event.clientY,
        items: [
          { label: 'Split Vertically', onSelect: () => splitPane('vertical') },
          { label: 'Split Horizontally', onSelect: () => splitPane('horizontal') },
        ],
        ariaLabel: 'Terminal actions',
        routerName: 'terminal-context-menu',
      });
    });

    tabBarEl.addEventListener('contextmenu', (event) => {
      const btn = event.target.closest('.tab-btn');
      if (!btn) return;
      event.preventDefault();

      let tabId = null;
      for (const [id, tab] of getTabs()) {
        if (tab.button === btn) {
          tabId = id;
          break;
        }
      }
      if (tabId == null) return;

      global.tlMenu.open({
        x: event.clientX,
        y: event.clientY,
        items: [
          { label: 'Rename Tab', onSelect: () => startTabRenameById(tabId) },
          { label: 'Close Tab', onSelect: () => closeTab(tabId) },
        ],
        ariaLabel: 'Tab actions',
        routerName: 'tab-context-menu',
      });
    });
  }

  global.termlabContextMenuRuntime = {
    init,
  };
})(window);
