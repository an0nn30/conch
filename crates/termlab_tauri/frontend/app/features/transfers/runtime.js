(function initTermLabTransferRuntime(global) {
  'use strict';

  const JOB_EVENT = 'transfer-job-updated';
  const SUMMARY_EVENT = 'transfer-queue-summary';
  const TERMINAL_KINDS = new Set(['completed', 'failed', 'cancelled']);
  const ATTENTION_KINDS = new Set(['needsAttention', 'needsConnection']);
  const RESOLUTION_KINDS = new Set(['resume', 'overwrite', 'rename', 'skip', 'restart']);
  const TOAST_WINDOW_MS = 300;

  const storeFactory = global.termlabTransferStore;
  const dataService = global.termlabTransferDataService;
  if (!storeFactory || typeof storeFactory.create !== 'function') {
    throw new Error('Transfer store must load before transfer runtime');
  }
  if (!dataService) throw new Error('Transfer data service must load before transfer runtime');

  const store = storeFactory.create();
  const subscribers = new Set();
  const bufferedEvents = [];
  const toastBatches = new Map();
  let dependencies = null;
  let startupPromise = null;
  let refreshPromise = null;
  let hydrated = false;
  let eventSequence = 0;

  function notify() {
    for (const listener of Array.from(subscribers)) {
      try {
        listener(store.getSnapshot());
      } catch (error) {
        console.error('transfer runtime subscriber failed', error);
      }
    }
  }

  function subscribe(listener) {
    if (typeof listener !== 'function') throw new TypeError('transfer subscriber must be a function');
    subscribers.add(listener);
    listener(store.getSnapshot());
    let active = true;
    return () => {
      if (!active) return;
      active = false;
      subscribers.delete(listener);
    };
  }

  function enqueueEvent(kind, event) {
    const payload = event && Object.prototype.hasOwnProperty.call(event, 'payload')
      ? event.payload
      : event;
    if (!payload || typeof payload.revision !== 'number') return;
    bufferedEvents.push({ kind, payload, sequence: eventSequence++ });
  }

  function plural(count, singular, pluralForm) {
    return `${count} ${count === 1 ? singular : pluralForm}`;
  }

  function flushToastBatch(key) {
    const batch = toastBatches.get(key);
    if (!batch) return;
    toastBatches.delete(key);
    const toast = dependencies && dependencies.toast;
    if (!toast) return;

    if (batch.failed > 0 && typeof toast.error === 'function') {
      toast.error('Transfer batch failed', `${plural(batch.failed, 'transfer failed', 'transfers failed')}. Open Transfers for details.`);
      return;
    }
    if (batch.attention > 0 && typeof toast.warn === 'function') {
      toast.warn('Transfer batch needs attention', `${plural(batch.attention, 'transfer needs', 'transfers need')} attention. Open Transfers for details.`);
      return;
    }
    if (batch.completed > 0 && typeof toast.success === 'function') {
      toast.success('Transfers complete', `${plural(batch.completed, 'transfer completed', 'transfers completed')}.`);
      return;
    }
    if (batch.cancelled > 0 && typeof toast.info === 'function') {
      toast.info('Transfers cancelled', `${plural(batch.cancelled, 'transfer was', 'transfers were')} cancelled.`);
    }
  }

  function aggregateTransition(job, previousKind) {
    const nextKind = job && job.state ? job.state.kind : '';
    const enteredTerminal = TERMINAL_KINDS.has(nextKind) && !TERMINAL_KINDS.has(previousKind);
    const enteredAttention = ATTENTION_KINDS.has(nextKind) && !ATTENTION_KINDS.has(previousKind);
    if (!enteredTerminal && !enteredAttention) return;

    const key = job.batchId || job.id;
    if (!key) return;
    let batch = toastBatches.get(key);
    if (!batch) {
      batch = { completed: 0, failed: 0, cancelled: 0, attention: 0 };
      toastBatches.set(key, batch);
      setTimeout(() => flushToastBatch(key), TOAST_WINDOW_MS);
    }
    if (nextKind === 'completed') batch.completed += 1;
    else if (nextKind === 'failed') batch.failed += 1;
    else if (nextKind === 'cancelled') batch.cancelled += 1;
    else batch.attention += 1;
  }

  function applyJobEvent(payload) {
    const before = store.getSnapshot();
    const previousKinds = new Map(before.jobs.map((job) => [job.id, job.state && job.state.kind]));
    const result = store.applyJobEvent(payload);
    if (result.needsRefresh) return result;
    if (payload.revision <= before.revision) return result;
    for (const job of Array.isArray(payload.upserts) ? payload.upserts : []) {
      aggregateTransition(job, previousKinds.get(job.id));
    }
    notify();
    return result;
  }

  function applySummaryEvent(payload) {
    const before = store.getSnapshot();
    const result = store.applySummaryEvent(payload);
    if (!result.needsRefresh && payload.revision === before.revision) notify();
    return result;
  }

  function sortBuffered(events) {
    return events.sort((left, right) => {
      if (left.payload.revision !== right.payload.revision) {
        return left.payload.revision - right.payload.revision;
      }
      if (left.kind !== right.kind) return left.kind === 'job' ? -1 : 1;
      return left.sequence - right.sequence;
    });
  }

  function replayBuffered() {
    const events = sortBuffered(bufferedEvents.splice(0));
    for (let index = 0; index < events.length; index += 1) {
      const item = events[index];
      const result = item.kind === 'job'
        ? applyJobEvent(item.payload)
        : applySummaryEvent(item.payload);
      if (result.needsRefresh) {
        bufferedEvents.push(...events.slice(index));
        return false;
      }
    }
    return true;
  }

  function synchronize(options) {
    if (refreshPromise) return refreshPromise;
    if (!dependencies) return Promise.reject(new Error('Transfer runtime has not started'));
    const allowFollowup = !options || options.allowFollowup !== false;
    let needsFollowup = false;

    refreshPromise = (async () => {
      const snapshot = await dataService.snapshot(dependencies.invoke);
      const current = store.getSnapshot();
      if (!hydrated || snapshot.revision > current.revision) {
        store.hydrate(snapshot);
        hydrated = true;
        notify();
      }
      needsFollowup = !replayBuffered();
      return store.getSnapshot();
    })().finally(() => {
      refreshPromise = null;
      if (needsFollowup && allowFollowup) {
        synchronize({ allowFollowup: false })
          .catch((error) => console.error('transfer queue refresh failed', error));
      }
    });
    return refreshPromise;
  }

  function onEvent(kind, event) {
    if (!hydrated || refreshPromise) {
      enqueueEvent(kind, event);
      return;
    }

    const payload = event && Object.prototype.hasOwnProperty.call(event, 'payload')
      ? event.payload
      : event;
    if (!payload || typeof payload.revision !== 'number') return;
    const result = kind === 'job' ? applyJobEvent(payload) : applySummaryEvent(payload);
    if (!result.needsRefresh) return;
    enqueueEvent(kind, payload);
    synchronize().catch((error) => console.error('transfer queue refresh failed', error));
  }

  function ensureStarted(options) {
    if (startupPromise) return startupPromise;
    if (!options || typeof options.invoke !== 'function' || typeof options.listen !== 'function') {
      return Promise.reject(new Error('Transfer runtime requires invoke and listen'));
    }
    dependencies = {
      invoke: options.invoke,
      listen: options.listen,
      toast: options.toast || global.toast || null,
    };
    startupPromise = Promise.all([
      dependencies.listen(JOB_EVENT, (event) => onEvent('job', event)),
      dependencies.listen(SUMMARY_EVENT, (event) => onEvent('summary', event)),
    ]).then(() => synchronize());
    return startupPromise;
  }

  function command(method, args) {
    if (!dependencies) return Promise.reject(new Error('Transfer runtime has not started'));
    return dataService[method](dependencies.invoke, ...args);
  }

  function resolveConflict(id, resolution) {
    if (!resolution || typeof resolution !== 'object' || Array.isArray(resolution)) {
      return Promise.reject(new TypeError('Conflict resolution must be a tagged object'));
    }
    if (!RESOLUTION_KINDS.has(resolution.kind)) {
      return Promise.reject(new TypeError('Conflict resolution has an invalid kind'));
    }
    if (resolution.kind === 'rename'
        && (typeof resolution.destination !== 'string' || !resolution.destination.trim())) {
      return Promise.reject(new TypeError('Rename conflict resolution requires a destination'));
    }
    return command('resolve', [id, resolution]);
  }

  async function reconnect(job) {
    if (!dependencies) throw new Error('Transfer runtime has not started');
    const endpoint = job && job.endpoint;
    if (!job || !job.id || !endpoint || endpoint.kind !== 'configured' || !endpoint.serverEntryId) {
      throw new TypeError('Configured transfer reconnect requires a job and server entry');
    }
    const filesData = global.termlabFilesFeatureDataService;
    if (!filesData || typeof filesData.connectHost !== 'function') {
      throw new Error('SFTP host connection service is unavailable');
    }

    let session = null;
    try {
      session = await filesData.connectHost(dependencies.invoke, endpoint.serverEntryId);
    } catch (startingError) {
      const connectAuth = global.termlabConnectAuth;
      if (!connectAuth || typeof connectAuth.run !== 'function') throw startingError;
      session = await connectAuth.run(endpoint.serverEntryId, startingError, {
        invoke: dependencies.invoke,
        data: filesData,
        onError(message) {
          const toast = dependencies && dependencies.toast;
          if (toast && typeof toast.error === 'function') toast.error('SFTP reconnect failed', String(message));
        },
      });
    }
    if (!session) return false;
    await command('resume', [job.id]);
    return true;
  }

  const runtime = {
    ensureStarted,
    subscribe,
    getSnapshot: () => store.getSnapshot(),
    pause: (id) => command('pause', [id]),
    resume: (id) => command('resume', [id]),
    cancel: (id) => command('cancel', [id]),
    retry: (id) => command('retry', [id]),
    resolve: resolveConflict,
    pauseAll: () => command('pauseAll', []),
    resumeAll: () => command('resumeAll', []),
    reorder: (id, before) => command('reorder', [id, before]),
    setPriority: (id, priority) => command('setPriority', [id, priority]),
    clearCompleted: () => command('clearCompleted', []),
    updateSettings: (settings) => command('updateSettings', [settings]),
    reconnect,
    refresh: () => synchronize(),
  };

  global.termlabTransferRuntime = runtime;
})(window);
