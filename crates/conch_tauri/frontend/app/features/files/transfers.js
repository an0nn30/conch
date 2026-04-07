(function initConchFilesTransfers(global) {
  'use strict';

  function createController(deps) {
    const d = deps || {};
    const activeTransferToasts = new Map();
    const formatSize = typeof d.formatSize === 'function'
      ? d.formatSize
      : ((value) => String(value || 0));
    const toastApi = d.toast || global.toast || {};

    function removeTransferToast(transferId) {
      const toast = activeTransferToasts.get(transferId);
      if (!toast) return;
      toast.classList.remove('visible');
      activeTransferToasts.delete(transferId);
      setTimeout(() => toast.remove(), 300);
    }

    function showCompletionToast(fileName, kind, error) {
      const arrow = kind === 'download' ? '\u2193' : '\u2191';
      if (error) {
        if (typeof toastApi.error === 'function') toastApi.error(`${arrow} Transfer Failed: ${fileName}`, error);
      } else if (typeof toastApi.success === 'function') {
        toastApi.success(`${arrow} Transfer Complete: ${fileName}`);
      }
    }

    function updateOrCreateTransferToast(progress) {
      let toast = activeTransferToasts.get(progress.transfer_id);

      if (!toast) {
        toast = document.createElement('div');
        toast.className = 'fp-progress-toast';
        toast.innerHTML = `
          <div class="fp-pt-header">
            <span class="fp-pt-kind">${progress.kind === 'download' ? '\u2193' : '\u2191'}</span>
            <span class="fp-pt-filename"></span>
            <button class="fp-pt-cancel" title="Cancel transfer">\u2715</button>
          </div>
          <div class="fp-pt-bar-wrap"><div class="fp-pt-bar"></div></div>
          <div class="fp-pt-details">
            <span class="fp-pt-bytes"></span>
            <span class="fp-pt-speed"></span>
          </div>
        `;
        toast.querySelector('.fp-pt-cancel').addEventListener('click', () => {
          if (typeof d.cancelTransfer === 'function') {
            Promise.resolve(d.cancelTransfer(progress.transfer_id)).catch(() => {});
          }
          removeTransferToast(progress.transfer_id);
        });
        toast._startTime = Date.now();
        toast._startBytes = 0;
        toast._lastBytes = 0;
        toast._lastTime = Date.now();

        let container = document.getElementById('toast-container');
        if (!container) {
          container = document.createElement('div');
          container.id = 'toast-container';
          document.body.appendChild(container);
        }
        container.appendChild(toast);
        requestAnimationFrame(() => toast.classList.add('visible'));
        activeTransferToasts.set(progress.transfer_id, toast);
      }

      toast.querySelector('.fp-pt-filename').textContent = progress.file_name;
      const pct = progress.total_bytes > 0
        ? Math.round((progress.bytes_transferred / progress.total_bytes) * 100)
        : 0;
      toast.querySelector('.fp-pt-bar').style.width = pct + '%';

      const bytesStr = formatSize(progress.bytes_transferred) + ' / ' + formatSize(progress.total_bytes);
      toast.querySelector('.fp-pt-bytes').textContent = bytesStr;

      const now = Date.now();
      const elapsed = (now - toast._lastTime) / 1000;
      if (elapsed > 0.05) {
        const bytesDelta = progress.bytes_transferred - toast._lastBytes;
        const speed = bytesDelta / elapsed;
        toast.querySelector('.fp-pt-speed').textContent = formatSize(Math.round(speed)) + '/s';
        toast._lastBytes = progress.bytes_transferred;
        toast._lastTime = now;
      }
    }

    function handleTransferProgress(event) {
      const progress = event && event.payload;
      if (!progress || !progress.transfer_id) return;

      const pane = progress.kind === 'download' ? d.localPane : d.remotePane;
      if (!pane) return;

      const pct = progress.total_bytes > 0
        ? Math.round((progress.bytes_transferred / progress.total_bytes) * 100)
        : 0;

      if (progress.status === 'completed') {
        removeTransferToast(progress.transfer_id);
        showCompletionToast(progress.file_name, progress.kind);
        pane.transferStatus[progress.file_name] = { status: 'completed', percent: 100 };
        if (typeof d.loadEntries === 'function') d.loadEntries(pane);
        return;
      }

      if (progress.status === 'failed' || progress.status === 'cancelled') {
        removeTransferToast(progress.transfer_id);
        delete pane.transferStatus[progress.file_name];
        if (progress.status === 'failed') {
          showCompletionToast(progress.file_name, progress.kind, progress.error);
        }
        return;
      }

      pane.transferStatus[progress.file_name] = { status: 'in_progress', percent: pct };
      updateOrCreateTransferToast(progress);
    }

    return {
      handleTransferProgress,
      removeTransferToast,
      showCompletionToast,
    };
  }

  global.conchFilesTransfers = {
    createController,
  };
})(window);
