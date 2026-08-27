(function initTermLabTabSwitcherRuntime(global) {
  'use strict';

  // ctrl+tab MRU switcher state machine. Holds ctrl: tab cycles forward,
  // shift+tab backward, digits 1-5 jump-commit, Escape cancels; releasing
  // ctrl commits the selection. The overlay itself is behind the injected
  // `view` seam ({ open, update, close }) so this machine stays DOM-free.
  function create(deps) {
    const getTabItems = deps.getTabItems;
    const activateTab = deps.activateTab;
    const view = deps.view;

    const mruApi = global.termlabTabMru;
    const mru = mruApi.create();

    let open = false;
    let items = [];
    let selectedIndex = 0;

    function close(commitId) {
      open = false;
      items = [];
      view.close();
      if (commitId != null) activateTab(commitId);
    }

    function cancel() {
      if (open) close(null);
    }

    function openSwitcher(backward) {
      const live = getTabItems() || [];
      if (live.length < 2) return false;
      const byId = new Map(live.map((item) => [item.id, item]));
      items = mru.order(live.map((item) => item.id)).map((id) => byId.get(id));
      // Forward entry lands on the previous tab — a quick tap bounces between
      // the two most recent tabs; backward entry starts from the far end.
      selectedIndex = backward ? items.length - 1 : Math.min(1, items.length - 1);
      open = true;
      view.open(items, selectedIndex);
      return true;
    }

    function step(delta) {
      selectedIndex = mruApi.stepIndex(items.length, selectedIndex, delta);
      view.update(items, selectedIndex);
    }

    function onKeyDown(event) {
      const isCtrlTab = event.key === 'Tab' && event.ctrlKey && !event.metaKey && !event.altKey;
      if (!open) {
        if (!isCtrlTab) return false;
        return openSwitcher(event.shiftKey);
      }
      if (isCtrlTab) {
        step(event.shiftKey ? -1 : 1);
        return true;
      }
      if (event.key === 'Escape') {
        close(null);
        return true;
      }
      if (event.key >= '1' && event.key <= '5') {
        const index = event.key.charCodeAt(0) - '1'.charCodeAt(0);
        if (index < items.length) {
          close(items[index].id);
        }
        return true;
      }
      // Anything else while the switcher is up is noise — swallow it so the
      // terminal underneath never sees keystrokes typed mid-switch.
      return true;
    }

    // Mouse path: the overlay's rows call this on click.
    function commitIndex(index) {
      if (!open || !items[index]) return;
      close(items[index].id);
    }

    function onKeyUp(event) {
      if (!open || event.key !== 'Control') return false;
      close(items[selectedIndex] ? items[selectedIndex].id : null);
      return true;
    }

    function init() {
      if (typeof global.addEventListener === 'function') {
        global.addEventListener('termlab-active-tab-changed', (event) => {
          if (event && event.detail && event.detail.tabId != null) {
            mru.touch(event.detail.tabId);
          }
        });
        // Losing the window mid-switch (cmd+tab away, a dialog stealing
        // focus) means the ctrl keyup may never arrive: treat it as cancel.
        global.addEventListener('blur', () => cancel());
      }
      const router = global.termlabKeyboardRouter;
      if (router && typeof router.register === 'function') {
        router.register({
          name: 'tab-switcher',
          priority: 130,
          onKeyDown,
          onKeyUp,
        });
      }
    }

    return { init, cancel, commitIndex };
  }

  global.termlabTabSwitcherRuntime = {
    create,
  };
})(window);
