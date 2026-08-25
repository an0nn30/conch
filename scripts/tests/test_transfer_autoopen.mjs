// Run: node scripts/tests/test_transfer_autoopen.mjs
//
// The Transfer Center auto-opens (FileZilla-style) when a NEW files-panel
// transfer is enqueued — and only then. Restored jobs from a previous run,
// editor remote-saves, progress updates, and additional files from the same
// batch must never re-summon a panel the user may have deliberately hidden.
// These tests drive the real store + runtime modules in a VM so the baseline
// is the actual hydrated snapshot, not a reimplementation.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const FRONTEND = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const MODULE_PATHS = [
  path.join(FRONTEND, 'app/features/transfers/data-service.js'),
  path.join(FRONTEND, 'app/features/transfers/store.js'),
  path.join(FRONTEND, 'app/features/transfers/runtime.js'),
  path.join(FRONTEND, 'app/features/transfers/auto-open.js'),
];

function job(id, kind, origin = 'filesPanel', batchId = null) {
  return {
    id,
    batchId,
    queueOrder: 0,
    fileName: `${id}.txt`,
    localPath: `/local/${id}.txt`,
    remotePath: `/remote/${id}.txt`,
    origin: origin === 'other' ? { kind: 'other', name: 'plugin' } : { kind: origin },
    state: { kind },
  };
}

function snapshot(revision, jobs) {
  return {
    revision,
    queuePaused: false,
    settings: { globalLimit: 3, perHostLimit: 2 },
    jobs,
    summary: {
      queued: 0, running: 0, paused: 0, attention: 0,
      failed: 0, active: 0, history: 0, queuePaused: false,
    },
    recoveryError: null,
  };
}

function delta(revision, upserts) {
  return {
    revision,
    upserts,
    removedIds: [],
    queuePaused: false,
    settings: { globalLimit: 3, perHostLimit: 2 },
  };
}

async function loadHarness(initialSnapshot) {
  const handlers = new Map();
  const sandbox = {
    console,
    Promise,
    structuredClone,
    setTimeout: () => 0,
    clearTimeout: () => {},
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const modulePath of MODULE_PATHS) {
    vm.runInContext(fs.readFileSync(modulePath, 'utf8'), sandbox, { filename: modulePath });
  }
  assert.ok(sandbox.termlabTransferAutoOpen, 'auto-open IIFE must expose window.termlabTransferAutoOpen');

  const invoke = (command) => {
    if (command === 'transfer_queue_snapshot') return Promise.resolve(initialSnapshot);
    return Promise.resolve(null);
  };
  const listen = (eventName, handler) => {
    handlers.set(eventName, handler);
    return Promise.resolve(() => handlers.delete(eventName));
  };
  await sandbox.termlabTransferRuntime.ensureStarted({ invoke, listen });

  const activations = [];
  const toolWindowManager = {
    activate: (id) => activations.push(id),
  };

  return {
    runtime: sandbox.termlabTransferRuntime,
    autoOpen: sandbox.termlabTransferAutoOpen,
    toolWindowManager,
    activations,
    emitJobs(revision, upserts) {
      const handler = handlers.get('transfer-job-updated');
      assert.ok(handler, 'transfer-job-updated listener must be installed');
      handler({ payload: delta(revision, upserts) });
    },
  };
}

// Restored jobs in the hydration snapshot are the baseline: launching the app
// with an in-flight queue must not pop the Transfer Center.
{
  const h = await loadHarness(snapshot(3, [
    job('restored-a', 'paused'),
    job('restored-b', 'queued'),
  ]));
  h.autoOpen.init({ runtime: h.runtime, toolWindowManager: h.toolWindowManager });
  assert.deepEqual(h.activations, [], 'restored jobs must not auto-open the Transfer Center');
}

// A new files-panel job activates the Transfer Center exactly once; later
// progress updates for the same job never re-activate (the user may have
// hidden the panel mid-transfer).
{
  const h = await loadHarness(snapshot(0, []));
  h.autoOpen.init({ runtime: h.runtime, toolWindowManager: h.toolWindowManager });

  h.emitJobs(1, [job('up-1', 'queued')]);
  assert.deepEqual(h.activations, ['transfer-center'], 'a new files-panel job must activate the Transfer Center');

  h.emitJobs(2, [job('up-1', 'running')]);
  h.emitJobs(3, [job('up-1', 'completed')]);
  assert.deepEqual(h.activations, ['transfer-center'], 'updates to a seen job must not re-activate');
}

// Editor remote-saves are quiet: they route through the queue but never summon
// the panel.
{
  const h = await loadHarness(snapshot(0, []));
  h.autoOpen.init({ runtime: h.runtime, toolWindowManager: h.toolWindowManager });
  h.emitJobs(1, [job('save-1', 'queued', 'editor')]);
  assert.deepEqual(h.activations, [], 'editor-origin jobs must not auto-open the Transfer Center');
}

// A multi-file batch is one user action: it activates once, even when its jobs
// arrive across separate events.
{
  const h = await loadHarness(snapshot(0, []));
  h.autoOpen.init({ runtime: h.runtime, toolWindowManager: h.toolWindowManager });
  h.emitJobs(1, [job('b-1', 'queued', 'filesPanel', 'batch-7'), job('b-2', 'queued', 'filesPanel', 'batch-7')]);
  h.emitJobs(2, [job('b-3', 'queued', 'filesPanel', 'batch-7')]);
  assert.deepEqual(h.activations, ['transfer-center'], 'one batch must activate the Transfer Center once');

  h.emitJobs(3, [job('solo', 'queued')]);
  assert.deepEqual(h.activations, ['transfer-center', 'transfer-center'],
    'a genuinely new transfer after a batch must activate again');
}

// A job first observed in a terminal state (history insert, instant failure)
// is not a starting transfer and must not open the panel.
{
  const h = await loadHarness(snapshot(0, []));
  h.autoOpen.init({ runtime: h.runtime, toolWindowManager: h.toolWindowManager });
  h.emitJobs(1, [job('done-1', 'completed'), job('failed-1', 'failed')]);
  assert.deepEqual(h.activations, [], 'jobs first seen in a terminal state must not auto-open');
}

// dispose() detaches the subscription: jobs arriving afterwards are ignored.
{
  const h = await loadHarness(snapshot(0, []));
  const controller = h.autoOpen.init({ runtime: h.runtime, toolWindowManager: h.toolWindowManager });
  controller.dispose();
  h.emitJobs(1, [job('late-1', 'queued')]);
  assert.deepEqual(h.activations, [], 'a disposed auto-open must not activate');
}

// init() validates its dependencies so a wiring regression fails loudly at
// startup instead of silently never auto-opening.
{
  // The modules run in a VM realm, so match on the error name rather than the
  // host realm's TypeError constructor.
  const isTypeError = (error) => error && error.name === 'TypeError';
  const h = await loadHarness(snapshot(0, []));
  assert.throws(() => h.autoOpen.init({ toolWindowManager: h.toolWindowManager }), isTypeError);
  assert.throws(() => h.autoOpen.init({ runtime: h.runtime }), isTypeError);
}

console.log('transfer auto-open: all assertions passed');
