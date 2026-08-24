(function initTermLabTransferStore(global) {
  'use strict';

  const TERMINAL_KINDS = new Set(['completed', 'failed', 'cancelled']);

  function clone(value) {
    if (typeof global.structuredClone === 'function') return global.structuredClone(value);
    return JSON.parse(JSON.stringify(value));
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
      state = clone(snapshot);
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
        if (index === -1) jobs.push(clone(incoming));
        else jobs[index] = clone(incoming);
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
