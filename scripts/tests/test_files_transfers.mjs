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
const dataServiceSource = fs.readFileSync(
  path.join(repoRoot, 'crates/termlab_tauri/frontend/app/features/files/data-service.js'),
  'utf8',
);
const paneViewSource = fs.readFileSync(
  path.join(repoRoot, 'crates/termlab_tauri/frontend/app/features/files/pane-view.js'),
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

// The files feature is an explicit normal-priority/ask-conflict producer.
// A regression here silently changes queue scheduling or bypasses the
// Transfer Center's conflict workflow even though the transfer still starts.
{
  const dataSandbox = {};
  dataSandbox.window = dataSandbox;
  vm.createContext(dataSandbox);
  vm.runInContext(dataServiceSource, dataSandbox, { filename: 'data-service.js' });
  const calls = [];
  const invoke = (command, args) => {
    calls.push({ command, args });
    return Promise.resolve(`${command}-id`);
  };
  const options = { origin: 'filesPanel', conflictPolicy: { kind: 'ask' } };

  await dataSandbox.termlabFilesFeatureDataService.transferUpload(
    invoke, 4, '/tmp/current.txt', '/srv/current.txt', options,
  );
  await dataSandbox.termlabFilesFeatureDataService.transferDownload(
    invoke, 4, '/srv/current.txt', '/tmp/current.txt', options,
  );

  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [
    {
      command: 'transfer_upload',
      args: {
        paneId: 4,
        localPath: '/tmp/current.txt',
        remotePath: '/srv/current.txt',
        origin: 'filesPanel',
        conflictPolicy: { kind: 'ask' },
      },
    },
    {
      command: 'transfer_download',
      args: {
        paneId: 4,
        remotePath: '/srv/current.txt',
        localPath: '/tmp/current.txt',
        origin: 'filesPanel',
        conflictPolicy: { kind: 'ask' },
      },
    },
  ]);
}

// Attention is a stable text badge, not a percentage or a spinner. Keeping
// this pure makes the state-to-label contract testable without a fake DOM.
{
  const paneSandbox = {};
  paneSandbox.window = paneSandbox;
  vm.createContext(paneSandbox);
  vm.runInContext(paneViewSource, paneSandbox, { filename: 'pane-view.js' });
  assert.equal(
    paneSandbox.termlabFilesPaneView.transferBadgeHtml({ status: 'attention' }),
    '<span class="fp-transfer-pct" role="status">Needs attention</span>',
  );
}

const localPane = { transferStatus: {} };
const remotePane = {
  transferStatus: {
    'current.txt': { status: 'in_progress', percent: 75, transferId: 'upload-1' },
  },
};
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
  [],
  'legacy completion does not refresh before the authoritative committed snapshot',
);
assert.equal(remotePane.transferStatus['current.txt'], undefined, 'legacy completion clears the badge');
assert.equal(successToasts.length, 0, 'the files producer never publishes lifecycle toasts');

controller.handleTransferSnapshot({
  revision: 1,
  jobs: [{
    id: 'upload-2',
    direction: 'upload',
    origin: { kind: 'filesPanel' },
    fileName: 'blocked.txt',
    state: { kind: 'running' },
  }],
});
controller.handleTransferSnapshot({
  revision: 2,
  jobs: [{
    id: 'upload-2',
    direction: 'upload',
    origin: { kind: 'filesPanel' },
    fileName: 'blocked.txt',
    state: { kind: 'needsAttention', reason: { kind: 'sourceChanged' } },
  }],
});
assert.deepEqual(JSON.parse(JSON.stringify(remotePane.transferStatus['blocked.txt'])), {
  status: 'attention', percent: 0, transferId: 'upload-2',
});

controller.handleTransferSnapshot({
  revision: 3,
  jobs: [{
    id: 'upload-2',
    direction: 'upload',
    origin: { kind: 'filesPanel' },
    fileName: 'blocked.txt',
    state: { kind: 'completed', result: 'transferred' },
  }],
});
assert.equal(remotePane.transferStatus['blocked.txt'], undefined, 'terminal snapshot clears the badge');
assert.deepEqual(
  refreshed,
  [localPane, remotePane],
  'a newly committed completion refreshes both source and destination listings',
);
controller.handleTransferSnapshot({
  revision: 4,
  jobs: [{
    id: 'upload-2',
    direction: 'upload',
    origin: { kind: 'filesPanel' },
    fileName: 'blocked.txt',
    state: { kind: 'completed', result: 'transferred' },
  }],
});
assert.deepEqual(
  refreshed,
  [localPane, remotePane],
  'replaying an already-observed completion does not refresh either pane again',
);

controller.handleTransferSnapshot({
  revision: 5,
  jobs: [{
    id: 'download-1',
    direction: 'download',
    origin: { kind: 'filesPanel' },
    fileName: 'offline.txt',
    state: { kind: 'running' },
  }],
});
controller.handleTransferSnapshot({
  revision: 6,
  jobs: [{
    id: 'download-1',
    direction: 'download',
    origin: { kind: 'filesPanel' },
    fileName: 'offline.txt',
    state: { kind: 'needsConnection', message: 'Reconnect' },
  }],
});
assert.deepEqual(JSON.parse(JSON.stringify(localPane.transferStatus['offline.txt'])), {
  status: 'attention', percent: 0, transferId: 'download-1',
});
controller.handleTransferSnapshot({
  revision: 7,
  jobs: [{
    id: 'download-1',
    direction: 'download',
    origin: { kind: 'filesPanel' },
    fileName: 'offline.txt',
    state: { kind: 'cancelled', cleanupError: null },
  }],
});
assert.equal(localPane.transferStatus['offline.txt'], undefined);

// The durable runtime and the compatibility transfer-progress handler receive
// the same terminal transition. With both real modules loaded, only the
// authoritative count-only toast may publish; the legacy byte-progress event
// may only clear its temporary pane badge.
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
