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

// Folder transfers hand the whole tree to the backend's recursive-expansion
// command rather than walking it from the frontend. `destPath` must stay the
// opposite pane's current directory as-is — the backend joins the source
// folder's own basename onto it — so this call shape is the contract the
// context-menu wiring in files-panel.js depends on.
{
  const dataSandbox = {};
  dataSandbox.window = dataSandbox;
  vm.createContext(dataSandbox);
  vm.runInContext(dataServiceSource, dataSandbox, { filename: 'data-service.js' });
  const calls = [];
  const invoke = (command, args) => {
    calls.push({ command, args });
    return Promise.resolve('batch-id');
  };

  await dataSandbox.termlabFilesFeatureDataService.transferRecursive(
    invoke, 4, 'upload', '/tmp/folder', '/srv/uploads',
  );

  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [
    {
      command: 'transfer_enqueue_recursive',
      args: {
        paneId: 4,
        direction: 'upload',
        sourcePath: '/tmp/folder',
        destPath: '/srv/uploads',
      },
    },
  ]);
}

// An attention state with a transfer id is an actionable control. Regressing
// this to passive text strands the job in another tool window with no
// discoverable recovery path from the Files workflow that created it.
{
  const paneSandbox = {};
  paneSandbox.window = paneSandbox;
  vm.createContext(paneSandbox);
  vm.runInContext(paneViewSource, paneSandbox, { filename: 'pane-view.js' });
  assert.equal(
    paneSandbox.termlabFilesPaneView.transferBadgeHtml({
      status: 'attention',
      transferId: 'upload-2',
    }),
    '<button type="button" class="fp-transfer-attention" data-transfer-id="upload-2" aria-label="Resolve transfer issue">Needs attention</button>',
  );
  assert.equal(
    paneSandbox.termlabFilesPaneView.transferBadgeHtml({
      status: 'preparing',
      direction: 'upload',
    }),
    '<span class="fp-transfer-phase" role="status">Preparing upload…</span>',
    'preflight is visibly different from byte progress',
  );
  assert.equal(
    paneSandbox.termlabFilesPaneView.transferBadgeHtml({
      status: 'starting',
      direction: 'download',
    }),
    '<span class="fp-transfer-phase" role="status">Starting download…</span>',
  );
  assert.equal(
    paneSandbox.termlabFilesPaneView.transferBadgeHtml({
      status: 'waiting',
      direction: 'upload',
    }),
    '<span class="fp-transfer-waiting" role="status">Waiting to retry upload…</span>',
    'retry backoff is not presented as active preflight work',
  );
  assert.equal(
    paneSandbox.termlabFilesPaneView.transferBadgeHtml({
      status: 'in_progress',
      percent: 0.025,
    }),
    '<span class="fp-transfer-pct">&lt;1%</span>',
    'large transfers show byte movement before a whole percentage is reached',
  );
  assert.equal(
    paneSandbox.termlabFilesPaneView.transferActivityText({
      'large.iso': { status: 'preparing', direction: 'upload' },
    }),
    'Preparing upload: large.iso',
    'footer activity remains visible when the destination has no row yet',
  );
  assert.equal(
    paneSandbox.termlabFilesPaneView.transferActivityText({
      'retry.iso': { status: 'waiting', direction: 'upload' },
    }),
    '',
    'a retry backoff does not animate the active-transfer footer',
  );

  const invoker = { id: 'attention-control' };
  const activations = [];
  paneSandbox.termlabFilesPaneView.activateTransferBadge(
    { status: 'attention', transferId: 'upload-2' },
    invoker,
    {
      onTransferAttention(transferId, source) {
        activations.push({ transferId, source });
      },
    },
  );
  assert.deepEqual(activations, [{ transferId: 'upload-2', source: invoker }]);

  function eventElement(tagName) {
    const listeners = new Map();
    let html = '';
    const element = {
      tagName: tagName.toUpperCase(),
      parentNode: null,
      children: [],
      dataset: {},
      style: {},
      className: '',
      tabIndex: -1,
      classList: { add() {}, remove() {} },
      setAttribute() {},
      addEventListener(name, listener) {
        if (!listeners.has(name)) listeners.set(name, []);
        listeners.get(name).push(listener);
      },
      appendChild(child) {
        child.parentNode = this;
        this.children.push(child);
        return child;
      },
      querySelector(selector) {
        if (selector === '.fp-transfer-attention[data-transfer-id]') return this.attentionControl || null;
        return null;
      },
      querySelectorAll() { return []; },
      dispatchBubbling(name, values = {}) {
        const event = {
          target: this,
          key: values.key,
          defaultPrevented: false,
          propagationStopped: false,
          preventDefault() { this.defaultPrevented = true; },
          stopPropagation() { this.propagationStopped = true; },
        };
        let current = this;
        while (current && !event.propagationStopped) {
          for (const listener of listenersFor(current, name)) listener(event);
          current = current.parentNode;
        }
        if (name === 'keydown'
            && this.tagName === 'BUTTON'
            && (event.key === 'Enter' || event.key === ' ')
            && !event.defaultPrevented) {
          this.dispatchBubbling('click');
        }
        return event;
      },
    };
    function listenersFor(target, name) {
      return target === element ? (listeners.get(name) || []) : target._listeners(name);
    }
    element._listeners = (name) => listeners.get(name) || [];
    Object.defineProperty(element, 'innerHTML', {
      get() { return html; },
      set(value) {
        html = String(value);
        if (html.includes('data-transfer-id="upload-keyboard"')) {
          const button = eventElement('button');
          button.parentNode = element;
          element.attentionControl = button;
        }
      },
    });
    return element;
  }

  const tbody = eventElement('tbody');
  const paneRoot = eventElement('div');
  paneRoot.querySelector = (selector) => (selector === 'tbody' ? tbody : null);
  const keyboardActivations = [];
  const rowActivations = [];
  paneSandbox.document = { createElement: (tag) => eventElement(tag) };
  paneSandbox.termlabFilesPaneView.renderPane({
    isLocal: true,
    entries: [{ name: 'keyboard.txt', is_dir: false, size: 1, modified: 0 }],
    transferStatus: {
      'keyboard.txt': { status: 'attention', transferId: 'upload-keyboard' },
    },
    backStack: [],
    forwardStack: [],
    currentPath: '/tmp',
    pathInput: '/tmp',
    showHidden: true,
    colExt: false,
    colSize: false,
    colModified: false,
    error: null,
  }, paneRoot, {
    onActivateEntry(entry) { rowActivations.push(entry.name); },
    onTransferAttention(transferId) { keyboardActivations.push(transferId); },
  });
  const attentionButton = tbody.children[0].attentionControl;
  attentionButton.dispatchBubbling('keydown', { key: 'Enter' });
  assert.deepEqual(rowActivations, [], 'Enter on the attention button must not activate the file row');
  assert.deepEqual(keyboardActivations, ['upload-keyboard'], 'Enter activates the attention button');
}

// Queue phases must describe the real work instead of presenting all preflight
// and zero-byte states as a frozen 0%. A legacy zero-byte event follows each
// durable snapshot and must not erase that more precise phase.
{
  const phaseLocalPane = { transferStatus: {} };
  const phaseRemotePane = { transferStatus: {} };
  const phaseController = sandbox.window.termlabFilesTransfers.createController({
    localPane: phaseLocalPane,
    remotePane: phaseRemotePane,
  });
  const phaseJob = (kind, extra = {}) => ({
    id: 'large-upload',
    direction: 'upload',
    origin: { kind: 'filesPanel' },
    fileName: 'large.iso',
    state: { kind },
    bytesTransferred: 0,
    totalBytes: 1_000_000_000,
    ...extra,
  });

  phaseRemotePane.transferStatus['large.iso'] = {
    status: 'preparing', direction: 'upload', provisional: true,
  };
  phaseController.handleTransferSnapshot({ revision: 1, jobs: [phaseJob('queued')] });
  assert.deepEqual(JSON.parse(JSON.stringify(phaseRemotePane.transferStatus['large.iso'])), {
    status: 'preparing', direction: 'upload', transferId: 'large-upload',
  }, 'the first authoritative snapshot claims the provisional activity');

  phaseController.handleTransferSnapshot({ revision: 2, jobs: [phaseJob('running')] });
  assert.deepEqual(JSON.parse(JSON.stringify(phaseRemotePane.transferStatus['large.iso'])), {
    status: 'starting', direction: 'upload', transferId: 'large-upload',
  });
  phaseController.handleTransferProgress({
    payload: {
      transfer_id: 'large-upload',
      kind: 'upload',
      status: 'in_progress',
      bytes_transferred: 0,
      total_bytes: 1_000_000_000,
      file_name: 'large.iso',
      error: null,
    },
  });
  assert.equal(
    phaseRemotePane.transferStatus['large.iso'].status,
    'starting',
    'zero-byte compatibility progress preserves the authoritative starting phase',
  );

  phaseController.handleTransferProgress({
    payload: {
      transfer_id: 'large-upload',
      kind: 'upload',
      status: 'in_progress',
      bytes_transferred: 262_144,
      total_bytes: 1_000_000_000,
      file_name: 'large.iso',
      error: null,
    },
  });
  assert.equal(phaseRemotePane.transferStatus['large.iso'].status, 'in_progress');
  assert.equal(phaseRemotePane.transferStatus['large.iso'].direction, 'upload');
  assert.ok(
    phaseRemotePane.transferStatus['large.iso'].percent > 0
      && phaseRemotePane.transferStatus['large.iso'].percent < 1,
    'sub-one-percent byte progress is retained rather than rounded back to zero',
  );

  phaseController.handleTransferSnapshot({ revision: 3, jobs: [phaseJob('retryWaiting')] });
  assert.deepEqual(JSON.parse(JSON.stringify(phaseRemotePane.transferStatus['large.iso'])), {
    status: 'waiting', direction: 'upload', transferId: 'large-upload',
  }, 'retry backoff has a non-spinning waiting presentation');
}

// A repeat submission can share a filename with an older active/history job.
// Only the newly observed queue job may claim the provisional marker, and
// delayed compatibility progress from the older UUID must not erase it.
{
  const repeatRemotePane = { transferStatus: {} };
  const repeatController = sandbox.window.termlabFilesTransfers.createController({
    localPane: { transferStatus: {} },
    remotePane: repeatRemotePane,
  });
  const repeatJob = (id, kind) => ({
    id,
    direction: 'upload',
    origin: { kind: 'filesPanel' },
    fileName: 'repeat.iso',
    state: { kind },
    bytesTransferred: 0,
    totalBytes: 1_000_000_000,
  });

  repeatController.handleTransferSnapshot({ revision: 1, jobs: [repeatJob('old-upload', 'running')] });
  repeatRemotePane.transferStatus['repeat.iso'] = {
    status: 'preparing', direction: 'upload', provisional: true,
  };
  repeatController.handleTransferProgress({
    payload: {
      transfer_id: 'old-upload', kind: 'upload', status: 'completed',
      bytes_transferred: 1_000_000_000, total_bytes: 1_000_000_000,
      file_name: 'repeat.iso', error: null,
    },
  });
  assert.deepEqual(
    JSON.parse(JSON.stringify(repeatRemotePane.transferStatus['repeat.iso'] || null)),
    { status: 'preparing', direction: 'upload', provisional: true },
    'an older terminal event cannot clear a new provisional submission',
  );

  repeatController.handleTransferSnapshot({
    revision: 2,
    jobs: [repeatJob('old-upload', 'running'), repeatJob('new-upload', 'queued')],
  });
  assert.equal(
    repeatRemotePane.transferStatus['repeat.iso'].transferId,
    'new-upload',
    'only the newly observed same-name job claims the provisional submission',
  );
  repeatController.handleTransferProgress({
    payload: {
      transfer_id: 'old-upload', kind: 'upload', status: 'in_progress',
      bytes_transferred: 524_288, total_bytes: 1_000_000_000,
      file_name: 'repeat.iso', error: null,
    },
  });
  assert.equal(
    repeatRemotePane.transferStatus['repeat.iso'].transferId,
    'new-upload',
    'older byte progress cannot replace the newer queued transfer state',
  );
}

const localPane = { transferStatus: {} };
const remotePane = {
  transferStatus: {
    'current.txt': { status: 'in_progress', percent: 75, transferId: 'upload-1' },
  },
};
const refreshed = [];
const conflictDialogs = [];
const conflictResolutions = [];

// Restoring a persisted queue must paint the attention badge without opening a
// dialog during app startup. Only a live transition should interrupt the user.
{
  const restoredDialogs = [];
  const restoredController = sandbox.window.termlabFilesTransfers.createController({
    localPane: { transferStatus: {} },
    remotePane: { transferStatus: {} },
    toast: sandbox.window.toast,
    loadEntries() {},
    transferRuntime: { resolve() { return Promise.resolve(); } },
    transferDialogs: {
      showConflict(job) { restoredDialogs.push(job); },
    },
  });
  restoredController.handleTransferSnapshot({
    revision: 1,
    jobs: [{
      id: 'restored-attention',
      direction: 'upload',
      origin: { kind: 'filesPanel' },
      fileName: 'restored.txt',
      state: { kind: 'needsAttention', reason: { kind: 'destinationConflict' } },
    }],
  });
  assert.equal(restoredDialogs.length, 0, 'initial persisted attention stays quiet at startup');
}

const controller = sandbox.window.termlabFilesTransfers.createController({
  localPane,
  remotePane,
  toast: sandbox.window.toast,
  loadEntries(pane) { refreshed.push(pane); },
  transferRuntime: {
    resolve(transferId, resolution) {
      conflictResolutions.push({ transferId, resolution });
      return Promise.resolve();
    },
  },
  transferDialogs: {
    showConflict(job, invoker, onResolve) {
      conflictDialogs.push({ job, invoker, onResolve });
    },
  },
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
    state: { kind: 'needsAttention', reason: { kind: 'destinationConflict', resumeAvailable: false } },
  }],
});
assert.deepEqual(JSON.parse(JSON.stringify(remotePane.transferStatus['blocked.txt'])), {
  status: 'attention', percent: 0, transferId: 'upload-2',
});
assert.equal(conflictDialogs.length, 1, 'a live conflict opens its resolution dialog immediately');
assert.equal(conflictDialogs[0].job.id, 'upload-2');
assert.equal(conflictDialogs[0].invoker, null);
const conflictInvoker = { id: 'blocked-file-badge' };
assert.equal(controller.handleTransferAttention('upload-2', conflictInvoker), true);
assert.equal(conflictDialogs.length, 2);
assert.equal(conflictDialogs[1].job.id, 'upload-2');
assert.equal(conflictDialogs[1].invoker, conflictInvoker);
await conflictDialogs[1].onResolve({ kind: 'overwrite' });
assert.deepEqual(conflictResolutions, [{
  transferId: 'upload-2',
  resolution: { kind: 'overwrite' },
}]);
controller.handleTransferProgress({
  payload: {
    transfer_id: 'upload-2',
    kind: 'upload',
    status: 'pending',
    bytes_transferred: 0,
    total_bytes: 100,
    file_name: 'blocked.txt',
    error: null,
  },
});
assert.deepEqual(
  JSON.parse(JSON.stringify(remotePane.transferStatus['blocked.txt'])),
  { status: 'attention', percent: 0, transferId: 'upload-2' },
  'the real queue-delta then legacy-pending event order preserves attention',
);

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
