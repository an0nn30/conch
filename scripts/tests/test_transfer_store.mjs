// Run: node scripts/tests/test_transfer_store.mjs
//
// The transfer queue is shared backend state projected into each desktop
// window. These tests load the real IIFE modules in a VM so revision handling,
// cloning, and runtime ordering are exercised without reimplementing them.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const FRONTEND = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const DATA_SERVICE_PATH = path.join(FRONTEND, 'app/features/transfers/data-service.js');
const STORE_PATH = path.join(FRONTEND, 'app/features/transfers/store.js');
const RUNTIME_PATH = path.join(FRONTEND, 'app/features/transfers/runtime.js');

function plain(value) {
  return JSON.parse(JSON.stringify(value));
}

function job(id, kind, queueOrder = 0, batchId = null) {
  return {
    id,
    batchId,
    queueOrder,
    fileName: `${id}.txt`,
    localPath: `/private/${id}.txt`,
    remotePath: `/srv/secret/${id}.txt`,
    state: { kind },
  };
}

function summary(queuePaused, values = {}) {
  return {
    queued: 0,
    running: 0,
    paused: 0,
    attention: 0,
    failed: 0,
    active: 0,
    history: 0,
    queuePaused,
    ...values,
  };
}

function snapshot(revision, jobs, values = {}) {
  return {
    revision,
    queuePaused: false,
    settings: { globalLimit: 3, perHostLimit: 2 },
    jobs,
    summary: summary(false),
    recoveryError: null,
    ...values,
  };
}

function delta(revision, values = {}) {
  return {
    revision,
    upserts: [],
    removedIds: [],
    queuePaused: false,
    settings: { globalLimit: 3, perHostLimit: 2 },
    ...values,
  };
}

function loadStore() {
  const sandbox = { console, structuredClone };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(STORE_PATH, 'utf8'), sandbox, { filename: STORE_PATH });
  assert.ok(sandbox.termlabTransferStore, 'store IIFE must expose window.termlabTransferStore');
  return sandbox.termlabTransferStore.create();
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function loadRuntime(options = {}) {
  const handlers = new Map();
  const calls = [];
  const order = [];
  const timers = [];
  const toastCalls = [];
  const responses = Array.isArray(options.snapshots) ? [...options.snapshots] : [snapshot(0, [])];

  const invoke = (command, args) => {
    calls.push({ command, args });
    order.push(`invoke:${command}`);
    if (command === 'transfer_queue_snapshot') {
      const response = responses.shift();
      return response && typeof response.then === 'function' ? response : Promise.resolve(response);
    }
    return Promise.resolve({ command, args });
  };
  const listen = (eventName, handler) => {
    order.push(`listen:${eventName}`);
    handlers.set(eventName, handler);
    return Promise.resolve(() => handlers.delete(eventName));
  };
  const toast = {
    success: (...args) => toastCalls.push({ kind: 'success', args }),
    error: (...args) => toastCalls.push({ kind: 'error', args }),
    warn: (...args) => toastCalls.push({ kind: 'warn', args }),
    info: (...args) => toastCalls.push({ kind: 'info', args }),
  };
  const sandbox = {
    console,
    Promise,
    structuredClone,
    setTimeout: (callback, delay) => {
      timers.push({ callback, delay });
      return timers.length;
    },
    clearTimeout: () => {},
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const modulePath of [DATA_SERVICE_PATH, STORE_PATH, RUNTIME_PATH]) {
    vm.runInContext(fs.readFileSync(modulePath, 'utf8'), sandbox, { filename: modulePath });
  }
  assert.ok(sandbox.termlabTransferRuntime, 'runtime IIFE must expose window.termlabTransferRuntime');

  return {
    runtime: sandbox.termlabTransferRuntime,
    invoke,
    listen,
    toast,
    handlers,
    calls,
    order,
    timers,
    toastCalls,
    emit(eventName, payload) {
      const handler = handlers.get(eventName);
      assert.ok(handler, `listener for ${eventName} must be installed`);
      return handler({ payload });
    },
    flushToasts() {
      const pending = timers.splice(0);
      for (const timer of pending) timer.callback();
    },
  };
}

// hydrate() must replace, rather than merge with, the previous projection and
// both ingress and egress must be cloned so consumers cannot corrupt it.
{
  const store = loadStore();
  const first = snapshot(2, [job('old', 'running')]);
  store.hydrate(first);
  first.jobs[0].state.kind = 'failed';
  assert.equal(store.getSnapshot().jobs[0].state.kind, 'running');

  store.hydrate(snapshot(4, [job('fresh', 'queued')]));
  const exposed = store.getSnapshot();
  exposed.jobs[0].state.kind = 'cancelled';
  assert.deepEqual(plain(store.getSnapshot().jobs), [job('fresh', 'queued')]);
}

// Schema v1 persists transfer speed as a number for backward compatibility.
// The frontend projection alone maps the numeric unknown sentinel to null.
{
  const store = loadStore();
  store.hydrate(snapshot(1, [{
    ...job('idle', 'queued'),
    speedBytesPerSecond: 0,
    commitBackupExpected: true,
  }]));
  assert.equal(store.getSnapshot().jobs[0].speedBytesPerSecond, null);
  assert.equal('commitBackupExpected' in store.getSnapshot().jobs[0], false);

  store.applyJobEvent(delta(2, {
    upserts: [{ ...job('idle', 'running'), speedBytesPerSecond: 768 }],
  }));
  assert.equal(store.getSnapshot().jobs[0].speedBytesPerSecond, 768);

  store.applyJobEvent(delta(3, {
    upserts: [{ ...job('idle', 'paused'), speedBytesPerSecond: 0 }],
  }));
  assert.equal(store.getSnapshot().jobs[0].speedBytesPerSecond, null);
}

// A revision is one complete delta. Applying only its first row, omitting a
// compaction upsert, or forgetting queue/settings fields would leave the UI at
// a state that never existed in the backend.
{
  const store = loadStore();
  store.hydrate(snapshot(4, [job('a', 'running', 1), job('b', 'queued', 2), job('gone', 'completed', 3)]));

  assert.deepEqual(
    plain(store.applyJobEvent(delta(5, {
      upserts: [job('a', 'paused', 2), job('b', 'queued', 1), job('new', 'queued', 3)],
      removedIds: ['gone'],
      queuePaused: true,
      settings: { globalLimit: 5, perHostLimit: 1 },
    }))),
    { needsRefresh: false },
  );

  const current = plain(store.getSnapshot());
  assert.equal(current.revision, 5);
  assert.equal(current.queuePaused, true);
  assert.deepEqual(current.settings, { globalLimit: 5, perHostLimit: 1 });
  assert.deepEqual(current.jobs, [job('a', 'paused', 2), job('b', 'queued', 1), job('new', 'queued', 3)]);
}

// A same/older delta is an idempotent no-op. A gap reports refresh without
// partially applying any row or advancing the authoritative revision.
{
  const store = loadStore();
  store.hydrate(snapshot(5, [job('a', 'paused')]));
  assert.deepEqual(plain(store.applyJobEvent(delta(5, { upserts: [job('a', 'failed')] }))), { needsRefresh: false });
  assert.deepEqual(plain(store.applyJobEvent(delta(7, { upserts: [job('b', 'queued')] }))), { needsRefresh: true });
  assert.deepEqual(plain(store.getSnapshot()), snapshot(5, [job('a', 'paused')]));
}

// Summaries belong to a revision but never advance it. A future summary cannot
// disguise a missing atomic job delta by publishing counts for unseen rows.
{
  const store = loadStore();
  store.hydrate(snapshot(5, [job('a', 'running')]));
  assert.deepEqual(
    plain(store.applySummaryEvent({ revision: 5, summary: summary(false, { running: 1, active: 1 }) })),
    { needsRefresh: false },
  );
  assert.equal(store.getSnapshot().summary.running, 1);

  assert.deepEqual(
    plain(store.applySummaryEvent({ revision: 6, summary: summary(false, { failed: 99, history: 99 }) })),
    { needsRefresh: true },
  );
  assert.equal(store.getSnapshot().revision, 5);
  assert.equal(store.getSnapshot().summary.failed, 0);
}

// Active/history are always derived from state.kind. No shadow flag can drift
// from the backend state machine.
{
  const store = loadStore();
  store.hydrate(snapshot(8, [
    job('queued', 'queued'),
    job('attention', 'needsAttention'),
    job('done', 'completed'),
    job('failed', 'failed'),
    job('cancelled', 'cancelled'),
  ]));
  assert.deepEqual(plain(store.activeJobs()).map((item) => item.id), ['queued', 'attention']);
  assert.deepEqual(plain(store.historyJobs()).map((item) => item.id), ['done', 'failed', 'cancelled']);
}

// Both listeners must exist before snapshot I/O starts. Early events are
// buffered, sorted by revision, and replayed over the authoritative snapshot.
{
  const initial = deferred();
  const harness = loadRuntime({ snapshots: [initial.promise] });
  const starting = harness.runtime.ensureStarted({
    invoke: harness.invoke,
    listen: harness.listen,
    toast: harness.toast,
  });

  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(harness.order.slice(0, 3), [
    'listen:transfer-job-updated',
    'listen:transfer-queue-summary',
    'invoke:transfer_queue_snapshot',
  ]);

  harness.emit('transfer-job-updated', delta(6, { upserts: [job('b', 'queued')] }));
  harness.emit('transfer-queue-summary', { revision: 6, summary: summary(false, { active: 2, queued: 1, running: 1 }) });
  harness.emit('transfer-job-updated', delta(5, { upserts: [job('a', 'paused')] }));
  initial.resolve(snapshot(4, [job('a', 'running')]));
  await starting;

  const current = plain(harness.runtime.getSnapshot());
  assert.equal(current.revision, 6);
  assert.deepEqual(current.jobs.map((item) => [item.id, item.state.kind]), [['a', 'paused'], ['b', 'queued']]);
  assert.equal(current.summary.active, 2);
}

// A replay gap never guesses. Multiple gap observations serialize behind one
// refresh snapshot, and ensureStarted remains one promise/listener pair.
{
  const refreshSnapshot = deferred();
  const harness = loadRuntime({ snapshots: [snapshot(4, [job('a', 'running')]), refreshSnapshot.promise] });
  const first = harness.runtime.ensureStarted({ invoke: harness.invoke, listen: harness.listen, toast: harness.toast });
  const second = harness.runtime.ensureStarted({ invoke: harness.invoke, listen: harness.listen, toast: harness.toast });
  assert.equal(first, second, 'idempotent startup must return the same in-flight promise');

  await first;
  harness.emit('transfer-job-updated', delta(7, { upserts: [job('gap-a', 'queued')] }));
  harness.emit('transfer-job-updated', delta(8, { upserts: [job('gap-b', 'queued')] }));
  await Promise.resolve();
  assert.equal(harness.calls.filter((call) => call.command === 'transfer_queue_snapshot').length, 2);
  assert.equal(harness.order.filter((item) => item === 'listen:transfer-job-updated').length, 1);
  assert.equal(harness.order.filter((item) => item === 'listen:transfer-queue-summary').length, 1);

  refreshSnapshot.resolve(snapshot(8, [job('a', 'completed'), job('gap-a', 'queued'), job('gap-b', 'queued')]));
  await harness.runtime.refresh();
  assert.equal(harness.runtime.getSnapshot().revision, 8);
}

// Subscribers get a cloned current value synchronously. Removing one callback
// is exact and does not affect other subscribers.
{
  const harness = loadRuntime({ snapshots: [snapshot(1, [job('a', 'queued')])] });
  const firstSeen = [];
  const secondSeen = [];
  const unsubscribe = harness.runtime.subscribe((value) => firstSeen.push(value.revision));
  harness.runtime.subscribe((value) => secondSeen.push(value.revision));
  assert.deepEqual(firstSeen, [0]);
  assert.deepEqual(secondSeen, [0]);
  unsubscribe();

  await harness.runtime.ensureStarted({ invoke: harness.invoke, listen: harness.listen, toast: harness.toast });
  assert.deepEqual(firstSeen, [0]);
  assert.deepEqual(secondSeen, [0, 1]);
}

// A refresh response can race a newer applied event. An older snapshot must
// not roll the public projection backward or notify subscribers with stale
// state.
{
  const stale = deferred();
  const harness = loadRuntime({ snapshots: [snapshot(5, [job('a', 'paused')]), stale.promise] });
  const seen = [];
  harness.runtime.subscribe((value) => seen.push([value.revision, value.jobs[0] && value.jobs[0].state.kind]));
  await harness.runtime.ensureStarted({ invoke: harness.invoke, listen: harness.listen, toast: harness.toast });

  const refreshing = harness.runtime.refresh();
  stale.resolve(snapshot(4, [job('a', 'running')]));
  await refreshing;

  assert.deepEqual(seen, [[0, undefined], [5, 'paused']]);
  assert.deepEqual(
    plain(harness.runtime.getSnapshot()),
    snapshot(5, [job('a', 'paused')]),
    'an older refresh response must leave the newer projection byte-for-byte intact',
  );
}

// If a stale response cannot close a buffered gap, finish that refresh, retain
// the gap, then start one coalesced follow-up. The first promise must not loop
// recursively while the later authoritative response is pending.
{
  const stale = deferred();
  const authoritative = deferred();
  const harness = loadRuntime({
    snapshots: [snapshot(5, [job('a', 'running')]), stale.promise, authoritative.promise],
  });
  const seen = [];
  harness.runtime.subscribe((value) => seen.push(value.revision));
  await harness.runtime.ensureStarted({ invoke: harness.invoke, listen: harness.listen, toast: harness.toast });

  harness.emit('transfer-job-updated', delta(7, { upserts: [job('gap', 'queued')] }));
  const staleRefresh = harness.runtime.refresh();
  let staleSettled = false;
  staleRefresh.then(() => { staleSettled = true; });
  assert.equal(harness.calls.filter((call) => call.command === 'transfer_queue_snapshot').length, 2);

  stale.resolve(snapshot(5, [job('a', 'running')]));
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(staleSettled, true, 'the stale refresh must settle before its one follow-up response');
  assert.equal(harness.runtime.getSnapshot().revision, 5);
  assert.deepEqual(seen, [0, 5], 'an equal stale response must not publish a duplicate snapshot');
  assert.equal(
    harness.calls.filter((call) => call.command === 'transfer_queue_snapshot').length,
    3,
    'one later authoritative refresh is started after the stale request clears',
  );
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(
    harness.calls.filter((call) => call.command === 'transfer_queue_snapshot').length,
    3,
    'the unresolved follow-up must not create overlapping or recursive requests',
  );

  authoritative.resolve(snapshot(7, [job('a', 'running'), job('gap', 'queued')]));
  await harness.runtime.refresh();
  assert.equal(harness.runtime.getSnapshot().revision, 7);
  assert.deepEqual(seen, [0, 5, 7]);
}

// Commands delegate exactly once through the focused data service and never
// mutate queue lifecycle optimistically. The event is the state transition.
{
  const harness = loadRuntime({ snapshots: [snapshot(1, [job('a', 'running')])] });
  await harness.runtime.ensureStarted({ invoke: harness.invoke, listen: harness.listen, toast: harness.toast });
  await harness.runtime.pause('a');
  assert.equal(harness.runtime.getSnapshot().jobs[0].state.kind, 'running');
  assert.deepEqual(
    plain(harness.calls.filter((call) => call.command === 'transfer_pause')),
    [{ command: 'transfer_pause', args: { transferId: 'a' } }],
  );

  harness.emit('transfer-job-updated', delta(2, { upserts: [job('a', 'paused')] }));
  assert.equal(harness.runtime.getSnapshot().jobs[0].state.kind, 'paused');

  await harness.runtime.resume('a');
  await harness.runtime.cancel('a');
  await harness.runtime.retry('a');
  await assert.rejects(
    harness.runtime.resolve('a', 'overwrite'),
    /tagged object/,
    'the Rust internally-tagged enum must never receive a bare string',
  );
  await harness.runtime.resolve('a', { kind: 'overwrite' });
  await harness.runtime.resolve('a', { kind: 'rename', destination: '/srv/renamed.txt' });
  await harness.runtime.pauseAll();
  await harness.runtime.resumeAll();
  await harness.runtime.reorder('a', 'b');
  await harness.runtime.setPriority('a', 'interactive');
  await harness.runtime.clearCompleted();
  await harness.runtime.updateSettings({ globalLimit: 4, perHostLimit: 2 });
  assert.deepEqual(
    plain(harness.calls.slice(2)),
    [
      { command: 'transfer_resume', args: { transferId: 'a' } },
      { command: 'transfer_cancel', args: { transferId: 'a' } },
      { command: 'transfer_retry', args: { transferId: 'a' } },
      { command: 'transfer_resolve', args: { transferId: 'a', resolution: { kind: 'overwrite' } } },
      {
        command: 'transfer_resolve',
        args: { transferId: 'a', resolution: { kind: 'rename', destination: '/srv/renamed.txt' } },
      },
      { command: 'transfer_pause_all' },
      { command: 'transfer_resume_all' },
      { command: 'transfer_reorder', args: { transferId: 'a', before: 'b' } },
      { command: 'transfer_set_priority', args: { transferId: 'a', priority: 'interactive' } },
      { command: 'transfer_clear_completed' },
      { command: 'transfer_update_settings', args: { settings: { globalLimit: 4, perHostLimit: 2 } } },
    ],
  );
}

// Terminal and newly attention-requiring transitions aggregate by batch for
// 300ms. Toast text is deliberately summary-only: no names, paths, or backend
// errors that could reveal credentials or sensitive locations.
{
  const jobs = [
    job('a', 'running', 1, 'success-batch'),
    job('b', 'running', 2, 'success-batch'),
    job('c', 'running', 3, 'failure-batch'),
    job('d', 'running', 4, 'attention-batch'),
  ];
  const harness = loadRuntime({ snapshots: [snapshot(1, jobs)] });
  await harness.runtime.ensureStarted({ invoke: harness.invoke, listen: harness.listen, toast: harness.toast });

  const completedA = job('a', 'completed', 1, 'success-batch');
  const completedB = job('b', 'completed', 2, 'success-batch');
  const failed = job('c', 'failed', 3, 'failure-batch');
  failed.state.error = 'password hunter2 at /private/c.txt';
  const attention = job('d', 'needsAttention', 4, 'attention-batch');
  attention.state.reason = { kind: 'conflict', destination: '/srv/secret/d.txt' };

  harness.emit('transfer-job-updated', delta(2, { upserts: [completedA, completedB] }));
  harness.emit('transfer-job-updated', delta(3, { upserts: [failed] }));
  harness.emit('transfer-job-updated', delta(4, { upserts: [attention] }));
  assert.deepEqual(harness.timers.map((timer) => timer.delay), [300, 300, 300]);
  assert.equal(harness.toastCalls.length, 0);

  harness.flushToasts();
  assert.deepEqual(harness.toastCalls.map((call) => call.kind), ['success', 'error', 'warn']);
  const allToastText = JSON.stringify(harness.toastCalls);
  assert.match(allToastText, /2/);
  for (const secret of ['a.txt', 'b.txt', 'c.txt', 'd.txt', '/private', '/srv/secret', 'hunter2']) {
    assert.ok(!allToastText.includes(secret), `toast text must exclude ${secret}`);
  }
}

console.log('transfer store/runtime: all assertions passed');
