(function initTermLabWindowEventsRuntime(global) {
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
      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return;

      let handle = null;
      let dismissed = false;
      const dismiss = () => {
        if (dismissed) return;
        dismissed = true;
        if (handle) handle.close();
      };

      handle = global.tlDialog.open({
        title: 'Update Ready',
        ariaLabel: 'Restart to apply update',
        size: 'sm',
        body: (bodyEl) => {
          bodyEl.innerHTML = `<div style="color:var(--fg);font-size:13px">The update has been installed. Restart now to apply?</div>`;
        },
        buttons: [
          { label: 'Restart Later', onSelect: dismiss },
          { label: 'Restart Now', primary: true, onSelect: () => {
            dismiss();
            invoke('restart_app');
          } },
        ],
        onClose: dismiss,
      });
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
        body: 'TermLab v' + esc(info.version) + ' is available.',
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
        const body = updateProgressToast.querySelector('.termlab-toast-body');
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

  global.termlabWindowEventsRuntime = {
    create,
  };
})(window);
