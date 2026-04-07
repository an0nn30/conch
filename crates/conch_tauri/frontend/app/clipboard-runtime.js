(function initConchClipboardRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const isTextInputTarget = deps.isTextInputTarget;
    const getCurrentPane = deps.getCurrentPane;

    function writeTextToCurrentPane(text) {
      const pane = getCurrentPane();
      if (!pane || pane.kind !== 'terminal' || !pane.spawned || typeof text !== 'string' || text.length === 0) return false;
      const cmd = pane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
      invoke(cmd, { paneId: pane.paneId, data: text }).catch((event) => {
        console.error('paste write error:', event);
      });
      if (pane.term) pane.term.focus();
      return true;
    }

    async function pasteIntoCurrentPane(explicitText) {
      if (typeof explicitText === 'string') {
        return writeTextToCurrentPane(explicitText);
      }
      try {
        const text = await invoke('clipboard_read_text');
        if (typeof text === 'string' && text.length > 0) {
          return writeTextToCurrentPane(text);
        }
      } catch (_) {}

      const isMac = /mac/i.test(navigator.platform || '');
      if (!isMac) {
        try {
          const text = await navigator.clipboard.readText();
          return writeTextToCurrentPane(text);
        } catch (_) {}
      }
      return false;
    }

    function initListeners() {
      const keyboardRouter = global.conchKeyboardRouter;
      if (keyboardRouter && typeof keyboardRouter.register === 'function') {
        keyboardRouter.register({
          name: 'clipboard-paste',
          priority: 125,
          onKeyDown: (event) => {
            const key = (event.key || '').toLowerCase();
            const isPasteCombo = (event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey && key === 'v';
            if (!isPasteCombo) return false;
            if (isTextInputTarget(event.target) || isTextInputTarget(document.activeElement)) return false;
            if (!getCurrentPane()) return false;
            pasteIntoCurrentPane();
            return true;
          },
        });
        keyboardRouter.register({
          name: 'clipboard-copy',
          priority: 125,
          onKeyDown: (event) => {
            const key = (event.key || '').toLowerCase();
            const isCopyCombo = (event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey && key === 'c';
            if (!isCopyCombo) return false;
            if (isTextInputTarget(event.target) || isTextInputTarget(document.activeElement)) return false;
            const pane = getCurrentPane();
            const text = pane && pane.term ? pane.term.getSelection() : '';
            if (!text) return false;
            invoke('clipboard_write_text', { text }).catch(() => {});
            return true;
          },
        });
      } else {
        console.warn('clipboard-runtime: keyboard router unavailable, copy/paste shortcut handlers were not registered');
      }

      document.addEventListener('paste', (event) => {
        if (isTextInputTarget(event.target) || isTextInputTarget(document.activeElement)) return;
        if (!getCurrentPane()) return;
        const text = event.clipboardData && event.clipboardData.getData
          ? event.clipboardData.getData('text/plain')
          : '';
        event.preventDefault();
        event.stopPropagation();
        if (text && text.length > 0) {
          writeTextToCurrentPane(text);
        } else {
          pasteIntoCurrentPane();
        }
      }, true);
    }

    return {
      initListeners,
      writeTextToCurrentPane,
      pasteIntoCurrentPane,
    };
  }

  global.conchClipboardRuntime = {
    create,
  };
})(window);
