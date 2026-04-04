/**
 * Backend Router — routes tab/pane actions by backend mode.
 *
 * In 'local' mode, actions go through the existing PTY commands.
 * In 'tmux' mode, actions go through the tmux Tauri commands.
 */
(function initConchBackendRouter(global) {
  'use strict';

  function create(deps) {
    const invoke = deps.invoke;

    let mode = 'local';

    function setMode(m) {
      mode = m;
      console.info('[backend-router] mode set to:', mode);
    }

    function getMode() {
      return mode;
    }

    function isTmux() {
      return mode === 'tmux';
    }

    // --- Tab/Window actions ---

    function newTab() {
      if (isTmux()) {
        return invoke('tmux_new_window');
      }
      return null;
    }

    function closeTab(tmuxWindowId) {
      if (isTmux()) {
        return invoke('tmux_close_window', { windowId: tmuxWindowId });
      }
      return null;
    }

    function renameTab(tmuxWindowId, name) {
      if (isTmux()) {
        return invoke('tmux_rename_window', { windowId: tmuxWindowId, name });
      }
      return null;
    }

    // --- Pane actions ---

    function writeToPane(paneId, data) {
      if (isTmux()) {
        return invoke('tmux_write_to_pane', { paneId, data });
      }
      return invoke('write_to_pty', { paneId, data });
    }

    function resizePane(paneId, cols, rows) {
      if (isTmux()) {
        return invoke('tmux_resize_pane', { paneId, cols, rows });
      }
      return invoke('resize_pty', { paneId, cols, rows });
    }

    function resizeClient(cols, rows) {
      if (isTmux()) {
        return invoke('tmux_resize_client', { cols, rows });
      }
      return null;
    }

    function splitVertical(paneId) {
      if (isTmux()) {
        // "Vertical split" in Conch means side-by-side panes, which maps to tmux -h.
        return invoke('tmux_split_pane', { paneId, horizontal: true });
      }
      return null;
    }

    function splitHorizontal(paneId) {
      if (isTmux()) {
        // "Horizontal split" in Conch means stacked panes, which maps to tmux -v.
        return invoke('tmux_split_pane', { paneId, horizontal: false });
      }
      return null;
    }

    function closePane(paneId) {
      if (isTmux()) {
        return invoke('tmux_close_pane', { paneId });
      }
      return invoke('close_pty', { paneId });
    }

    function selectPane(paneId) {
      if (isTmux()) {
        return invoke('tmux_select_pane', { paneId });
      }
      return null;
    }

    // --- Session actions ---

    function connect(sessionName) {
      return invoke('tmux_connect', { sessionName });
    }

    function disconnect() {
      return invoke('tmux_disconnect');
    }

    return {
      setMode,
      getMode,
      isTmux,
      newTab,
      closeTab,
      renameTab,
      writeToPane,
      resizePane,
      resizeClient,
      splitVertical,
      splitHorizontal,
      closePane,
      selectPane,
      connect,
      disconnect,
    };
  }

  global.conchBackendRouter = { create };
})(typeof window !== 'undefined' ? window : globalThis);
