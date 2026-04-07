(function initConchBridgeRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const showStatus = deps.showStatus;
    const inputRuntime = deps.inputRuntime;
    const layoutRuntime = deps.layoutRuntime;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const currentPane = deps.currentPane;
    const currentTab = deps.currentTab;
    const createSshTab = deps.createSshTab;
    const getHandleMenuAction = deps.getHandleMenuAction;

    const clipboardRuntime = global.conchClipboardRuntime && global.conchClipboardRuntime.create
      ? global.conchClipboardRuntime.create({
          invoke,
          showStatus: (message) => showStatus(message),
          isTextInputTarget: (el) => inputRuntime.isTextInputTarget(el),
          getCurrentPane: () => currentPane(),
        })
      : null;

    const commandPaletteRuntime = global.conchCommandPaletteRuntime && global.conchCommandPaletteRuntime.create
      ? global.conchCommandPaletteRuntime.create({
          invoke,
          listen: listenOnCurrentWindow,
          esc: (text) => global.utils.esc(text),
          handleMenuAction: (action) => getHandleMenuAction()(action),
          createSshTab: (opts) => createSshTab(opts),
          getCurrentPane: () => currentPane(),
          showStatus: (message) => showStatus(message),
          refreshTitlebar: () => {
            if (global.titlebar && typeof global.titlebar.refresh === 'function') {
              global.titlebar.refresh().catch(() => {});
            }
          },
          refreshSshPanel: () => {
            if (global.sshPanel) global.sshPanel.refreshAll();
          },
        })
      : null;

    function fitAndResizePane(pane) {
      if (layoutRuntime && layoutRuntime.fitAndResizePane) {
        return layoutRuntime.fitAndResizePane(pane);
      }
    }

    function fitAndResizeTab(tab) {
      if (layoutRuntime && layoutRuntime.fitAndResizeTab) {
        return layoutRuntime.fitAndResizeTab(tab);
      }
    }

    function debouncedFitAndResize() {
      if (layoutRuntime && layoutRuntime.debouncedFitAndResize) {
        return layoutRuntime.debouncedFitAndResize();
      }
    }

    function normalizeTabTitle(rawTitle, fallback) {
      if (layoutRuntime && layoutRuntime.normalizeTabTitle) {
        return layoutRuntime.normalizeTabTitle(rawTitle, fallback);
      }
      return fallback;
    }

    function rebuildTreeDOM(tab) {
      if (layoutRuntime && layoutRuntime.rebuildTreeDOM) {
        return layoutRuntime.rebuildTreeDOM(tab);
      }
    }

    function isTextInputTarget(el) {
      return inputRuntime.isTextInputTarget(el);
    }

    function writeTextToCurrentPane(text) {
      if (!clipboardRuntime || typeof clipboardRuntime.writeTextToCurrentPane !== 'function') return false;
      return clipboardRuntime.writeTextToCurrentPane(text);
    }

    function pasteIntoCurrentPane(explicitText) {
      if (!clipboardRuntime || typeof clipboardRuntime.pasteIntoCurrentPane !== 'function') {
        return Promise.resolve(false);
      }
      return clipboardRuntime.pasteIntoCurrentPane(explicitText);
    }

    function openCommandPalette() {
      if (!commandPaletteRuntime || typeof commandPaletteRuntime.open !== 'function') {
        return Promise.resolve();
      }
      return commandPaletteRuntime.open();
    }

    function closeCommandPalette(refocus = true) {
      if (!commandPaletteRuntime || typeof commandPaletteRuntime.close !== 'function') return;
      commandPaletteRuntime.close(refocus);
    }

    function isCommandPaletteOpen() {
      if (!commandPaletteRuntime || typeof commandPaletteRuntime.isOpen !== 'function') return false;
      return commandPaletteRuntime.isOpen();
    }

    function initClipboardListeners() {
      if (clipboardRuntime && typeof clipboardRuntime.initListeners === 'function') {
        clipboardRuntime.initListeners();
      }
    }

    return {
      fitAndResizePane,
      fitAndResizeTab,
      debouncedFitAndResize,
      normalizeTabTitle,
      rebuildTreeDOM,
      isTextInputTarget,
      writeTextToCurrentPane,
      pasteIntoCurrentPane,
      openCommandPalette,
      closeCommandPalette,
      isCommandPaletteOpen,
      initClipboardListeners,
    };
  }

  global.conchBridgeRuntime = {
    create,
  };
})(window);
