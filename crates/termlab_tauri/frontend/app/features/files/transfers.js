(function initTermLabFilesTransfers(global) {
  'use strict';

  function createController(deps) {
    const d = deps || {};
    const terminalKinds = new Set(['completed', 'failed', 'cancelled']);
    const attentionKinds = new Set(['needsAttention', 'needsConnection']);
    let previousJobs = new Map();
    // Folder batches this panel started, still waiting for their expansion to
    // finish. See reportFinishedEmptyBatches below.
    const watchedBatches = new Set();

    function paneForDirection(direction) {
      return direction === 'download' ? d.localPane : d.remotePane;
    }

    function clearBadge(pane, fileName, transferId) {
      if (!pane || !pane.transferStatus) return false;
      const current = pane.transferStatus[fileName];
      if (!current
          || current.provisional
          || (current.transferId && current.transferId !== transferId)) return false;
      delete pane.transferStatus[fileName];
      return true;
    }

    function renderTransferStatus(pane) {
      if (typeof d.renderTransferStatus === 'function') d.renderTransferStatus(pane);
    }

    function transferProgressStatus(job, current) {
      const kind = job && job.state && job.state.kind;
      const direction = job && job.direction === 'download' ? 'download' : 'upload';
      const transferId = job && job.id;
      if (['queued', 'connecting', 'checking'].includes(kind)) {
        return { status: 'preparing', direction, transferId };
      }
      if (kind === 'retryWaiting') {
        return { status: 'waiting', direction, transferId };
      }
      if (kind === 'paused') {
        return { status: 'paused', direction, transferId };
      }

      const bytes = Number(job && job.bytesTransferred) || 0;
      const total = Number(job && job.totalBytes) || 0;
      if (bytes > 0 && total > 0) {
        return {
          status: 'in_progress',
          direction,
          percent: Math.min(100, (bytes / total) * 100),
          transferId,
        };
      }
      if (current && current.transferId === transferId && Number(current.percent) > 0) {
        return { ...current, direction };
      }
      return { status: 'starting', direction, transferId };
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

      const current = pane.transferStatus[progress.file_name];
      if (current
          && (current.provisional
            || (current.transferId && current.transferId !== progress.transfer_id))) return;
      if (Number(progress.bytes_transferred) <= 0) {
        if (!current) {
          pane.transferStatus[progress.file_name] = {
            status: 'starting',
            direction: progress.kind === 'download' ? 'download' : 'upload',
            transferId: progress.transfer_id,
          };
          renderTransferStatus(pane);
        }
        return;
      }

      const pct = progress.total_bytes > 0
        ? Math.min(100, (progress.bytes_transferred / progress.total_bytes) * 100)
        : 0;

      pane.transferStatus[progress.file_name] = {
        status: 'in_progress',
        direction: progress.kind === 'download' ? 'download' : 'upload',
        percent: pct,
        transferId: progress.transfer_id,
      };
      renderTransferStatus(pane);
    }

    // A folder whose tree holds no files produces a batch with no member
    // jobs at all. Every other completion notice — the runtime's batched
    // "Transfers complete" toast, the pane badges above — is aggregated from
    // member-job transitions, so zero members means the whole transfer ends
    // in silence even though the destination directory was created. This is
    // the one-shot notice for that case, driven off the batches projection
    // rather than the job list.
    function watchFolderBatch(batchId) {
      if (!batchId) return;
      watchedBatches.add(String(batchId));
      const runtime = d.transferRuntime;
      if (runtime && typeof runtime.getSnapshot === 'function') {
        // The expansion may already have finished by the time the command
        // that created the batch resolved, in which case no further snapshot
        // is coming — evaluate what is already known.
        reportFinishedEmptyBatches(runtime.getSnapshot());
      }
    }

    function reportFinishedEmptyBatches(snapshot) {
      if (watchedBatches.size === 0) return;
      const batches = Array.isArray(snapshot && snapshot.batches) ? snapshot.batches : [];
      for (const aggregate of batches) {
        const info = aggregate && aggregate.info;
        const id = info && info.id ? String(info.id) : '';
        if (!id || !watchedBatches.has(id)) continue;
        const expansion = info.expansion && info.expansion.kind;
        if (expansion !== 'complete' && expansion !== 'interrupted') continue;
        // Terminal either way: an interrupted walk is reported through its
        // own header marker, so it just stops being watched.
        watchedBatches.delete(id);
        if (expansion !== 'complete' || Number(info.discoveredFiles) !== 0) continue;
        if (d.toast && typeof d.toast.info === 'function') {
          d.toast.info(
            'Folder created',
            `${info.name || 'The folder'} contained no files to transfer.`,
          );
        }
      }
    }

    function handleTransferSnapshot(snapshot) {
      reportFinishedEmptyBatches(snapshot);
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
          const newlyObserved = !previousJobs.has(job.id);
          if (!current
              || (current.provisional && newlyObserved)
              || current.transferId === job.id) {
            pane.transferStatus[job.fileName] = transferProgressStatus(job, current);
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
      watchFolderBatch,
    };
  }

  global.termlabFilesTransfers = {
    createController,
  };
})(window);
