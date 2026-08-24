(function initTransferCenterPanel(global) {
  'use strict';

  const TERMINAL_KINDS = new Set(['completed', 'failed', 'cancelled']);
  const ACTIONS_BY_KIND = {
    running: new Set(['pause', 'cancel']),
    paused: new Set(['resume', 'cancel']),
    failed: new Set(['retry', 'details']),
    needsConnection: new Set(['connect', 'cancel']),
    needsAttention: new Set(['resolve', 'cancel']),
    queued: new Set(['pause', 'toggle-priority', 'move-up', 'move-down', 'cancel']),
    completed: new Set(['details']),
    cancelled: new Set(['details']),
  };

  function visibleJobs(snapshot, viewMode) {
    const history = viewMode === 'history';
    return snapshot.jobs.filter((job) => {
      const kind = job && job.state ? job.state.kind : '';
      return TERMINAL_KINDS.has(kind) === history;
    });
  }

  function init(options) {
    const opts = options || {};
    const panelEl = opts.panelEl;
    const runtime = global.termlabTransferRuntime;
    const viewFactory = global.termlabTransferCenterView;
    const dialogs = global.termlabTransferDialogs;
    if (!panelEl) throw new Error('Transfer Center panel requires panelEl');
    if (!runtime || typeof runtime.subscribe !== 'function') {
      throw new Error('Transfer Center panel requires the shared transfer runtime');
    }
    if (!viewFactory || typeof viewFactory.create !== 'function') {
      throw new Error('Transfer Center view must load before its panel');
    }

    let latestSnapshot = null;
    let viewMode = 'active';
    let selectedId = null;

    function reconcileSelection() {
      if (!latestSnapshot) return;
      const jobs = visibleJobs(latestSnapshot, viewMode);
      if (!jobs.some((job) => job.id === selectedId)) selectedId = jobs.length > 0 ? jobs[0].id : null;
    }

    function jobById(jobId) {
      return latestSnapshot && latestSnapshot.jobs.find((job) => job.id === jobId);
    }

    function reportCommandError(error) {
      const toast = opts.toast || global.toast;
      if (toast && typeof toast.error === 'function') {
        toast.error('Transfer action failed', error && error.message ? error.message : String(error));
      }
    }

    function acknowledge(result) {
      return Promise.resolve(result).catch(reportCommandError);
    }

    function queuedJobs() {
      if (!latestSnapshot) return [];
      return latestSnapshot.jobs
        .filter((job) => job && job.state && job.state.kind === 'queued')
        .slice()
        .sort((left, right) => (Number(left.queueOrder) || 0) - (Number(right.queueOrder) || 0));
    }

    function dispatchAction(event) {
      const action = event && event.action;
      if (action === 'pause-all') return acknowledge(runtime.pauseAll());
      if (action === 'resume-all') return acknowledge(runtime.resumeAll());
      if (action === 'clear-completed') return acknowledge(runtime.clearCompleted());
      if (action === 'concurrency') {
        if (dialogs && typeof dialogs.showConcurrency === 'function') {
          dialogs.showConcurrency(latestSnapshot && latestSnapshot.settings, event.invoker, (settings) => (
            acknowledge(runtime.updateSettings(settings))
          ));
        }
        return undefined;
      }

      const job = jobById(event && event.jobId);
      const kind = job && job.state ? job.state.kind : '';
      if (!job || !ACTIONS_BY_KIND[kind] || !ACTIONS_BY_KIND[kind].has(action)) return undefined;

      if (action === 'pause') return acknowledge(runtime.pause(job.id));
      if (action === 'resume') return acknowledge(runtime.resume(job.id));
      if (action === 'retry') return acknowledge(runtime.retry(job.id));
      if (action === 'toggle-priority') {
        return acknowledge(runtime.setPriority(job.id, job.priority === 'interactive' ? 'normal' : 'interactive'));
      }
      if (action === 'move-up' || action === 'move-down') {
        const queue = queuedJobs();
        const index = queue.findIndex((item) => item.id === job.id);
        if (index < 0) return undefined;
        if (action === 'move-up') {
          if (index === 0) return undefined;
          return acknowledge(runtime.reorder(job.id, queue[index - 1].id));
        }
        if (index === queue.length - 1) return undefined;
        const before = index + 2 < queue.length ? queue[index + 2].id : null;
        return acknowledge(runtime.reorder(job.id, before));
      }
      if (action === 'cancel') {
        if (dialogs && typeof dialogs.showCancel === 'function') {
          dialogs.showCancel(job, event.invoker, () => acknowledge(runtime.cancel(job.id)));
        }
        return undefined;
      }
      if (action === 'details') {
        if (dialogs && typeof dialogs.showDetails === 'function') dialogs.showDetails(job, event.invoker);
        return undefined;
      }
      if (action === 'resolve') {
        if (dialogs && typeof dialogs.showConflict === 'function') {
          dialogs.showConflict(job, event.invoker, (resolution) => acknowledge(runtime.resolve(job.id, resolution)));
        }
        return undefined;
      }
      if (action === 'connect') {
        if (job.endpoint && job.endpoint.kind === 'configured' && typeof runtime.reconnect === 'function') {
          return acknowledge(runtime.reconnect(job));
        }
        if (dialogs && typeof dialogs.showAdHocReconnect === 'function') {
          dialogs.showAdHocReconnect(job, event.invoker, () => acknowledge(runtime.retry(job.id)));
        }
      }
      return undefined;
    }

    const view = viewFactory.create({
      panelEl,
      formatSize: global.utils && global.utils.formatSize,
      onViewChange(nextMode) {
        if (nextMode !== 'active' && nextMode !== 'history') return;
        viewMode = nextMode;
        reconcileSelection();
        view.render(latestSnapshot, { viewMode, selectedId });
      },
      onSelect(jobId) {
        selectedId = jobId || null;
        view.setSelection(selectedId);
        if (typeof opts.onSelect === 'function') opts.onSelect(selectedId);
      },
      onAction(action) {
        dispatchAction(action);
        if (typeof opts.onAction === 'function') opts.onAction(action);
      },
    });

    view.render(null, { viewMode, selectedId });
    const unsubscribe = runtime.subscribe((snapshot) => {
      latestSnapshot = snapshot;
      reconcileSelection();
      view.render(latestSnapshot, { viewMode, selectedId });
    });

    return {
      view,
      getState: () => ({ viewMode, selectedId }),
      destroy() {
        if (typeof unsubscribe === 'function') unsubscribe();
        view.destroy();
      },
    };
  }

  global.transferCenterPanel = { init };
})(window);
