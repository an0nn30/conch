(function initTermLabTransferStore(global) {
  'use strict';

  const TERMINAL_KINDS = new Set(['completed', 'failed', 'cancelled']);

  function clone(value) {
    if (typeof global.structuredClone === 'function') return global.structuredClone(value);
    return JSON.parse(JSON.stringify(value));
  }

  // Queue schema v1 persists its original numeric speed field. Keep the
  // unknown sentinel at the frontend ingress boundary so view consumers get
  // the nullable projection promised by the generated interface.
  function projectJob(job) {
    const projected = clone(job);
    if (projected && projected.speedBytesPerSecond === 0) {
      projected.speedBytesPerSecond = null;
    }
    if (projected) delete projected.commitBackupExpected;
    return projected;
  }

  function projectSnapshot(snapshot) {
    const projected = clone(snapshot);
    projected.jobs = Array.isArray(projected.jobs) ? projected.jobs.map(projectJob) : [];
    return projected;
  }

  function emptySnapshot() {
    return {
      revision: 0,
      queuePaused: true,
      settings: { globalLimit: 3, perHostLimit: 2 },
      jobs: [],
      summary: {
        queued: 0,
        running: 0,
        paused: 0,
        attention: 0,
        failed: 0,
        active: 0,
        history: 0,
        queuePaused: true,
      },
      recoveryError: null,
    };
  }

  function create() {
    let state = emptySnapshot();

    function hydrate(snapshot) {
      state = projectSnapshot(snapshot);
    }

    function getSnapshot() {
      return clone(state);
    }

    function applyJobEvent(event) {
      if (!event || event.revision <= state.revision) return { needsRefresh: false };
      if (event.revision !== state.revision + 1) return { needsRefresh: true };

      const removed = new Set(Array.isArray(event.removedIds) ? event.removedIds : []);
      const jobs = state.jobs.filter((job) => !removed.has(job.id));
      for (const incoming of Array.isArray(event.upserts) ? event.upserts : []) {
        const index = jobs.findIndex((job) => job.id === incoming.id);
        if (index === -1) jobs.push(projectJob(incoming));
        else jobs[index] = projectJob(incoming);
      }

      state = {
        ...state,
        revision: event.revision,
        jobs,
        queuePaused: !!event.queuePaused,
        settings: clone(event.settings),
        summary: {
          ...state.summary,
          queuePaused: !!event.queuePaused,
        },
      };
      return { needsRefresh: false };
    }

    function applySummaryEvent(event) {
      if (!event || event.revision < state.revision) return { needsRefresh: false };
      if (event.revision > state.revision) return { needsRefresh: true };
      state = {
        ...state,
        summary: clone(event.summary),
      };
      return { needsRefresh: false };
    }

    function jobsByTerminalState(terminal) {
      return clone(state.jobs.filter((job) => {
        const kind = job && job.state ? job.state.kind : '';
        return TERMINAL_KINDS.has(kind) === terminal;
      }));
    }

    return {
      hydrate,
      getSnapshot,
      applyJobEvent,
      applySummaryEvent,
      activeJobs: () => jobsByTerminalState(false),
      historyJobs: () => jobsByTerminalState(true),
    };
  }

  global.termlabTransferStore = { create };
})(window);
