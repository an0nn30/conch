(function initTransferCenterPanel(global) {
  'use strict';

  const TERMINAL_KINDS = new Set(['completed', 'failed', 'cancelled']);

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
        // Task 11 exposes one delegated action seam but deliberately does not
        // mutate queue lifecycle. Task 12 binds runtime commands and dialogs.
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
