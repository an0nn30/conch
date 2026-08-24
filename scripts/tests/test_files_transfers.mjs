// Run: node scripts/tests/test_files_transfers.mjs

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..');
const source = fs.readFileSync(
  path.join(repoRoot, 'crates/termlab_tauri/frontend/app/features/files/transfers.js'),
  'utf8',
);
const transferModulePaths = [
  'crates/termlab_tauri/frontend/app/features/transfers/data-service.js',
  'crates/termlab_tauri/frontend/app/features/transfers/store.js',
  'crates/termlab_tauri/frontend/app/features/transfers/runtime.js',
  'crates/termlab_tauri/frontend/app/features/files/transfers.js',
].map((relativePath) => path.join(repoRoot, relativePath));

const successToasts = [];
const sandbox = {
  window: {
    toast: {
      success(message) { successToasts.push(message); },
    },
  },
  setTimeout,
  clearTimeout,
};
sandbox.window.window = sandbox.window;
vm.createContext(sandbox);
vm.runInContext(source, sandbox, { filename: 'transfers.js' });

const localPane = { transferStatus: {} };
const remotePane = { transferStatus: {} };
const refreshed = [];
const controller = sandbox.window.termlabFilesTransfers.createController({
  localPane,
  remotePane,
  toast: sandbox.window.toast,
  loadEntries(pane) { refreshed.push(pane); },
});

controller.handleTransferProgress({
  payload: {
    transfer_id: 'upload-1',
    kind: 'upload',
    status: 'completed',
    bytes_transferred: 8,
    total_bytes: 8,
    file_name: 'current.txt',
    error: null,
  },
});

assert.deepEqual(
  refreshed,
  [localPane, remotePane],
  'upload completion refreshes both source and destination listings',
);
assert.equal(successToasts.length, 1);

// The durable runtime and the compatibility transfer-progress handler receive
// the same terminal transition. With both real modules loaded, only the
// authoritative count-only toast may publish; the legacy handler still owns
// its temporary badge cleanup until the files panel migrates fully.
{
  const handlers = new Map();
  const timers = [];
  const toastCalls = [];
  const integrationSandbox = {
    console,
    Promise,
    structuredClone,
    setTimeout(callback, delay) {
      timers.push({ callback, delay });
      return timers.length;
    },
    clearTimeout() {},
  };
  integrationSandbox.window = integrationSandbox;
  vm.createContext(integrationSandbox);
  for (const modulePath of transferModulePaths) {
    vm.runInContext(fs.readFileSync(modulePath, 'utf8'), integrationSandbox, { filename: modulePath });
  }

  const authoritativeToast = {
    success: (...args) => toastCalls.push({ kind: 'success', args }),
    error: (...args) => toastCalls.push({ kind: 'error', args }),
    warn: (...args) => toastCalls.push({ kind: 'warn', args }),
    info: (...args) => toastCalls.push({ kind: 'info', args }),
  };
  const runningJob = {
    id: 'job-1',
    batchId: 'batch-1',
    fileName: 'secret.txt',
    localPath: '/private/secret.txt',
    remotePath: '/srv/secret.txt',
    queueOrder: 1,
    state: { kind: 'running' },
  };
  const initialSnapshot = {
    revision: 1,
    queuePaused: false,
    settings: { globalLimit: 3, perHostLimit: 2 },
    jobs: [runningJob],
    summary: {
      queued: 0,
      running: 1,
      paused: 0,
      attention: 0,
      failed: 0,
      active: 1,
      history: 0,
      queuePaused: false,
    },
    recoveryError: null,
  };
  const invoke = (command) => {
    assert.equal(command, 'transfer_queue_snapshot');
    return Promise.resolve(initialSnapshot);
  };
  const listen = (eventName, handler) => {
    handlers.set(eventName, handler);
    return Promise.resolve(() => handlers.delete(eventName));
  };
  await integrationSandbox.termlabTransferRuntime.ensureStarted({
    invoke,
    listen,
    toast: authoritativeToast,
  });

  const integrationLocalPane = { transferStatus: {} };
  const integrationRemotePane = {
    transferStatus: { 'secret.txt': { status: 'in_progress', percent: 55 } },
  };
  const integrationController = integrationSandbox.termlabFilesTransfers.createController({
    localPane: integrationLocalPane,
    remotePane: integrationRemotePane,
    toast: authoritativeToast,
  });
  const failedJob = {
    ...runningJob,
    state: { kind: 'failed', error: 'password hunter2 at /private/secret.txt' },
  };
  handlers.get('transfer-job-updated')({
    payload: {
      revision: 2,
      upserts: [failedJob],
      removedIds: [],
      queuePaused: false,
      settings: { globalLimit: 3, perHostLimit: 2 },
    },
  });
  integrationController.handleTransferProgress({
    payload: {
      transfer_id: 'job-1',
      kind: 'upload',
      status: 'failed',
      bytes_transferred: 55,
      total_bytes: 100,
      file_name: 'secret.txt',
      error: 'password hunter2 at /private/secret.txt',
    },
  });

  assert.equal(integrationRemotePane.transferStatus['secret.txt'], undefined,
    'legacy progress still clears its temporary transfer badge');
  for (const timer of timers.splice(0)) timer.callback();
  assert.deepEqual(toastCalls.map((call) => call.kind), ['error']);
  const toastText = JSON.stringify(toastCalls);
  assert.match(toastText, /1 transfer failed/);
  for (const secret of ['secret.txt', '/private', '/srv', 'hunter2']) {
    assert.ok(!toastText.includes(secret), `combined terminal toast must exclude ${secret}`);
  }
}

console.log('files transfer completion tests passed');
