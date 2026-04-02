(function initConchMenuActions(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const getCurrentPane = deps.getCurrentPane;
    const isTextInputTarget = deps.isTextInputTarget;
    const createTab = deps.createTab;
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
      if (action === 'new-window') {
        invoke('open_new_window').catch((error) => showStatus('Failed to open window: ' + String(error)));
        return;
      }
      if (action === 'close-tab' && getActiveTabId() !== null) {
        closeTab(getActiveTabId()).catch((error) => showStatus('Failed to close tab: ' + String(error)));
        return;
      }
      if (action === 'toggle-left-panel' && global.toolWindowManager) {
        global.toolWindowManager.toggle('file-explorer');
        debouncedSaveLayout();
        return;
      }
      if (action === 'toggle-right-panel' && global.toolWindowManager) {
        global.toolWindowManager.toggle('ssh-sessions');
        debouncedSaveLayout();
        return;
      }
      if (action === 'focus-sessions' && global.sshPanel) {
        if (global.toolWindowManager && !global.toolWindowManager.isVisible('ssh-sessions')) {
          global.toolWindowManager.activate('ssh-sessions');
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
        const filesHidden = global.filesPanel && global.filesPanel.isHidden();
        const sshHidden = global.sshPanel && global.sshPanel.isHidden();
        const allHidden = filesHidden && sshHidden;
        if (allHidden) {
          if (global.filesPanel) global.filesPanel.togglePanel();
          if (global.sshPanel) global.sshPanel.togglePanel();
        } else {
          if (global.filesPanel && !global.filesPanel.isHidden()) global.filesPanel.togglePanel();
          if (global.sshPanel && !global.sshPanel.isHidden()) global.sshPanel.togglePanel();
        }
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
        const bp = document.getElementById('bottom-panel');
        if (bp) {
          bp.classList.toggle('hidden');
          setTimeout(() => fitAndResizeCurrentTab(), 50);
          debouncedSaveLayout();
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

  global.conchMenuActions = {
    create,
  };
})(window);
