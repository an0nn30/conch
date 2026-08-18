(function initTermLabMenuActions(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const getCurrentPane = deps.getCurrentPane;
    const isTextInputTarget = deps.isTextInputTarget;
    const createTab = deps.createTab;
    const createPlainShellTab = deps.createPlainShellTab;
    const showStatus = deps.showStatus;
    const openCommandPalette = deps.openCommandPalette;
    const closeCommandPalette = deps.closeCommandPalette;
    const isCommandPaletteOpen = deps.isCommandPaletteOpen;
    const getActiveTabId = deps.getActiveTabId;
    const closeTab = deps.closeTab;
    const debouncedSaveLayout = deps.debouncedSaveLayout;
    const getZoom = deps.getZoom;
    const setZoom = deps.setZoom;
    const splitPane = deps.splitPane;
    const getFocusedPaneId = deps.getFocusedPaneId;
    const closePane = deps.closePane;
    const renameActiveTab = deps.renameActiveTab;
    const fitAndResizeCurrentTab = deps.fitAndResizeCurrentTab;
    const showAboutDialog = deps.showAboutDialog;
    const showUpdateAvailableToast = deps.showUpdateAvailableToast;
    const initialLayout = global.__termlabInitialLayout || {};
    const zenState = {
      active: global.__termlabInitialZenMode === true,
      leftVisible: initialLayout.files_panel_visible !== false,
      rightVisible: initialLayout.ssh_panel_visible !== false,
      bottomVisible: initialLayout.bottom_panel_visible !== false,
    };
    global.__termlabZenRestoreState = {
      leftVisible: !!zenState.leftVisible,
      rightVisible: !!zenState.rightVisible,
      bottomVisible: !!zenState.bottomVisible,
    };

    function handleMenuAction(action) {
      if (action === 'paste') {
        deps.pasteIntoCurrentPane();
        return;
      }
      if (action === 'copy') {
        const pane = getCurrentPane();
        const text = pane && pane.term ? pane.term.getSelection() : '';
        if (text) {
          invoke('clipboard_write_text', { text }).catch(() => {
            navigator.clipboard.writeText(text).catch(() => {});
          });
        } else if (isTextInputTarget(document.activeElement)) {
          document.execCommand('copy');
        }
        return;
      }
      if (action === 'cut') {
        if (isTextInputTarget(document.activeElement)) {
          document.execCommand('cut');
        }
        return;
      }
      if (action === 'select-all') {
        const active = document.activeElement;
        if (isTextInputTarget(active) && typeof active.select === 'function') {
          active.select();
        } else {
          const pane = getCurrentPane();
          if (pane && pane.term) pane.term.selectAll();
        }
        return;
      }
      if (action === 'new-tab') {
        createTab().catch((error) => showStatus('Failed to create tab: ' + String(error)));
        return;
      }
      if (action === 'new-file') {
        // An in-memory buffer: nothing is written until the first save, which
        // routes through the Save As chooser (editor-service's savePane).
        const service = global.termlabEditorService;
        if (service) service.openUntitled();
        return;
      }
      if (action === 'save-file') {
        // Scoped to editor panes: in a terminal this must not swallow the
        // keystroke, so shortcut-runtime declines the combo before it reaches
        // here and the service returns without acting if it is called anyway.
        const service = global.termlabEditorService;
        if (service) service.saveActiveEditor();
        return;
      }
      if (action === 'save-file-as') {
        // Scoped to editor panes exactly like save-file: shortcut-runtime
        // declines the combo outside an editor, and the check here is what
        // keeps the OTHER routes to this action (the File menu, the command
        // palette) from opening a save chooser with a terminal focused —
        // neither of those goes through that guard.
        const pane = getCurrentPane();
        if (!pane || pane.kind !== 'editor') return;
        const dialog = global.termlabFileDialog;
        if (!dialog) {
          showStatus('File dialog unavailable');
          return;
        }
        // Failures of the save itself are reported by the editor service,
        // which also owns leaving the pane on its old binding; a rejection
        // here only means the chooser could not be shown.
        Promise.resolve(dialog.openForSave(pane)).catch((error) => {
          showStatus('Failed to save file: ' + String(error));
        });
        return;
      }
      if (action === 'open-file') {
        // Unscoped on purpose (see shortcut-runtime.js): the chooser opens
        // from a terminal too. Routing and error reporting live in the
        // dialog; a rejection here only means it could not even be shown.
        const dialog = global.termlabFileDialog;
        if (!dialog) {
          showStatus('File dialog unavailable');
          return;
        }
        Promise.resolve(dialog.openForOpen()).catch((error) => {
          showStatus('Failed to open file: ' + String(error));
        });
        return;
      }
      if (action === 'new-plain-shell-tab') {
        createPlainShellTab().catch((error) => showStatus('Failed to create plain shell tab: ' + String(error)));
        return;
      }
      if (action === 'new-window') {
        invoke('open_new_window').catch((error) => showStatus('Failed to open window: ' + String(error)));
        return;
      }
      if (action === 'close-tab' && getActiveTabId() !== null) {
        closeTab(getActiveTabId()).catch((error) => showStatus('Failed to close tab: ' + String(error)));
        return;
      }
      if (action === 'toggle-left-panel' && global.toolWindowManager) {
        global.toolWindowManager.togglePanel('left');
        debouncedSaveLayout();
        return;
      }
      if (action === 'toggle-right-panel' && global.toolWindowManager) {
        global.toolWindowManager.togglePanel('right');
        debouncedSaveLayout();
        return;
      }
      if (action === 'focus-sessions' && global.sshPanel) {
        if (global.toolWindowManager) {
          if (!global.toolWindowManager.isPanelVisible('right')) {
            global.toolWindowManager.setPanelVisibility('right', true);
          }
          if (!global.toolWindowManager.isVisible('ssh-sessions')) {
            global.toolWindowManager.activate('ssh-sessions');
          }
        }
        global.sshPanel.focusQuickConnect();
        return;
      }
      if (action === 'settings') {
        if (global.settings) global.settings.open();
        return;
      }
      if (action === 'manage-tunnels' && global.tunnelManager) {
        global.tunnelManager.show();
        return;
      }
      if (action === 'open-command-palette') {
        if (isCommandPaletteOpen()) {
          closeCommandPalette();
        } else {
          openCommandPalette();
        }
        return;
      }
      if (action === 'vault-open' && global.vault) {
        global.vault.showVaultDialog();
        return;
      }
      if (action === 'vault-lock') {
        invoke('vault_lock').then(() => {
          global.toast.info('Vault Locked', 'Credential vault has been locked.');
        }).catch(() => {});
        return;
      }
      if (action === 'keygen-open' && global.keygen) {
        global.keygen.showKeygenDialog();
        return;
      }
      if (action === 'ssh-export' && global.sshPanel) {
        global.sshPanel.exportConfig();
        return;
      }
      if (action === 'ssh-import' && global.sshPanel) {
        global.sshPanel.importConfig();
        return;
      }
      if (action === 'zen-mode') {
        // An explicit toggle is a real preference: stop treating this window's
        // zen state as a session-only default so it persists again.
        window.__termlabZenIsSessionDefault = false;
        const appRoot = document.getElementById('app');

        if (!zenState.active) {
          zenState.leftVisible = global.toolWindowManager
            ? global.toolWindowManager.isPanelVisible('left')
            : !!(global.filesPanel && !global.filesPanel.isHidden());
          zenState.rightVisible = global.toolWindowManager
            ? global.toolWindowManager.isPanelVisible('right')
            : !!(global.sshPanel && !global.sshPanel.isHidden());
          zenState.bottomVisible = global.toolWindowManager
            ? global.toolWindowManager.isPanelVisible('bottom')
            : false;
          global.__termlabZenRestoreState = {
            leftVisible: !!zenState.leftVisible,
            rightVisible: !!zenState.rightVisible,
            bottomVisible: !!zenState.bottomVisible,
          };

          if (global.toolWindowManager) {
            if (zenState.leftVisible) global.toolWindowManager.setPanelVisibility('left', false);
            if (zenState.rightVisible) global.toolWindowManager.setPanelVisibility('right', false);
            if (zenState.bottomVisible) global.toolWindowManager.setPanelVisibility('bottom', false);
          } else {
            if (global.filesPanel && !global.filesPanel.isHidden()) global.filesPanel.togglePanel();
            if (global.sshPanel && !global.sshPanel.isHidden()) global.sshPanel.togglePanel();
          }
          if (appRoot) appRoot.classList.add('zen-mode');
          zenState.active = true;
        } else {
          // Drop the zen-mode class before restoring visibility: updateSidebar
          // and updateBottomZone both treat an active zen-mode class as "stay
          // hidden" regardless of the requested visibility, so calling
          // setPanelVisibility(..., true) while the class is still present
          // would silently no-op and leave that zone's chrome stuck hidden
          // (with its strip tab, whose 'active' class only tracks the
          // requested visibility, then rendering active over hidden content).
          if (appRoot) appRoot.classList.remove('zen-mode');
          if (global.toolWindowManager) {
            global.toolWindowManager.setPanelVisibility('left', !!zenState.leftVisible);
            global.toolWindowManager.setPanelVisibility('right', !!zenState.rightVisible);
            global.toolWindowManager.setPanelVisibility('bottom', !!zenState.bottomVisible);
          } else {
            if (global.filesPanel && zenState.leftVisible && global.filesPanel.isHidden()) global.filesPanel.togglePanel();
            if (global.filesPanel && !zenState.leftVisible && !global.filesPanel.isHidden()) global.filesPanel.togglePanel();
            if (global.sshPanel && zenState.rightVisible && global.sshPanel.isHidden()) global.sshPanel.togglePanel();
            if (global.sshPanel && !zenState.rightVisible && !global.sshPanel.isHidden()) global.sshPanel.togglePanel();
          }
          zenState.active = false;
          global.__termlabZenRestoreState = null;
        }
        debouncedSaveLayout();
        return;
      }
      if (action === 'zoom-in') {
        const nextZoom = Math.min(3.0, +(getZoom() + 0.1).toFixed(1));
        setZoom(nextZoom);
        invoke('set_zoom_level', { scaleFactor: nextZoom }).catch(() => {});
        return;
      }
      if (action === 'zoom-out') {
        const nextZoom = Math.max(0.5, +(getZoom() - 0.1).toFixed(1));
        setZoom(nextZoom);
        invoke('set_zoom_level', { scaleFactor: nextZoom }).catch(() => {});
        return;
      }
      if (action === 'zoom-reset') {
        setZoom(1.0);
        invoke('set_zoom_level', { scaleFactor: 1.0 }).catch(() => {});
        return;
      }
      if (action === 'split-vertical') {
        splitPane('vertical').catch((error) => showStatus('Split failed: ' + String(error)));
        return;
      }
      if (action === 'split-horizontal') {
        splitPane('horizontal').catch((error) => showStatus('Split failed: ' + String(error)));
        return;
      }
      if (action === 'close-pane' && getFocusedPaneId() != null) {
        closePane(getFocusedPaneId());
        return;
      }
      if (action === 'rename-tab') {
        renameActiveTab();
        return;
      }
      if (action === 'toggle-bottom-panel') {
        if (global.toolWindowManager) {
          global.toolWindowManager.togglePanel('bottom');
          debouncedSaveLayout();
        }
        return;
      }
      if (action === 'notifications' && global.toolWindowManager) {
        // Notifications moved out of the bottom panel into its own right-bottom
        // tool window (design-system Phase 2); this activates it the same way
        // 'focus-sessions' activates the Hosts tool window above.
        if (!global.toolWindowManager.isPanelVisible('right')) {
          global.toolWindowManager.setPanelVisibility('right', true);
        }
        if (!global.toolWindowManager.isVisible('notifications')) {
          global.toolWindowManager.activate('notifications');
        }
        return;
      }
      if (action === 'about') {
        showAboutDialog();
        return;
      }
      if (action === 'check-for-updates') {
        invoke('check_for_update').then((info) => {
          if (info) {
            showUpdateAvailableToast(info);
          } else {
            global.toast.info('Up to Date', "You're running the latest version.");
          }
        }).catch(() => {
          global.toast.warn('Update Check Failed', 'Unable to check for updates.');
        });
        return;
      }
      if (action === 'open-devtools') {
        invoke('open_devtools')
          .catch((error) => showStatus('Failed to open developer console: ' + String(error)));
      }
    }

    return {
      handleMenuAction,
    };
  }

  global.termlabMenuActions = {
    create,
  };
})(window);
