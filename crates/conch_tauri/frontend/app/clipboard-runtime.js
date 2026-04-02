(function initConchClipboardRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const showStatus = deps.showStatus;
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
      showStatus('Paste failed: clipboard unavailable');
      return false;
    }

    function initListeners() {
      document.addEventListener('keydown', (event) => {
        const isPasteCombo = (event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === 'v';
        if (!isPasteCombo) return;
        if (isTextInputTarget(event.target) || isTextInputTarget(document.activeElement)) return;
        if (!getCurrentPane()) return;
        event.preventDefault();
        event.stopPropagation();
        pasteIntoCurrentPane();
      }, true);

      document.addEventListener('keydown', (event) => {
        const isCopyCombo = (event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === 'c';
        if (!isCopyCombo) return;
        if (isTextInputTarget(event.target) || isTextInputTarget(document.activeElement)) return;
        const pane = getCurrentPane();
        const text = pane && pane.term ? pane.term.getSelection() : '';
        if (!text) return;
        event.preventDefault();
        event.stopPropagation();
        invoke('clipboard_write_text', { text }).catch(() => {});
      }, true);

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
