// FileZilla-style auto-open: activate the Transfer Center when a NEW
// files-panel transfer is enqueued, and only then. The subscriber's first
// snapshot is the baseline — restored jobs from a previous run never summon
// the panel (the queue's no-surprise-activity-on-restart rule extends to the
// UI). Tracking is per batch, so a multi-file transfer is one activation and
// a panel the user hid mid-batch stays hidden until a genuinely new transfer.
(function initTermLabTransferAutoOpen(global) {
  'use strict';

  const TERMINAL_KINDS = new Set(['completed', 'failed', 'cancelled']);
  const DEFAULT_WINDOW_ID = 'transfer-center';

  function init(options) {
    const runtime = options && options.runtime;
    const manager = options && options.toolWindowManager;
    const windowId = (options && options.windowId) || DEFAULT_WINDOW_ID;
    if (!runtime || typeof runtime.subscribe !== 'function') {
      throw new TypeError('Transfer auto-open requires the transfer runtime');
    }
    if (!manager || typeof manager.activate !== 'function') {
      throw new TypeError('Transfer auto-open requires the tool window manager');
    }

    const seenKeys = new Set();
    let baselined = false;

    const unsubscribe = runtime.subscribe((snapshot) => {
      const jobs = snapshot && Array.isArray(snapshot.jobs) ? snapshot.jobs : [];
      let shouldActivate = false;
      for (const job of jobs) {
        if (!job || !job.id) continue;
        const key = job.batchId || job.id;
        if (seenKeys.has(key)) continue;
        seenKeys.add(key);
        if (baselined
            && job.origin && job.origin.kind === 'filesPanel'
            && job.state && !TERMINAL_KINDS.has(job.state.kind)) {
          shouldActivate = true;
        }
      }
      baselined = true;
      if (shouldActivate) manager.activate(windowId);
    });

    return { dispose: unsubscribe };
  }

  global.termlabTransferAutoOpen = { init };
})(window);
