(function initTermLabFilesTransfers(global) {
  'use strict';

  function createController(deps) {
    const d = deps || {};
    const terminalKinds = new Set(['completed', 'failed', 'cancelled']);
    const attentionKinds = new Set(['needsAttention', 'needsConnection']);
    let previousJobs = new Map();

    function paneForDirection(direction) {
      return direction === 'download' ? d.localPane : d.remotePane;
    }

    function clearBadge(pane, fileName, transferId) {
      if (!pane || !pane.transferStatus) return false;
      const current = pane.transferStatus[fileName];
      if (!current || (current.transferId && current.transferId !== transferId)) return false;
      delete pane.transferStatus[fileName];
      return true;
    }

    function renderTransferStatus(pane) {
      if (typeof d.renderTransferStatus === 'function') d.renderTransferStatus(pane);
    }

    function reportCommandError(error) {
      if (d.toast && typeof d.toast.error === 'function') {
        d.toast.error(
          'Transfer action failed',
          error && error.message ? error.message : String(error),
        );
      }
    }

    function acknowledge(result) {
      return Promise.resolve(result).catch(reportCommandError);
    }

    function handleTransferAttention(transferId, invoker) {
      const job = previousJobs.get(transferId);
      const kind = job && job.state && job.state.kind;
      const runtime = d.transferRuntime;
      const dialogs = d.transferDialogs;
      if (!job || !runtime) return false;

      if (kind === 'needsAttention'
          && dialogs
          && typeof dialogs.showConflict === 'function'
          && typeof runtime.resolve === 'function') {
        dialogs.showConflict(job, invoker, (resolution) => (
          acknowledge(runtime.resolve(job.id, resolution))
        ));
        return true;
      }

      if (kind === 'needsConnection') {
        if (job.endpoint
            && job.endpoint.kind === 'configured'
            && typeof runtime.reconnect === 'function') {
          acknowledge(runtime.reconnect(job));
          return true;
        }
        if (dialogs
            && typeof dialogs.showAdHocReconnect === 'function'
            && typeof runtime.retry === 'function') {
          dialogs.showAdHocReconnect(job, invoker, () => acknowledge(runtime.retry(job.id)));
          return true;
        }
      }
      return false;
    }

    // Legacy byte progress remains the compatibility source for percentages.
    // Lifecycle notifications and committed completion come from the durable
    // runtime snapshot below, so this path never creates a toast or refreshes
    // a directory from an event that can race the final handle close.
    function handleTransferProgress(event) {
      const progress = event && event.payload;
      if (!progress || !progress.transfer_id) return;

      const pane = paneForDirection(progress.kind);
      if (!pane) return;

      if (terminalKinds.has(progress.status)) {
        if (clearBadge(pane, progress.file_name, progress.transfer_id)) {
          renderTransferStatus(pane);
        }
        return;
      }
      if (progress.status !== 'in_progress') return;

      const pct = progress.total_bytes > 0
        ? Math.round((progress.bytes_transferred / progress.total_bytes) * 100)
        : 0;

      pane.transferStatus[progress.file_name] = {
        status: 'in_progress',
        percent: pct,
        transferId: progress.transfer_id,
      };
    }

    function handleTransferSnapshot(snapshot) {
      const jobs = Array.isArray(snapshot && snapshot.jobs) ? snapshot.jobs : [];
      const currentJobs = new Map();
      const panesToRender = new Set();
      const attentionToOpen = [];
      let refreshAfterCommit = false;

      for (const job of jobs) {
        if (!job || !job.id || !job.origin || job.origin.kind !== 'filesPanel') continue;
        currentJobs.set(job.id, job);
        const kind = job.state && job.state.kind;
        const pane = paneForDirection(job.direction);
        if (!pane || !job.fileName) continue;

        if (attentionKinds.has(kind)) {
          pane.transferStatus[job.fileName] = {
            status: 'attention',
            percent: 0,
            transferId: job.id,
          };
          panesToRender.add(pane);
        } else if (terminalKinds.has(kind)) {
          if (clearBadge(pane, job.fileName, job.id)) panesToRender.add(pane);
        } else {
          const current = pane.transferStatus[job.fileName];
          if (!current || current.transferId === job.id) {
            pane.transferStatus[job.fileName] = {
              status: 'in_progress',
              percent: current && Number(current.percent) ? Number(current.percent) : 0,
              transferId: job.id,
            };
            panesToRender.add(pane);
          }
        }

        const previous = previousJobs.get(job.id);
        const previousKind = previous && previous.state && previous.state.kind;
        if (kind === 'needsAttention' && previousKind && previousKind !== 'needsAttention') {
          attentionToOpen.push(job.id);
        }
        if (kind === 'completed' && previousKind && previousKind !== 'completed') {
          refreshAfterCommit = true;
        }
      }

      // History compaction/removal must not leave a badge behind.
      for (const [id, job] of previousJobs) {
        if (currentJobs.has(id)) continue;
        const pane = paneForDirection(job.direction);
        if (pane && clearBadge(pane, job.fileName, id)) panesToRender.add(pane);
      }
      previousJobs = currentJobs;
      attentionToOpen.forEach((transferId) => handleTransferAttention(transferId, null));

      if (refreshAfterCommit && typeof d.loadEntries === 'function') {
        if (d.localPane) d.loadEntries(d.localPane);
        if (d.remotePane && d.remotePane !== d.localPane) d.loadEntries(d.remotePane);
        return;
      }
      panesToRender.forEach(renderTransferStatus);
    }

    return {
      handleTransferProgress,
      handleTransferSnapshot,
      handleTransferAttention,
    };
  }

  global.termlabFilesTransfers = {
    createController,
  };
})(window);
