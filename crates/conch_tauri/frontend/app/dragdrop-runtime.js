(function initConchDragDropRuntime(global) {
  function create(deps) {
    const terminalHostEl = deps.terminalHostEl;
    const currentWindow = deps.currentWindow;
    const getCurrentPane = deps.getCurrentPane;
    const invoke = deps.invoke;

    function writePathsToTerminal(paths) {
      const pane = getCurrentPane();
      if (!pane || !pane.spawned) return;
      const escaped = paths.map((path) => {
        if (/[\s"'\\$`!#&|;()<>]/.test(path)) {
          return "'" + path.replace(/'/g, "'\\''") + "'";
        }
        return path;
      });
      const text = escaped.join(' ');
      const cmd = pane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
      invoke(cmd, { paneId: pane.paneId, data: text }).catch((event) => {
        console.error('drag-drop write error:', event);
      });
    }

    function init() {
      terminalHostEl.addEventListener('dragover', (event) => {
        event.preventDefault();
        terminalHostEl.classList.add('drag-over');
      }, true);
      terminalHostEl.addEventListener('dragleave', (event) => {
        if (event.target === terminalHostEl || !terminalHostEl.contains(event.relatedTarget)) {
          terminalHostEl.classList.remove('drag-over');
        }
      }, true);
      terminalHostEl.addEventListener('drop', (event) => {
        event.preventDefault();
        event.stopPropagation();
        terminalHostEl.classList.remove('drag-over');
        if (event.dataTransfer && event.dataTransfer.files && event.dataTransfer.files.length > 0) {
          const paths = [];
          for (const file of event.dataTransfer.files) {
            const path = file.path || file.name;
            if (path) paths.push(path);
          }
          if (paths.length > 0) {
            writePathsToTerminal(paths);
          }
        }
      }, true);

      if (currentWindow && typeof currentWindow.onDragDropEvent === 'function') {
        currentWindow.onDragDropEvent((event) => {
          if (!event || !event.payload) return;
          if (event.payload.type === 'over') {
            terminalHostEl.classList.add('drag-over');
          } else if (event.payload.type === 'leave') {
            terminalHostEl.classList.remove('drag-over');
          } else if (event.payload.type === 'drop') {
            terminalHostEl.classList.remove('drag-over');
            const paths = event.payload.paths;
            if (paths && paths.length > 0) {
              writePathsToTerminal(paths);
            }
          }
        });
      }
    }

    return {
      init,
      writePathsToTerminal,
    };
  }

  global.conchDragDropRuntime = {
    create,
  };
})(window);
