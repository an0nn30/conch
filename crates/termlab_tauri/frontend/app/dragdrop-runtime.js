(function initTermLabDragDropRuntime(global) {
  function create(deps) {
    const terminalHostEl = deps.terminalHostEl;
    const currentWindow = deps.currentWindow;
    const getCurrentPane = deps.getCurrentPane;
    const invoke = deps.invoke;

    // Task 8 fix round: `currentWindow.onDragDropEvent` is a WINDOW-level
    // Tauri v2 event — it fires for every OS drag/drop anywhere in the
    // window, not just ones over the terminal. Before this hit-test existed,
    // a Finder drop onto the SFTP remote pane (files-panel.js's native-drop
    // handling, features/files/native-drop.js) ALSO reached here and got
    // pasted into whatever terminal pane happened to be focused — garbage
    // text in a live shell prompt, potentially executed on the next Enter.
    //
    // `position` is the event payload's own position (physical px, same
    // shape files-panel.js's native drop handling scales/hit-tests — see
    // features/files/native-drop.js, loaded before this file per
    // index.html's script order, which is why this reuses its
    // scaleNativeDropPosition/pointInRect instead of keeping a duplicate).
    // Returns true (accept) when `position` is absent — some drag/drop event
    // shapes may omit it, and failing OPEN there preserves today's
    // unconditional terminal-drop behavior rather than silently breaking
    // plain drops onto the terminal.
    function hitsTerminalHost(position) {
      if (!position) return true;
      const nativeDrop = global.termlabNativeDrop;
      if (!nativeDrop
          || typeof nativeDrop.scaleNativeDropPosition !== 'function'
          || typeof nativeDrop.pointInRect !== 'function') {
        return true; // module unavailable -- fail open rather than breaking terminal drops
      }
      const scaled = nativeDrop.scaleNativeDropPosition(position);
      if (!scaled) return true;
      return nativeDrop.pointInRect(scaled, terminalHostEl.getBoundingClientRect());
    }

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
            if (hitsTerminalHost(event.payload.position)) {
              terminalHostEl.classList.add('drag-over');
            } else {
              terminalHostEl.classList.remove('drag-over');
            }
          } else if (event.payload.type === 'leave') {
            terminalHostEl.classList.remove('drag-over');
          } else if (event.payload.type === 'drop') {
            terminalHostEl.classList.remove('drag-over');
            if (!hitsTerminalHost(event.payload.position)) return;
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

  global.termlabDragDropRuntime = {
    create,
  };
})(window);
