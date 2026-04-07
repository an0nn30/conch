(function initConchWindowEventsRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const listen = deps.listen;
    const currentWindowLabel = deps.currentWindowLabel;
    const getPanes = deps.getPanes;
    const closePane = deps.closePane;
    const refreshSshSessions = deps.refreshSshSessions;
    const esc = deps.esc;

    let updateProgressToast = null;

    function showRestartDialog() {
      const overlay = document.createElement('div');
      overlay.className = 'ssh-overlay';
      overlay.id = 'update-restart-overlay';
      overlay.setAttribute('role', 'dialog');
      overlay.setAttribute('aria-modal', 'true');
      overlay.setAttribute('aria-label', 'Restart to apply update');

      const dialog = document.createElement('div');
      dialog.className = 'ssh-form';
      dialog.style.width = '400px';

      const title = document.createElement('div');
      title.className = 'ssh-form-title';
      title.textContent = 'Update Ready';
      dialog.appendChild(title);

      const msg = document.createElement('div');
      msg.style.cssText = 'padding:16px 20px;color:var(--fg);font-size:13px';
      msg.textContent = 'The update has been installed. Restart now to apply?';
      dialog.appendChild(msg);

      const buttons = document.createElement('div');
      buttons.className = 'ssh-form-buttons';

      const laterBtn = document.createElement('button');
      laterBtn.className = 'ssh-form-btn';
      laterBtn.textContent = 'Restart Later';
      laterBtn.addEventListener('click', () => dismiss());

      const restartBtn = document.createElement('button');
      restartBtn.className = 'ssh-form-btn primary';
      restartBtn.textContent = 'Restart Now';
      restartBtn.addEventListener('click', () => {
        dismiss();
        invoke('restart_app');
      });

      buttons.appendChild(laterBtn);
      buttons.appendChild(restartBtn);
      dialog.appendChild(buttons);

      let unregisterEscape = null;
      const dismiss = () => {
        if (typeof unregisterEscape === 'function') unregisterEscape();
        unregisterEscape = null;
        overlay.remove();
      };

      overlay.appendChild(dialog);
      overlay.addEventListener('mousedown', (event) => { if (event.target === overlay) dismiss(); });
      const keyboardRouter = global.conchKeyboardRouter;
      if (keyboardRouter && typeof keyboardRouter.register === 'function') {
        unregisterEscape = keyboardRouter.register({
          name: 'update-restart-dialog',
          priority: 220,
          isActive: () => !!overlay.isConnected,
          onKeyDown: (event) => {
            if (event.key !== 'Escape') return false;
            dismiss();
            return true;
          },
        });
      }
      document.body.appendChild(overlay);
    }

    async function startUpdate() {
      updateProgressToast = global.toast.show({
        level: 'info',
        title: 'Updating',
        body: 'Downloading update...',
        duration: 0,
      });

      try {
        await invoke('install_update');
        if (updateProgressToast) {
          global.toast.dismiss(updateProgressToast);
          updateProgressToast = null;
        }
        showRestartDialog();
      } catch (error) {
        if (updateProgressToast) {
          global.toast.dismiss(updateProgressToast);
          updateProgressToast = null;
        }
        global.toast.error('Update Failed', String(error));
      }
    }

    function showUpdateAvailableToast(info) {
      global.toast.show({
        level: 'info',
        title: 'Update Available',
        body: 'Conch v' + esc(info.version) + ' is available.',
        duration: 0,
        action: {
          label: 'Update Now',
          callback: () => startUpdate(),
        },
      });
    }

    async function init() {
      await listenOnCurrentWindow('pty-output', (event) => {
        const payload = event.payload || {};
        const windowLabel = payload.window_label;
        const paneId = payload.pane_id;
        const data = payload.data;
        if (typeof windowLabel !== 'string' || windowLabel !== currentWindowLabel) return;
        if (typeof paneId !== 'number' || typeof data !== 'string') return;
        const pane = getPanes().get(paneId);
        if (pane && pane.kind === 'terminal' && pane.term) pane.term.write(data);
      });

      await listenOnCurrentWindow('pty-exit', (event) => {
        const payload = event.payload || {};
        const windowLabel = payload.window_label;
        const paneId = payload.pane_id;
        if (typeof windowLabel !== 'string' || windowLabel !== currentWindowLabel) return;
        if (typeof paneId !== 'number') return;
        const pane = getPanes().get(paneId);
        if (!pane || pane.kind !== 'terminal') return;
        pane.spawned = false;
        closePane(paneId);
        refreshSshSessions();
      });

      await listen('update-available', (event) => {
        const info = event.payload;
        if (!info || !info.version) return;
        showUpdateAvailableToast(info);
      });

      await listen('update-progress', (event) => {
        if (!updateProgressToast) return;
        const p = event.payload;
        const body = updateProgressToast.querySelector('.conch-toast-body');
        if (body && p.total) {
          const pct = Math.round((p.downloaded / p.total) * 100);
          body.textContent = 'Downloading update... ' + pct + '%';
        }
      });

      return {
        showUpdateAvailableToast,
      };
    }

    return {
      init,
      showUpdateAvailableToast,
    };
  }

  global.conchWindowEventsRuntime = {
    create,
  };
})(window);
