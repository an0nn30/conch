// Run: node scripts/tests/test_transfer_center.mjs
//
// The Transfer Center is a projection over the shared transfer runtime. This
// harness loads the real view/controller IIFEs against a small DOM, then
// drives the runtime subscription exactly as a docked or panel-host mount
// would. Assertions stay on rendered behavior, not implementation source.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const FRONTEND = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const DIALOG_PATH = path.join(FRONTEND, 'app/features/transfers/dialogs.js');
const TRANSFER_DATA_PATH = path.join(FRONTEND, 'app/features/transfers/data-service.js');
const TRANSFER_STORE_PATH = path.join(FRONTEND, 'app/features/transfers/store.js');
const TRANSFER_RUNTIME_PATH = path.join(FRONTEND, 'app/features/transfers/runtime.js');
const VIEW_PATH = path.join(FRONTEND, 'app/features/transfers/view.js');
const PANEL_PATH = path.join(FRONTEND, 'app/panels/transfer-center.js');
const MANAGER_PATH = path.join(FRONTEND, 'app/layout/tool-window-manager.js');
const TOOL_RUNTIME_PATH = path.join(FRONTEND, 'app/tool-window-runtime.js');
let focusDocument = null;

function dataName(attribute) {
  return attribute.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function selectorParts(selector) {
  const match = /^([a-z0-9-]+)?(?:#([a-z0-9_-]+))?(?:\.([a-z0-9_-]+))?(?:\[([a-z0-9_-]+)(?:="([^"]*)")?\])?$/i.exec(selector);
  if (!match) throw new Error(`unsupported selector in test DOM: ${selector}`);
  return { tag: match[1], id: match[2], className: match[3], attr: match[4], attrValue: match[5] };
}

function makeElement(tag) {
  const listeners = new Map();
  const attributes = new Map();
  let ownText = '';
  let className = '';
  const element = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    parentNode: null,
    dataset: {},
    style: {},
    id: '',
    hidden: false,
    disabled: false,
    value: 0,
    max: 0,
    tabIndex: -1,
    _textContentWriteCount: 0,
    _focusCount: 0,
    get isConnected() {
      let current = this;
      while (current && current.parentNode) current = current.parentNode;
      return !!current && current.tagName === 'BODY';
    },
    get className() { return className; },
    set className(value) { className = String(value || ''); },
    classList: {
      add(...names) {
        const values = new Set(className.split(/\s+/).filter(Boolean));
        names.forEach((name) => values.add(name));
        className = Array.from(values).join(' ');
      },
      remove(...names) {
        const removed = new Set(names);
        className = className.split(/\s+/).filter((name) => name && !removed.has(name)).join(' ');
      },
      toggle(name, force) {
        const exists = this.contains(name);
        const next = force === undefined ? !exists : !!force;
        if (next) this.add(name); else this.remove(name);
        return next;
      },
      contains(name) { return className.split(/\s+/).includes(name); },
    },
    get textContent() {
      return ownText + this.children.map((child) => child.textContent || '').join('');
    },
    set textContent(value) {
      this._textContentWriteCount += 1;
      for (const child of this.children) child.parentNode = null;
      this.children = [];
      ownText = String(value == null ? '' : value);
    },
    appendChild(child) {
      if (child.parentNode) child.parentNode.removeChild(child);
      this.children.push(child);
      child.parentNode = this;
      return child;
    },
    get firstChild() { return this.children[0] || null; },
    insertBefore(child, reference) {
      if (!reference) return this.appendChild(child);
      if (child.parentNode) child.parentNode.removeChild(child);
      const index = this.children.indexOf(reference);
      if (index < 0) return this.appendChild(child);
      this.children.splice(index, 0, child);
      child.parentNode = this;
      return child;
    },
    removeChild(child) {
      if (focusDocument && child._contains(focusDocument.activeElement)) {
        focusDocument.activeElement = focusDocument.body;
      }
      const index = this.children.indexOf(child);
      if (index >= 0) this.children.splice(index, 1);
      child.parentNode = null;
      return child;
    },
    replaceChildren(...children) {
      for (const child of this.children) {
        if (focusDocument && child._contains(focusDocument.activeElement)) {
          focusDocument.activeElement = focusDocument.body;
        }
        child.parentNode = null;
      }
      this.children = [];
      ownText = '';
      children.forEach((child) => this.appendChild(child));
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    focus() {
      this._focusCount += 1;
      if (focusDocument && this.isConnected) focusDocument.activeElement = this;
    },
    _contains(candidate) {
      let current = candidate;
      while (current) {
        if (current === this) return true;
        current = current.parentNode;
      }
      return false;
    },
    setAttribute(name, value) {
      const stringValue = String(value);
      attributes.set(name, stringValue);
      if (name === 'id') this.id = stringValue;
      else if (name === 'class') this.className = stringValue;
      else if (name === 'tabindex') this.tabIndex = Number(stringValue);
      else if (name.startsWith('data-')) this.dataset[dataName(name)] = stringValue;
    },
    getAttribute(name) {
      if (name === 'id' && this.id) return this.id;
      if (name === 'class' && this.className) return this.className;
      if (name.startsWith('data-')) {
        const value = this.dataset[dataName(name)];
        return value === undefined ? null : String(value);
      }
      return attributes.has(name) ? attributes.get(name) : null;
    },
    matches(selector) {
      const parts = selectorParts(selector);
      if (parts.tag && this.tagName !== parts.tag.toUpperCase()) return false;
      if (parts.id && this.id !== parts.id) return false;
      if (parts.className && !this.classList.contains(parts.className)) return false;
      if (parts.attr) {
        const value = this.getAttribute(parts.attr);
        if (value === null) return false;
        if (parts.attrValue !== undefined && value !== parts.attrValue) return false;
      }
      return true;
    },
    closest(selector) {
      let current = this;
      while (current) {
        if (current.matches(selector)) return current;
        current = current.parentNode;
      }
      return null;
    },
    querySelectorAll(selector) {
      const found = [];
      const visit = (node) => {
        for (const child of node.children) {
          if (child.matches(selector)) found.push(child);
          visit(child);
        }
      };
      visit(this);
      return found;
    },
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; },
    addEventListener(name, handler) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(handler);
    },
    removeEventListener(name, handler) {
      const values = listeners.get(name) || [];
      const index = values.indexOf(handler);
      if (index >= 0) values.splice(index, 1);
    },
    _fire(name, event = {}) {
      for (const handler of listeners.get(name) || []) handler({ target: this, ...event });
    },
    _listenerCount(name) { return (listeners.get(name) || []).length; },
  };
  return element;
}

function summary(values = {}) {
  return {
    queued: 0,
    running: 0,
    paused: 0,
    attention: 0,
    failed: 0,
    active: 0,
    history: 0,
    queuePaused: false,
    ...values,
  };
}

function job(id, kind, values = {}) {
  return {
    id,
    protocol: 'sftp',
    direction: 'upload',
    origin: { kind: 'filesPanel' },
    endpoint: { kind: 'configured', serverEntryId: 'server-1', label: 'Production' },
    localPath: `/local/${id}.bin`,
    remotePath: `/remote/${id}.bin`,
    fileName: `${id}.bin`,
    batchId: null,
    priority: 'normal',
    queueOrder: 1,
    hostKey: 'configured:server-1',
    destinationKey: `configured:server-1:/remote/${id}.bin`,
    state: { kind },
    durableCheckpoint: 0,
    bytesTransferred: 25,
    totalBytes: 100,
    speedBytesPerSecond: 1024,
    etaSeconds: 12,
    createdAtMs: 1,
    updatedAtMs: 2,
    ...values,
  };
}

function snapshot(jobs, values = {}) {
  return {
    revision: 1,
    queuePaused: false,
    settings: { globalLimit: 3, perHostLimit: 2 },
    jobs,
    batches: [],
    summary: summary(),
    recoveryError: null,
    ...values,
  };
}

function batchAgg(id, overrides = {}) {
  const infoOverrides = overrides.info || {};
  const rest = { ...overrides };
  delete rest.info;
  return {
    info: {
      id,
      name: `${id}-batch`,
      direction: 'upload',
      expansion: { kind: 'running' },
      discoveredFiles: 5,
      discoveredBytes: 500,
      skipped: [],
      createdAtMs: 1,
      ...infoOverrides,
    },
    filesDone: 3,
    bytesDone: 300,
    speedBytesPerSecond: 1024,
    etaSeconds: 5,
    ...rest,
  };
}

function loadHarness(options = {}) {
  const body = makeElement('body');
  const shellSentinel = makeElement('div');
  shellSentinel.id = 'shell-sentinel';
  body.appendChild(shellSentinel);
  const panelEl = makeElement('div');
  panelEl.id = 'transfer-center-panel';
  body.appendChild(panelEl);

  let subscriber = null;
  const runtimeCalls = [];
  const dialogs = [];
  const actionEvents = [];
  const selectionEvents = [];
  const runtime = {
    subscribe(handler) {
      subscriber = handler;
      return () => { subscriber = null; };
    },
  };
  for (const method of [
    'pause', 'resume', 'cancel', 'cancelBatch', 'cancelAll', 'retry', 'resolve', 'pauseAll',
    'resumeAll', 'clearCompleted', 'reorder', 'setPriority', 'updateSettings', 'reconnect',
  ]) {
    runtime[method] = (...args) => {
      runtimeCalls.push({ method, args });
      return Promise.resolve();
    };
  }

  const documentStub = {
    body,
    activeElement: body,
    createElement: (tag) => makeElement(tag),
    getElementById: (id) => body.querySelector(`#${id}`),
  };
  focusDocument = documentStub;
  const sandbox = {
    console,
    Promise,
    Number,
    Math,
    Set,
    Map,
    document: documentStub,
    utils: {
      formatSize(value) { return `${Number(value)} B`; },
    },
    termlabTransferRuntime: runtime,
    tlDialog: {
      open(dialogOptions) {
        const root = makeElement('div');
        const bodyEl = makeElement('div');
        root.appendChild(bodyEl);
        let closed = false;
        const handle = {
          el: root,
          close(result) {
            if (closed) return;
            closed = true;
            if (typeof dialogOptions.onClose === 'function') dialogOptions.onClose(result);
          },
        };
        if (typeof dialogOptions.body === 'function') dialogOptions.body(bodyEl);
        dialogs.push({ options: dialogOptions, handle, bodyEl });
        return handle;
      },
    },
  };
  Object.assign(sandbox, options.globals || {});
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  vm.createContext(sandbox);

  assert.ok(fs.existsSync(VIEW_PATH), 'Transfer Center view IIFE must exist');
  assert.ok(fs.existsSync(PANEL_PATH), 'Transfer Center panel IIFE must exist');
  assert.ok(fs.existsSync(DIALOG_PATH), 'Transfer Center dialogs IIFE must exist');
  vm.runInContext(fs.readFileSync(DIALOG_PATH, 'utf8'), sandbox, { filename: DIALOG_PATH });
  vm.runInContext(fs.readFileSync(VIEW_PATH, 'utf8'), sandbox, { filename: VIEW_PATH });
  if (typeof options.configureView === 'function') {
    options.configureView(sandbox.termlabTransferCenterView);
  }
  vm.runInContext(fs.readFileSync(PANEL_PATH, 'utf8'), sandbox, { filename: PANEL_PATH });
  const controller = sandbox.transferCenterPanel.init({
    panelEl,
    invoke: () => { throw new Error('the Transfer Center must not invoke Tauri directly'); },
    listen: () => { throw new Error('the Transfer Center must not listen to Tauri directly'); },
    onAction: (event) => {
      actionEvents.push(event);
      if (typeof options.onAction === 'function') options.onAction(event);
    },
    onSelect: (id) => {
      selectionEvents.push(id);
      if (typeof options.onSelect === 'function') options.onSelect(id);
    },
  });

  return {
    sandbox,
    panelEl,
    shellSentinel,
    runtimeCalls,
    controller,
    actionEvents,
    selectionEvents,
    dialogs,
    emit(value) {
      assert.ok(subscriber, 'controller must subscribe to the shared transfer runtime');
      subscriber(value);
    },
    subscriptionCount: () => (subscriber ? 1 : 0),
  };
}

function actionNames(row) {
  return row.querySelectorAll('[data-transfer-action]').map((button) => button.dataset.transferAction);
}

async function loadReconnectHarness(startingError) {
  const calls = [];
  const authCalls = [];
  const filesData = {
    connectHost(invoke, serverEntryId) {
      calls.push({ command: 'files.connectHost', serverEntryId, invoke });
      return Promise.reject(startingError);
    },
  };
  const auth = {
    run(serverEntryId, error, ctx) {
      authCalls.push({ serverEntryId, error, ctx });
      return Promise.resolve({ sessionKey: 'won-session' });
    },
  };
  let queueState = 'needsConnection';
  const invoke = (command, args) => {
    calls.push({ command, args });
    if (command === 'transfer_queue_snapshot') return Promise.resolve(snapshot([]));
    if (command === 'transfer_resume') {
      if (queueState !== 'paused') return Promise.reject(new Error(`resume is illegal from ${queueState}`));
      queueState = 'queued';
    }
    if (command === 'transfer_retry') {
      if (queueState !== 'needsConnection') return Promise.reject(new Error(`retry is illegal from ${queueState}`));
      queueState = 'queued';
    }
    return Promise.resolve(undefined);
  };
  const sandbox = {
    console,
    Promise,
    Set,
    Map,
    structuredClone,
    setTimeout,
    clearTimeout,
    termlabFilesFeatureDataService: filesData,
    termlabConnectAuth: auth,
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const modulePath of [TRANSFER_DATA_PATH, TRANSFER_STORE_PATH, TRANSFER_RUNTIME_PATH]) {
    vm.runInContext(fs.readFileSync(modulePath, 'utf8'), sandbox, { filename: modulePath });
  }
  await sandbox.termlabTransferRuntime.ensureStarted({
    invoke,
    listen: () => Promise.resolve(() => {}),
  });
  return { sandbox, calls, authCalls, filesData, invoke, queueState: () => queueState };
}

function rowIds(panelEl) {
  return panelEl.querySelectorAll('tr[data-job-id]').map((row) => row.dataset.jobId);
}

// The backend state is the only action-availability authority. Every state
// gets its exact supported surface; terminal rows cannot accidentally expose
// lifecycle commands, and conflict resume is not inferred from checkpoints.
{
  const harness = loadHarness();
  harness.emit(snapshot([
    job('running', 'running'),
    job('paused', 'paused'),
    job('failed', 'failed', { state: { kind: 'failed', error: 'disk full' } }),
    job('connection', 'needsConnection'),
    job('attention', 'needsAttention', { state: { kind: 'needsAttention', reason: { kind: 'destinationConflict', resumeAvailable: false } } }),
    job('queued', 'queued'),
    job('completed', 'completed'),
    job('cancelled', 'cancelled'),
  ], { summary: summary({ active: 6, history: 2 }) }));

  const actions = (id) => actionNames(harness.panelEl.querySelector(`tr[data-job-id="${id}"]`));
  assert.deepStrictEqual(actions('running'), ['pause', 'cancel']);
  assert.deepStrictEqual(actions('paused'), ['resume', 'cancel']);
  assert.deepStrictEqual(actions('connection'), ['connect', 'cancel']);
  assert.deepStrictEqual(actions('attention'), ['resolve', 'cancel']);
  assert.deepStrictEqual(actions('queued'), ['pause', 'toggle-priority', 'move-up', 'move-down', 'cancel']);

  click(harness.panelEl, harness.panelEl.querySelector('[data-transfer-view="history"]'));
  assert.deepStrictEqual(actions('failed'), ['retry', 'details']);
  assert.deepStrictEqual(actions('completed'), ['details']);
  assert.deepStrictEqual(actions('cancelled'), ['details']);
}

function click(panelEl, target) {
  panelEl._fire('click', { target });
}

function dialogButton(dialog, label) {
  const button = (dialog.options.buttons || []).find((item) => item.label === label);
  assert.ok(button, `dialog must expose ${label}`);
  return button;
}

// Configured reconnects use the files service first, preserve the typed
// starting error for the shared auth chain, and requeue only after a session
// is actually won.
{
  const startingError = { kind: 'needsPassword', hasVaultAccount: true, message: 'Password required' };
  const harness = await loadReconnectHarness(startingError);
  const configured = job('reconnect', 'needsConnection');
  await harness.sandbox.termlabTransferRuntime.reconnect(configured);
  assert.strictEqual(harness.authCalls.length, 1);
  assert.strictEqual(harness.authCalls[0].serverEntryId, 'server-1');
  assert.strictEqual(harness.authCalls[0].error, startingError);
  assert.strictEqual(harness.authCalls[0].ctx.invoke, harness.invoke);
  assert.strictEqual(harness.authCalls[0].ctx.data, harness.filesData);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(harness.calls.filter((call) => call.command === 'transfer_retry'))), [
    { command: 'transfer_retry', args: { transferId: 'reconnect' } },
  ]);
  assert.strictEqual(harness.queueState(), 'queued');
  assert.ok(!harness.calls.some((call) => call.command === 'transfer_resume'));
}

// Ad-hoc endpoints never enter the configured-host runtime path. The dialog
// gives the exact matching session identity and makes the no-storage behavior
// explicit.
{
  const harness = loadHarness();
  let queueState = 'needsConnection';
  harness.sandbox.termlabTransferRuntime.retry = (id) => {
    if (queueState !== 'needsConnection') {
      return Promise.reject(new Error(`retry is illegal from ${queueState}`));
    }
    queueState = 'queued';
    harness.runtimeCalls.push({ method: 'retry', args: [id] });
    return Promise.resolve();
  };
  const adHoc = job('adhoc', 'needsConnection', {
    endpoint: { kind: 'adHoc', user: 'sam', host: 'files.example', port: 2222 },
  });
  harness.emit(snapshot([adHoc], { summary: summary({ active: 1, attention: 1 }) }));
  click(harness.panelEl, harness.panelEl.querySelector('[data-transfer-action="connect"]'));
  assert.deepStrictEqual(harness.runtimeCalls, []);
  const text = harness.dialogs.at(-1).bodyEl.textContent;
  assert.ok(text.includes('sam@files.example:2222'));
  assert.ok(text.includes('will not store credentials'));
  assert.ok(text.includes('choose Requeue transfer'));
  await dialogButton(harness.dialogs.at(-1), 'Requeue transfer').onSelect();
  assert.strictEqual(queueState, 'queued');
  assert.deepStrictEqual(harness.runtimeCalls, [{ method: 'retry', args: ['adhoc'] }]);
}

// Delegated controls acknowledge exactly one runtime command and keep the
// backend projection untouched until a new snapshot arrives.
{
  const harness = loadHarness();
  harness.emit(snapshot([
    job('a', 'running'),
    job('b', 'queued', { queueOrder: 2, priority: 'normal' }),
    job('c', 'queued', { queueOrder: 3, priority: 'normal' }),
  ], { summary: summary({ active: 3, running: 1, queued: 2 }) }));

  click(harness.panelEl, harness.panelEl.querySelector('tr[data-job-id="a"]').querySelector('[data-transfer-action="pause"]'));
  assert.deepStrictEqual(harness.runtimeCalls, [{ method: 'pause', args: ['a'] }]);
  assert.deepStrictEqual(actionNames(harness.panelEl.querySelector('tr[data-job-id="a"]')), ['pause', 'cancel'],
    'command acknowledgement must not optimistically rewrite backend-owned state');

  click(harness.panelEl, harness.panelEl.querySelector('tr[data-job-id="b"]').querySelector('[data-transfer-action="toggle-priority"]'));
  click(harness.panelEl, harness.panelEl.querySelector('tr[data-job-id="c"]').querySelector('[data-transfer-action="move-up"]'));
  assert.deepStrictEqual(harness.runtimeCalls.slice(1), [
    { method: 'setPriority', args: ['b', 'interactive'] },
    { method: 'reorder', args: ['c', 'b'] },
  ]);

  click(harness.panelEl, harness.panelEl.querySelector('[data-transfer-action="pause-all"]'));
  assert.deepStrictEqual(harness.runtimeCalls.at(-1), { method: 'pauseAll', args: [] });
}

// Cancel is explicit and names the file. Escape/cancel never mutates, input
// data is cleared on close, and focus returns to the invoking control.
{
  const harness = loadHarness();
  const invoker = makeElement('button');
  harness.sandbox.document.body.appendChild(invoker);
  const calls = [];
  harness.sandbox.termlabTransferDialogs.showCancel(
    job('danger', 'running', { fileName: '<danger>.key' }),
    invoker,
    () => calls.push('cancel'),
  );
  const first = harness.dialogs.at(-1);
  assert.ok(first.bodyEl.textContent.includes('<danger>.key'));
  assert.ok(dialogButton(first, 'Cancel transfer').danger);
  first.handle.close('escape');
  assert.deepStrictEqual(calls, []);
  assert.strictEqual(invoker._focusCount, 1);

  harness.sandbox.termlabTransferDialogs.showCancel(job('danger', 'running'), invoker, () => calls.push('cancel'));
  const confirmed = harness.dialogs.at(-1);
  await dialogButton(confirmed, 'Cancel transfer').onSelect();
  assert.deepStrictEqual(calls, ['cancel']);
}

// Controller-owned confirmations and settings call the runtime once after
// confirmation, using one atomic settings payload and no optimistic render.
{
  const harness = loadHarness();
  harness.emit(snapshot([
    job('running', 'running'),
    job('attention', 'needsAttention', {
      state: { kind: 'needsAttention', reason: { kind: 'destinationConflict', resumeAvailable: false } },
    }),
  ], {
    settings: { globalLimit: 3, perHostLimit: 2 },
    summary: summary({ active: 2, running: 1, attention: 1 }),
  }));

  click(harness.panelEl, harness.panelEl.querySelector('tr[data-job-id="running"]').querySelector('[data-transfer-action="cancel"]'));
  assert.deepStrictEqual(harness.runtimeCalls, []);
  await dialogButton(harness.dialogs.at(-1), 'Cancel transfer').onSelect();
  assert.deepStrictEqual(harness.runtimeCalls, [{ method: 'cancel', args: ['running'] }]);

  click(harness.panelEl, harness.panelEl.querySelector('tr[data-job-id="attention"]').querySelector('[data-transfer-action="resolve"]'));
  await dialogButton(harness.dialogs.at(-1), 'Overwrite').onSelect();
  assert.deepStrictEqual(JSON.parse(JSON.stringify(harness.runtimeCalls.at(-1))), {
    method: 'resolve', args: ['attention', { kind: 'overwrite' }],
  });

  click(harness.panelEl, harness.panelEl.querySelector('[data-transfer-action="concurrency"]'));
  const concurrency = harness.dialogs.at(-1);
  concurrency.bodyEl.querySelector('[data-transfer-field="global-limit"]').value = '6';
  concurrency.bodyEl.querySelector('[data-transfer-field="per-host-limit"]').value = '3';
  concurrency.bodyEl.querySelector('[data-transfer-field="pipeline-depth"]').value = '8';
  concurrency.bodyEl.querySelector('[data-transfer-field="pipeline-chunk-kib"]').value = '512';
  concurrency.bodyEl.querySelector('[data-transfer-field="max-queued"]').value = '5000';
  await dialogButton(concurrency, 'Save').onSelect();
  assert.deepStrictEqual(JSON.parse(JSON.stringify(harness.runtimeCalls.at(-1))), {
    method: 'updateSettings',
    args: [{
      globalLimit: 6, perHostLimit: 3, pipelineDepth: 8,
      pipelineChunkBytes: 512 * 1024, maxQueued: 5000,
    }],
  });

  const priorCallCount = harness.runtimeCalls.length;
  click(harness.panelEl, harness.panelEl.querySelector('[data-transfer-action="concurrency"]'));
  const invalidConcurrency = harness.dialogs.at(-1);
  invalidConcurrency.bodyEl.querySelector('[data-transfer-field="pipeline-depth"]').value = '0';
  await dialogButton(invalidConcurrency, 'Save').onSelect();
  const concurrencyError = invalidConcurrency.bodyEl.querySelector('[data-transfer-error="concurrency"]');
  assert.strictEqual(concurrencyError.hidden, false);
  assert.strictEqual(harness.runtimeCalls.length, priorCallCount, 'invalid pipeline depth records no updateSettings call');
}

// Mutating dialogs restore focus through the live keyed table, not a detached
// action button captured before the backend event patched or removed its row.
{
  const harness = loadHarness();
  const running = job('a', 'running');
  const sibling = job('b', 'running', { queueOrder: 2 });
  harness.emit(snapshot([running, sibling], {
    summary: summary({ active: 2, running: 2 }),
  }));
  const oldButton = harness.panelEl.querySelector('tr[data-job-id="a"]').querySelector('[data-transfer-action="cancel"]');
  oldButton.focus();
  click(harness.panelEl, oldButton);
  harness.sandbox.termlabTransferRuntime.cancel = () => {
    harness.emit(snapshot([
      job('a', 'paused'),
      sibling,
    ], { revision: 2, summary: summary({ active: 2, running: 1, paused: 1 }) }));
    return Promise.resolve();
  };
  await dialogButton(harness.dialogs.at(-1), 'Cancel transfer').onSelect();
  const liveRow = harness.panelEl.querySelector('tr[data-job-id="a"]');
  assert.strictEqual(oldButton.isConnected, false);
  assert.strictEqual(harness.sandbox.document.activeElement, liveRow,
    'replaced action button falls back to its still-connected keyed row');
}

// Files-pane conflict dialogs use the same shared dialog helper. When a queue
// update rerenders the file row while the dialog is open, focus returns to the
// replacement attention control rather than being stranded on the document.
{
  const harness = loadHarness();
  const pane = makeElement('div');
  pane.className = 'fp-pane';
  harness.sandbox.document.body.appendChild(pane);

  const oldRow = makeElement('tr');
  oldRow.setAttribute('data-name', 'blocked.txt');
  const oldButton = makeElement('button');
  oldButton.className = 'fp-transfer-attention';
  oldButton.setAttribute('data-transfer-id', 'upload-2');
  oldRow.appendChild(oldButton);
  pane.appendChild(oldRow);
  oldButton.focus();

  harness.sandbox.termlabTransferDialogs.showConflict(job('upload-2', 'needsAttention', {
    fileName: 'blocked.txt',
    state: { kind: 'needsAttention', reason: { kind: 'destinationConflict', resumeAvailable: false } },
  }), oldButton, () => {});

  const replacementRow = makeElement('tr');
  replacementRow.setAttribute('data-name', 'blocked.txt');
  const replacementButton = makeElement('button');
  replacementButton.className = 'fp-transfer-attention';
  replacementButton.setAttribute('data-transfer-id', 'upload-2');
  replacementRow.appendChild(replacementButton);
  pane.replaceChildren(replacementRow);

  await dialogButton(harness.dialogs.at(-1), 'Cancel').onSelect();
  assert.strictEqual(harness.sandbox.document.activeElement, replacementButton,
    'Files conflict close restores focus to the replacement badge after rerender');
}

{
  const harness = loadHarness();
  const sibling = job('b', 'running', { queueOrder: 2 });
  harness.emit(snapshot([job('a', 'running'), sibling], {
    summary: summary({ active: 2, running: 2 }),
  }));
  const oldButton = harness.panelEl.querySelector('tr[data-job-id="a"]').querySelector('[data-transfer-action="cancel"]');
  oldButton.focus();
  click(harness.panelEl, oldButton);
  harness.sandbox.termlabTransferRuntime.cancel = () => {
    harness.emit(snapshot([
      job('a', 'cancelled', { state: { kind: 'cancelled', cleanupError: null } }),
      sibling,
    ], { revision: 2, summary: summary({ active: 1, history: 1, running: 1 }) }));
    return Promise.resolve();
  };
  await dialogButton(harness.dialogs.at(-1), 'Cancel transfer').onSelect();
  assert.strictEqual(harness.sandbox.document.activeElement, harness.panelEl.querySelector('tr[data-job-id="b"]'),
    'removed row falls forward to the next connected row');
}

// Details are useful for diagnosis but deliberately omit credential-shaped
// fields even when an over-complete object reaches the UI.
{
  const harness = loadHarness();
  const detailed = job('details', 'failed', {
    state: { kind: 'failed', error: 'permission denied' },
    durableCheckpoint: 42,
    endpoint: {
      kind: 'configured', serverEntryId: 'server-1', label: 'Production',
      password: 'pw-secret', passphrase: 'phrase-secret', privateKey: 'key-secret',
      vaultHandle: 'vault-secret', liveHandle: 'live-secret',
    },
  });
  harness.sandbox.termlabTransferDialogs.showDetails(detailed, makeElement('tr'));
  const text = harness.dialogs.at(-1).bodyEl.textContent;
  for (const visible of ['Production', '/local/details.bin', '/remote/details.bin', '42', 'permission denied']) {
    assert.ok(text.includes(visible), `details must include ${visible}`);
  }
  for (const secret of ['pw-secret', 'phrase-secret', 'key-secret', 'vault-secret', 'live-secret']) {
    assert.ok(!text.includes(secret), `details must exclude ${secret}`);
  }
}

// Conflict choices come solely from the backend reason. Rename validates
// nonblank destinations and concurrency commits its two limits atomically.
{
  const harness = loadHarness();
  const resolutions = [];
  const conflict = job('conflict', 'needsAttention', {
    state: { kind: 'needsAttention', reason: { kind: 'destinationConflict', resumeAvailable: false } },
  });
  harness.sandbox.termlabTransferDialogs.showConflict(conflict, makeElement('tr'), (value) => resolutions.push(value));
  const first = harness.dialogs.at(-1);
  assert.deepStrictEqual(Array.from(first.options.buttons, (button) => button.label), ['Cancel', 'Skip', 'Rename', 'Overwrite']);
  assert.ok(dialogButton(first, 'Overwrite').danger);
  const renameInput = first.bodyEl.querySelector('[data-transfer-field="rename"]');
  const renameError = first.bodyEl.querySelector('[data-transfer-error="rename"]');
  renameInput.value = '   ';
  await dialogButton(first, 'Rename').onSelect();
  assert.deepStrictEqual(resolutions, []);
  assert.strictEqual(renameError.hidden, false);
  renameInput.value = '/remote/renamed.bin';
  await dialogButton(first, 'Rename').onSelect();
  assert.deepStrictEqual(JSON.parse(JSON.stringify(resolutions)), [{ kind: 'rename', destination: '/remote/renamed.bin' }]);
  assert.strictEqual(renameInput.value, '', 'rename input is cleared on close');

  harness.sandbox.termlabTransferDialogs.showConflict(job('resume', 'needsAttention', {
    state: { kind: 'needsAttention', reason: { kind: 'destinationConflict', resumeAvailable: true } },
  }), makeElement('tr'), () => {});
  assert.deepStrictEqual(Array.from(harness.dialogs.at(-1).options.buttons, (button) => button.label),
    ['Cancel', 'Skip', 'Resume', 'Rename', 'Overwrite']);

  harness.sandbox.termlabTransferDialogs.showConflict(job('changed', 'needsAttention', {
    state: { kind: 'needsAttention', reason: { kind: 'sourceChanged' } },
  }), makeElement('tr'), () => {});
  assert.deepStrictEqual(Array.from(harness.dialogs.at(-1).options.buttons, (button) => button.label), ['Cancel', 'Skip', 'Restart']);

  const settings = [];
  harness.sandbox.termlabTransferDialogs.showConcurrency(
    { globalLimit: 3, perHostLimit: 2 },
    makeElement('button'),
    (value) => settings.push(value),
  );
  const concurrency = harness.dialogs.at(-1);
  const globalInput = concurrency.bodyEl.querySelector('[data-transfer-field="global-limit"]');
  const hostInput = concurrency.bodyEl.querySelector('[data-transfer-field="per-host-limit"]');
  const depthInput = concurrency.bodyEl.querySelector('[data-transfer-field="pipeline-depth"]');
  const chunkInput = concurrency.bodyEl.querySelector('[data-transfer-field="pipeline-chunk-kib"]');
  const settingsError = concurrency.bodyEl.querySelector('[data-transfer-error="concurrency"]');
  // The pipelined engine clamps chunks to the raw SFTP cap unless the server
  // advertises a higher limit, so the field says so rather than silently
  // accepting a size it will not use. Copy only — the range is unchanged.
  assert.strictEqual(
    chunkInput.parentNode.querySelector('.tl-field__label').textContent,
    'Chunk size (KiB, servers may cap at 255)',
    'the chunk field must warn that servers may cap the configured size');
  assert.strictEqual(chunkInput.getAttribute('max'), '1024',
    'the cap warning must not change what the field accepts');
  globalInput.value = '0';
  hostInput.value = '2.5';
  await dialogButton(concurrency, 'Save').onSelect();
  assert.deepStrictEqual(settings, []);
  assert.strictEqual(settingsError.hidden, false);
  globalInput.value = '8';
  hostInput.value = '4';
  depthInput.value = '12';
  chunkInput.value = '128';
  await dialogButton(concurrency, 'Save').onSelect();
  assert.deepStrictEqual(JSON.parse(JSON.stringify(settings)),
    [{
      globalLimit: 8, perHostLimit: 4, pipelineDepth: 12,
      pipelineChunkBytes: 128 * 1024, maxQueued: 2000,
    }]);
  assert.strictEqual(globalInput.value, '');
  assert.strictEqual(hostInput.value, '');
  assert.strictEqual(depthInput.value, '');
  assert.strictEqual(chunkInput.value, '');
}

// Every backend attention reason has its own safe copy and resolution set.
// Recovery/cleanup messages survive verbatim; destination-only mutations can
// never leak onto source/recovery reasons.
{
  const harness = loadHarness();
  const cases = [
    {
      reason: { kind: 'sourceChanged', expected: { size: 10, modifiedToken: 'old' }, actual: { size: 12, modifiedToken: 'new' } },
      title: 'Source changed', buttons: ['Cancel', 'Skip', 'Restart'], copy: ['Expected size', '10', 'Current size', '12'],
    },
    {
      reason: { kind: 'sourceCannotResume' },
      title: 'Resume unavailable', buttons: ['Cancel', 'Skip', 'Restart'], copy: ['cannot be safely resumed'],
    },
    {
      reason: { kind: 'sourceMissing' },
      title: 'Source missing', buttons: ['Cancel', 'Skip', 'Restart'], copy: ['source path no longer exists'],
    },
    {
      reason: { kind: 'missingPartial' },
      title: 'Partial file missing', buttons: ['Cancel', 'Skip', 'Restart'], copy: ['managed partial file is missing'],
    },
    {
      reason: { kind: 'commitRecovery', message: 'backup and final both remain; preserve both' },
      title: 'Commit recovery required', buttons: ['Cancel', 'Skip', 'Restart'], copy: ['backup and final both remain; preserve both'],
    },
    {
      reason: { kind: 'cleanup', message: 'partial cleanup failed with EACCES' },
      title: 'Cleanup required', buttons: ['Cancel', 'Skip', 'Restart'], copy: ['partial cleanup failed with EACCES'],
    },
  ];

  for (const item of cases) {
    harness.sandbox.termlabTransferDialogs.showConflict(job(item.reason.kind, 'needsAttention', {
      state: { kind: 'needsAttention', reason: item.reason },
    }), makeElement('tr'), () => {});
    const dialog = harness.dialogs.at(-1);
    assert.strictEqual(dialog.options.title, item.title);
    assert.deepStrictEqual(Array.from(dialog.options.buttons, (button) => button.label), item.buttons);
    assert.ok(!dialog.options.buttons.some((button) => ['Overwrite', 'Rename', 'Resume'].includes(button.label)));
    for (const copy of item.copy) assert.ok(dialog.bodyEl.textContent.includes(copy));
  }
}

async function loadMountLifecycleHarness() {
  const body = makeElement('body');
  const appEl = makeElement('div');
  appEl.id = 'app';
  body.appendChild(appEl);
  for (const zoneName of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right']) {
    const zoneEl = makeElement('div');
    zoneEl.setAttribute('data-zone', zoneName);
    const tabsEl = makeElement('div');
    tabsEl.className = 'zone-tab-strip';
    const contentEl = makeElement('div');
    contentEl.className = 'zone-content';
    zoneEl.appendChild(tabsEl);
    zoneEl.appendChild(contentEl);
    appEl.appendChild(zoneEl);
  }

  let currentSnapshot = snapshot([job('a', 'running')], {
    summary: summary({ active: 1, running: 1 }),
  });
  const subscribers = new Set();
  const transferRuntime = {
    ensureStarted: () => Promise.resolve(currentSnapshot),
    subscribe(handler) {
      subscribers.add(handler);
      handler(currentSnapshot);
      return () => subscribers.delete(handler);
    },
  };
  const invokeCalls = [];
  const invoke = (command, args) => {
    invokeCalls.push({ command, args });
    if (command === 'open_panel_host') return Promise.resolve(17);
    return Promise.resolve(undefined);
  };
  const sandbox = {
    console,
    Promise,
    Number,
    Math,
    Set,
    Map,
    JSON,
    setTimeout,
    clearTimeout,
    innerWidth: 1200,
    innerHeight: 800,
    addEventListener() {},
    removeEventListener() {},
    document: {
      body,
      createElement: (tag) => makeElement(tag),
      createTextNode: (text) => ({ textContent: String(text), parentNode: null }),
      getElementById: (id) => body.querySelector(`#${id}`),
      querySelector: (selector) => body.querySelector(selector),
      querySelectorAll: (selector) => body.querySelectorAll(selector),
      addEventListener() {},
    },
    utils: { formatSize: (value) => `${Number(value)} B` },
    termlabTransferRuntime: transferRuntime,
    toast: {},
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  vm.createContext(sandbox);
  for (const modulePath of [VIEW_PATH, PANEL_PATH, MANAGER_PATH, TOOL_RUNTIME_PATH]) {
    vm.runInContext(fs.readFileSync(modulePath, 'utf8'), sandbox, { filename: modulePath });
  }

  sandbox.toolWindowManager.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  const toolRuntime = sandbox.termlabToolWindowRuntime.create({
    invoke,
    listen: () => Promise.resolve(() => {}),
    listenOnCurrentWindow: () => Promise.resolve(() => {}),
    layoutService: { getSavedLayout: () => Promise.resolve({}), saveLayout: () => {} },
    debouncedFitAndResize: () => {},
    getCurrentTab: () => null,
    getCurrentPane: () => null,
  });
  await toolRuntime.registerAll({ registrationsOnly: true });

  return {
    sandbox,
    body,
    invokeCalls,
    activeSubscribers: () => subscribers.size,
    emit(nextSnapshot) {
      currentSnapshot = nextSnapshot;
      for (const subscriber of Array.from(subscribers)) subscriber(nextSnapshot);
    },
  };
}

// Missing view/controller files or globals is the first RED. Once present,
// initialization must show an explicit loading state and install one event
// delegation surface plus one runtime projection subscription.
{
  const harness = loadHarness();
  assert.ok(harness.sandbox.termlabTransferCenterView);
  assert.ok(harness.sandbox.transferCenterPanel);
  assert.strictEqual(harness.panelEl.querySelector('[data-transfer-state="loading"]').textContent, 'Loading transfers…');
  assert.strictEqual(harness.panelEl._listenerCount('click'), 1,
    'one panel listener delegates tabs, selection, and action hooks');
  assert.strictEqual(harness.subscriptionCount(), 1);
  assert.deepStrictEqual(harness.runtimeCalls, [],
    'the read-only Task 11 slice must not optimistically call queue actions');
}

// A render result is the mount's disposal contract. Repeated dock -> window
// -> host -> dock transitions must keep exactly one live runtime subscriber,
// and removed DOM must stop receiving snapshots immediately.
{
  const harness = await loadMountLifecycleHarness();
  const manager = harness.sandbox.toolWindowManager;
  assert.ok(manager.listWindows().some((item) => item.id === 'transfer-center'),
    'generic tool-window shortcut inventory includes the registered Transfer Center id');
  manager.activate('transfer-center');
  assert.strictEqual(harness.activeSubscribers(), 1);
  const firstPanel = harness.body.querySelector('#transfer-center-panel');
  const firstSummary = firstPanel.querySelector('[aria-live="polite"]');

  manager.setViewMode('transfer-center', 'window');
  assert.strictEqual(harness.activeSubscribers(), 0,
    'detaching the docked DOM must destroy its controller subscription');
  assert.strictEqual(firstPanel._listenerCount('click'), 0);
  assert.strictEqual(firstPanel._listenerCount('keydown'), 0,
    'detaching removes both root-scoped delegated handlers');
  const detachedText = firstSummary.textContent;
  harness.emit(snapshot([job('a', 'running')], {
    revision: 2,
    summary: summary({ active: 2, running: 2 }),
  }));
  assert.strictEqual(firstSummary.textContent, detachedText,
    'a detached docked panel must stop receiving runtime snapshots');

  const registration = manager.getRegistration('transfer-center');
  const hostRoot = makeElement('div');
  const hostMount = registration.renderFn(hostRoot);
  const hostPanel = hostRoot.querySelector('#transfer-center-panel');
  assert.strictEqual(typeof hostMount.destroy, 'function',
    'the shared renderFn must return the controller disposal contract to a host');
  assert.strictEqual(harness.activeSubscribers(), 1);
  hostMount.destroy();
  assert.strictEqual(harness.activeSubscribers(), 0,
    'host teardown must release its controller subscription');
  assert.strictEqual(hostPanel._listenerCount('click'), 0);
  assert.strictEqual(hostPanel._listenerCount('keydown'), 0);

  manager.notifyHostDocked('transfer-center');
  assert.strictEqual(harness.activeSubscribers(), 1,
    'dock-back creates one fresh controller subscription');
  const secondPanel = harness.body.querySelector('#transfer-center-panel');
  assert.notStrictEqual(secondPanel, firstPanel);

  manager.setViewMode('transfer-center', 'window');
  assert.strictEqual(harness.activeSubscribers(), 0);
  const secondHostRoot = makeElement('div');
  const secondHostMount = registration.renderFn(secondHostRoot);
  const secondHostPanel = secondHostRoot.querySelector('#transfer-center-panel');
  assert.strictEqual(harness.activeSubscribers(), 1);
  secondHostMount.destroy();
  assert.strictEqual(harness.activeSubscribers(), 0);
  assert.strictEqual(secondHostPanel._listenerCount('click'), 0);
  assert.strictEqual(secondHostPanel._listenerCount('keydown'), 0,
    'repeated host mounts do not leak delegated handlers');
  manager.notifyHostDocked('transfer-center');
  assert.strictEqual(harness.activeSubscribers(), 1,
    'a second complete lifecycle still has exactly one live subscriber');
  manager.unregister('transfer-center');
  assert.strictEqual(harness.activeSubscribers(), 0,
    'unregistering a mounted tool window disposes its final controller');
}

// Row actions and selection share one delegated surface; the controller binds
// the selected backend job to exactly one runtime acknowledgement.
{
  const harness = loadHarness();
  harness.emit(snapshot([job('a', 'running')], { summary: summary({ active: 1, running: 1 }) }));
  const pause = harness.panelEl.querySelector('[data-transfer-action="pause"]');
  click(harness.panelEl, pause);
  assert.deepStrictEqual(harness.selectionEvents, ['a']);
  assert.strictEqual(harness.actionEvents[0].action, 'pause');
  assert.strictEqual(harness.actionEvents[0].jobId, 'a');
  assert.strictEqual(harness.actionEvents[0].invoker, pause);
  assert.deepStrictEqual(harness.runtimeCalls, [{ method: 'pause', args: ['a'] }]);
}

// Root-scoped keyboard flow moves selection, dispatches only eligible state
// actions, and leaves native inputs/buttons alone.
{
  const harness = loadHarness();
  harness.emit(snapshot([
    job('a', 'running'),
    job('b', 'paused', { queueOrder: 2 }),
    job('attention', 'needsAttention', {
      queueOrder: 3,
      state: { kind: 'needsAttention', reason: { kind: 'destinationConflict', resumeAvailable: false } },
    }),
    job('failed', 'failed', { state: { kind: 'failed', error: 'disk full' } }),
  ], { summary: summary({ active: 3, history: 1, running: 1, paused: 1, attention: 1, failed: 1 }) }));
  const rowA = harness.panelEl.querySelector('tr[data-job-id="a"]');
  const rowB = harness.panelEl.querySelector('tr[data-job-id="b"]');
  const rowAttention = harness.panelEl.querySelector('tr[data-job-id="attention"]');

  let arrowPrevented = 0;
  harness.panelEl._fire('keydown', {
    target: rowA,
    key: 'ArrowDown',
    preventDefault: () => { arrowPrevented += 1; },
  });
  assert.deepStrictEqual(harness.selectionEvents, ['b']);
  assert.strictEqual(rowB.getAttribute('aria-selected'), 'true');
  assert.strictEqual(rowA.getAttribute('aria-selected'), 'false');
  assert.strictEqual(rowB._focusCount, 1);
  assert.strictEqual(arrowPrevented, 1);

  let spacePrevented = 0;
  harness.panelEl._fire('keydown', {
    target: rowB,
    key: ' ',
    preventDefault: () => { spacePrevented += 1; },
  });
  assert.deepStrictEqual(harness.runtimeCalls, [{ method: 'resume', args: ['b'] }]);
  assert.strictEqual(spacePrevented, 1, 'Space must not scroll the transfer table');

  harness.panelEl._fire('keydown', {
    target: rowB,
    key: 'ArrowDown',
    preventDefault() {},
  });
  harness.panelEl._fire('keydown', {
    target: rowAttention,
    key: 'Enter',
    preventDefault() {},
  });
  assert.strictEqual(harness.dialogs.at(-1).options.title, 'Destination conflict');

  harness.panelEl._fire('keydown', { target: rowAttention, key: 'Delete', preventDefault() {} });
  assert.strictEqual(harness.dialogs.at(-1).options.title, 'Cancel transfer?');
  assert.deepStrictEqual(harness.runtimeCalls, [{ method: 'resume', args: ['b'] }],
    'Delete opens confirmation without cancelling');

  const nativeButton = rowAttention.querySelector('[data-transfer-action="resolve"]');
  const nativeInput = makeElement('input');
  rowAttention.appendChild(nativeInput);
  harness.panelEl._fire('keydown', { target: nativeButton, key: 'Enter', preventDefault() {
    throw new Error('button behavior stays native');
  } });
  harness.panelEl._fire('keydown', { target: nativeInput, key: ' ', preventDefault() {
    throw new Error('input behavior stays native');
  } });

  click(harness.panelEl, harness.panelEl.querySelector('[data-transfer-view="history"]'));
  const failedRow = harness.panelEl.querySelector('tr[data-job-id="failed"]');
  harness.panelEl._fire('keydown', { target: failedRow, key: 'Enter', preventDefault() {} });
  assert.strictEqual(harness.dialogs.at(-1).options.title, 'Transfer details');

  assert.strictEqual(harness.panelEl._listenerCount('keydown'), 1);
  harness.controller.destroy();
  assert.strictEqual(harness.panelEl._listenerCount('click'), 0);
  assert.strictEqual(harness.panelEl._listenerCount('keydown'), 0,
    'destroy removes both delegated handlers');
  assert.strictEqual(harness.subscriptionCount(), 0,
    'destroy also releases the one runtime subscription');
}

// A focused row wins over stale selection for every keyboard action. This
// covers tab/programmatic focus movement that does not first emit Arrow keys.
{
  const harness = loadHarness();
  harness.emit(snapshot([
    job('selected', 'running'),
    job('focused', 'paused', { queueOrder: 2 }),
    job('attention-focus', 'needsAttention', {
      queueOrder: 3,
      state: { kind: 'needsAttention', reason: { kind: 'missingPartial' } },
    }),
  ], { summary: summary({ active: 3, running: 1, paused: 1, attention: 1 }) }));
  const focused = harness.panelEl.querySelector('tr[data-job-id="focused"]');
  focused.focus();
  harness.panelEl._fire('keydown', { target: focused, key: ' ', preventDefault() {} });
  assert.deepStrictEqual(harness.runtimeCalls, [{ method: 'resume', args: ['focused'] }]);
  assert.strictEqual(harness.controller.getState().selectedId, 'focused');

  const attention = harness.panelEl.querySelector('tr[data-job-id="attention-focus"]');
  attention.focus();
  harness.panelEl._fire('keydown', { target: attention, key: 'Enter', preventDefault() {} });
  assert.strictEqual(harness.dialogs.at(-1).options.title, 'Partial file missing');
  assert.strictEqual(harness.controller.getState().selectedId, 'attention-focus');

  harness.panelEl._fire('keydown', { target: attention, key: 'Delete', preventDefault() {} });
  assert.ok(harness.dialogs.at(-1).bodyEl.textContent.includes('attention-focus.bin'));
}

// Active is the default projection, driven only by job.state.kind. The table
// keeps stable dense columns, safe text, one selected row, and textual status
// alongside the native progress value.
{
  const harness = loadHarness();
  const malicious = job('running', 'running', {
    fileName: '<img src=x onerror=alert(1)>',
    direction: 'download',
    localPath: '/Users/me/report.txt',
    remotePath: '/srv/report.txt',
    endpoint: { kind: 'adHoc', host: 'files.example', port: 2222, user: 'sam' },
  });
  const jobs = [
    malicious,
    job('queued', 'queued', { queueOrder: 2 }),
    job('attention', 'needsAttention', { queueOrder: 3, state: { kind: 'needsAttention', reason: { kind: 'destinationConflict', resumeAvailable: true } } }),
    job('completed', 'completed', { queueOrder: 4, state: { kind: 'completed', result: 'transferred' } }),
    job('failed', 'failed', { queueOrder: 5, state: { kind: 'failed', error: 'Permission denied' } }),
    job('cancelled', 'cancelled', { queueOrder: 6, state: { kind: 'cancelled', cleanupError: null } }),
  ];
  harness.emit(snapshot(jobs, {
    summary: summary({ active: 3, history: 3, running: 1, queued: 1, attention: 1, failed: 1 }),
  }));

  assert.deepStrictEqual(
    harness.panelEl.querySelectorAll('th').map((cell) => cell.textContent),
    ['File / Direction', 'Host', 'Destination', 'Status / Progress', 'Speed / ETA', 'Actions'],
  );
  assert.deepStrictEqual(rowIds(harness.panelEl), ['running', 'queued', 'attention']);
  assert.strictEqual(harness.panelEl.querySelectorAll('tr[aria-selected="true"]').length, 1);
  assert.strictEqual(harness.panelEl.querySelector('tr[aria-selected="true"]').dataset.jobId, 'running');
  assert.strictEqual(harness.panelEl.querySelector('img'), null,
    'untrusted names are text, never parsed markup');
  assert.ok(harness.panelEl.textContent.includes('<img src=x onerror=alert(1)>'));
  assert.ok(harness.panelEl.textContent.includes('Download'));
  assert.ok(harness.panelEl.textContent.includes('sam@files.example:2222'));
  assert.ok(harness.panelEl.textContent.includes('/Users/me/report.txt'));
  assert.ok(harness.panelEl.textContent.includes('Running — 25%'));
  assert.ok(harness.panelEl.textContent.includes('Needs attention'));
  const progress = harness.panelEl.querySelector('progress');
  assert.strictEqual(progress.value, 25);
  assert.strictEqual(progress.max, 100);
  assert.strictEqual(
    progress.getAttribute('aria-label'),
    '<img src=x onerror=alert(1)> progress: 25%',
    'progress uses the visible file label, never an opaque backend id',
  );
  assert.ok(harness.panelEl.querySelector('[aria-live="polite"]').textContent.includes('3 active'));
  assert.strictEqual(harness.shellSentinel.parentNode, harness.sandbox.document.body,
    'runtime updates stay inside the panel surface and do not replace shell chrome');
}

// Selection and rendering share the view projection contract. Instrumenting
// that public contract must observe controller reconciliation as well as view
// rendering; a second terminal-state classifier in the panel would drift.
{
  let projectionCalls = 0;
  const harness = loadHarness({
    configureView(view) {
      const jobsFor = view.jobsFor;
      view.jobsFor = (...args) => {
        projectionCalls += 1;
        return jobsFor(...args);
      };
    },
  });
  harness.emit(snapshot([job('active', 'running')]));
  assert.ok(projectionCalls > 0, 'controller reconciliation delegates membership to the view');
  assert.strictEqual(harness.controller.getState().selectedId, 'active');
}

// History is a delegated view-state change. Its membership is terminal-only,
// and selection follows the visible rows without inventing lifecycle state.
{
  const harness = loadHarness();
  harness.emit(snapshot([
    job('running', 'running'),
    job('completed', 'completed', { state: { kind: 'completed', result: 'transferred' } }),
    job('failed', 'failed', { state: { kind: 'failed', error: 'Disk full' } }),
    job('cancelled', 'cancelled', { state: { kind: 'cancelled', cleanupError: 'Partial remained' } }),
  ], { summary: summary({ active: 1, history: 3, running: 1, failed: 1 }) }));

  click(harness.panelEl, harness.panelEl.querySelector('[data-transfer-view="history"]'));
  assert.deepStrictEqual(rowIds(harness.panelEl), ['completed', 'failed', 'cancelled']);
  assert.strictEqual(harness.panelEl.querySelector('tr[aria-selected="true"]').dataset.jobId, 'completed');
  assert.ok(harness.panelEl.textContent.includes('Completed'));
  assert.ok(harness.panelEl.textContent.includes('Failed — Disk full'));
  assert.ok(harness.panelEl.textContent.includes('Cancelled — Partial remained'));
  assert.deepStrictEqual(harness.runtimeCalls, []);
}

// Loading, recovery failure, and both empty projections are distinct states.
{
  const harness = loadHarness();
  harness.emit(snapshot([], { recoveryError: 'Recovered queue was quarantined', summary: summary() }));
  const recovery = harness.panelEl.querySelector('[data-transfer-state="recovery-error"]');
  assert.strictEqual(recovery.getAttribute('role'), 'alert');
  assert.ok(recovery.textContent.includes('Recovered queue was quarantined'));
  assert.strictEqual(harness.panelEl.querySelector('[data-transfer-state="empty"]').textContent, 'No active transfers');

  click(harness.panelEl, harness.panelEl.querySelector('[data-transfer-view="history"]'));
  assert.strictEqual(harness.panelEl.querySelector('[data-transfer-state="empty"]').textContent, 'No transfer history');
}

// Progress/summary updates patch the existing row and table body. A true
// membership change rebuilds rows, which is the only reason stable row keys
// should be discarded.
{
  const harness = loadHarness();
  const first = snapshot([job('a', 'running')], {
    revision: 1,
    summary: summary({ active: 1, running: 1 }),
  });
  harness.emit(first);
  const tbody = harness.panelEl.querySelector('tbody');
  const row = harness.panelEl.querySelector('tr[data-job-id="a"]');

  harness.emit(snapshot([job('a', 'running', {
    bytesTransferred: 75,
    speedBytesPerSecond: 2048,
    etaSeconds: 3,
  })], {
    revision: 2,
    summary: summary({ active: 1, running: 1 }),
  }));
  assert.strictEqual(harness.panelEl.querySelector('tbody'), tbody);
  assert.strictEqual(harness.panelEl.querySelector('tr[data-job-id="a"]'), row,
    'progress ticks patch the keyed row instead of replacing it');
  assert.ok(row.textContent.includes('Running — 75%'));
  assert.ok(row.textContent.includes('2048 B/s'));
  assert.ok(row.textContent.includes('3s remaining'));

  harness.emit(snapshot([
    job('a', 'running', { bytesTransferred: 75 }),
    job('b', 'queued', { queueOrder: 2 }),
  ], {
    revision: 3,
    summary: summary({ active: 2, running: 1, queued: 1 }),
  }));
  assert.notStrictEqual(harness.panelEl.querySelector('tr[data-job-id="a"]'), row,
    'membership changes rebuild the table rows');
}

// The polite region announces aggregate state, not byte ticks. Repeated
// progress snapshots with identical counts must perform zero text writes;
// one backend summary change earns exactly one new announcement.
{
  const harness = loadHarness();
  const aggregate = summary({ active: 1, running: 1 });
  harness.emit(snapshot([job('a', 'running')], { revision: 1, summary: aggregate }));
  const liveRegion = harness.panelEl.querySelector('[aria-live="polite"]');
  const writesAfterInitialSummary = liveRegion._textContentWriteCount;

  for (let revision = 2; revision <= 12; revision += 1) {
    harness.emit(snapshot([job('a', 'running', {
      bytesTransferred: revision * 5,
      speedBytesPerSecond: revision * 1024,
      etaSeconds: 20 - revision,
    })], { revision, summary: aggregate }));
  }
  assert.strictEqual(liveRegion._textContentWriteCount, writesAfterInitialSummary,
    'progress-only snapshots must not churn the polite live region');

  harness.emit(snapshot([
    job('a', 'running', { bytesTransferred: 60 }),
    job('b', 'queued', { queueOrder: 2 }),
  ], {
    revision: 13,
    summary: summary({ active: 2, running: 1, queued: 1 }),
  }));
  assert.strictEqual(liveRegion._textContentWriteCount, writesAfterInitialSummary + 1,
    'one aggregate count change produces one polite announcement');
}

// --- Toolbar actions are compact icon buttons with tooltips ----------------
// The queue/clear/concurrency controls render as tl-icon-btn icon buttons
// (matching the tool-window header chrome) with `title` tooltips that mirror
// their aria-labels — not wide text buttons. The queue toggle's icon, tooltip,
// and action all flip together with the paused state.
{
  const harness = loadHarness();
  harness.emit(snapshot([job('done', 'completed')], { summary: summary({ history: 1 }) }));

  const buttons = harness.panelEl.querySelectorAll('[data-transfer-action]')
    .filter((el) => ['pause-all', 'resume-all', 'clear-completed', 'concurrency'].includes(el.dataset.transferAction));
  assert.strictEqual(buttons.length, 3, 'three toolbar action buttons');
  for (const button of buttons) {
    assert.ok(button.className.includes('tl-icon-btn'),
      `${button.dataset.transferAction} must be a compact icon button`);
    assert.ok(!button.className.split(' ').includes('tl-btn'),
      `${button.dataset.transferAction} must not keep the wide text-button class`);
    assert.ok(button.getAttribute('title'), `${button.dataset.transferAction} must carry a tooltip`);
    assert.strictEqual(button.getAttribute('title'), button.getAttribute('aria-label'),
      `${button.dataset.transferAction} tooltip must mirror its aria-label`);
  }

  const queueButton = harness.panelEl.querySelector('[data-transfer-action="pause-all"]');
  assert.strictEqual(queueButton.getAttribute('title'), 'Pause all active transfers');

  harness.emit(snapshot([job('done', 'completed')], { queuePaused: true, summary: summary({ history: 1, queuePaused: true }) }));
  const resumeButton = harness.panelEl.querySelector('[data-transfer-action="resume-all"]');
  assert.ok(resumeButton, 'paused queue flips the action to resume-all');
  assert.strictEqual(resumeButton.getAttribute('title'), 'Resume all eligible transfers');
}

// --- Row tooltip carries the transfer paths --------------------------------
// The narrow card layout hides the destination cell, so the full
// source -> destination route must be reachable as a hover tooltip on the row
// itself (harmless in the wide table, essential in cards).
{
  const harness = loadHarness();
  harness.emit(snapshot([job('tip', 'running')]));
  const row = harness.panelEl.querySelector('tr[data-job-id="tip"]');
  const tooltip = row.getAttribute('title') || '';
  assert.ok(tooltip.includes('/local/tip.bin'), 'row tooltip must include the source path');
  assert.ok(tooltip.includes('/remote/tip.bin'), 'row tooltip must include the destination path');
  assert.ok(tooltip.indexOf('/local/tip.bin') < tooltip.indexOf('/remote/tip.bin'),
    'tooltip reads source before destination for an upload');
}

// --- Batch grouping: header row aggregates, groups members, leaves
// batchless rows untouched, and routes its own cancel through a dedicated
// confirm dialog naming the batch. ---
{
  const harness = loadHarness();
  const runningBatch = batchAgg('batch-1', {
    info: {
      name: 'photos', direction: 'upload', expansion: { kind: 'running' },
      discoveredFiles: 5, discoveredBytes: 500, createdAtMs: 1,
    },
    filesDone: 3, bytesDone: 300, speedBytesPerSecond: 2048, etaSeconds: 9,
  });
  const memberA = job('m1', 'running', { batchId: 'batch-1', fileName: 'a.jpg' });
  const memberB = job('m2', 'queued', { batchId: 'batch-1', fileName: 'b.jpg', queueOrder: 2 });
  const loner = job('solo', 'running', { fileName: 'solo.bin' });
  harness.emit(snapshot([memberA, memberB, loner], {
    batches: [runningBatch],
    summary: summary({ active: 3, running: 2, queued: 1 }),
  }));

  const headerRow = harness.panelEl.querySelector('tr.tl-transfer-center__batch[data-batch-id="batch-1"]');
  assert.ok(headerRow, 'batch header row renders with data-batch-id');
  assert.ok(headerRow.textContent.includes('photos'));
  assert.ok(headerRow.textContent.includes('Upload'));
  assert.ok(headerRow.textContent.includes('3/5+'), 'running expansion appends + to the files-done ratio');
  assert.ok(headerRow.textContent.includes('300 B of 500 B'));
  assert.ok(headerRow.textContent.includes('2048 B/s'));
  assert.ok(headerRow.textContent.includes('9s remaining'));
  assert.strictEqual(
    headerRow.querySelector('[data-transfer-action="cancel-batch"]').dataset.batchId,
    'batch-1',
  );

  // The <td> must stay a plain table cell — no flex display of its own —
  // or `colspan` collapses under `table-layout: fixed`. The flex row lives
  // on an inner wrapper div instead.
  const headerCell = headerRow.querySelector('td.tl-transfer-center__batch-cell');
  assert.ok(headerCell, 'header renders a normal table cell');
  assert.strictEqual(headerCell.className, 'tl-transfer-center__batch-cell',
    'the table cell itself carries no flex-layout class');
  assert.strictEqual(headerCell.children.length, 1, 'the cell wraps its content in exactly one child');
  const headerLine = headerCell.children[0];
  assert.strictEqual(headerLine.className, 'tl-transfer-center__batch-line',
    'the flex layout lives on the inner wrapper, not the table cell');
  assert.ok(headerLine.querySelector('.tl-transfer-center__batch-name'),
    'header content renders inside the wrapper line');

  assert.deepStrictEqual(rowIds(harness.panelEl), ['m1', 'm2', 'solo'],
    'member rows render beneath their header; batchless rows are unaffected');
  const rowsInOrder = harness.panelEl.querySelector('tbody').children;
  assert.strictEqual(rowsInOrder[0], headerRow, 'the header renders before its member rows');
  assert.strictEqual(rowsInOrder[1].dataset.jobId, 'm1');
  assert.strictEqual(rowsInOrder[2].dataset.jobId, 'm2');
  assert.strictEqual(rowsInOrder[3].dataset.jobId, 'solo');

  // Cancel-batch dialog counts remaining files (discoveredFiles - filesDone),
  // not the batch total — 3 of 5 already completed, so only 2 remain to
  // cancel. While expansion is still running, more files may yet be
  // discovered, so the remaining count is marked "+".
  click(harness.panelEl, headerRow.querySelector('[data-transfer-action="cancel-batch"]'));
  const runningDialog = harness.dialogs.at(-1);
  assert.ok(runningDialog.bodyEl.textContent.includes('2+ files remaining'),
    'remaining (not total) count, marked + while expansion keeps discovering files');
  assert.ok(!runningDialog.bodyEl.textContent.includes('5 files'),
    'dialog must not use the batch total as the cancelled count');
  await dialogButton(runningDialog, 'Keep transfer').onSelect();
  assert.deepStrictEqual(harness.runtimeCalls, [], 'Keep transfer must not cancel anything');

  // Interrupted expansion shows the warning marker with the reason as title,
  // and drops the running "+" suffix.
  harness.emit(snapshot([memberA, memberB, loner], {
    revision: 2,
    batches: [batchAgg('batch-1', {
      info: {
        name: 'photos', direction: 'upload',
        expansion: { kind: 'interrupted', reason: 'permission denied' },
        discoveredFiles: 5, discoveredBytes: 500, createdAtMs: 1,
      },
      filesDone: 3, bytesDone: 300, speedBytesPerSecond: null, etaSeconds: null,
    })],
    summary: summary({ active: 3, running: 1, queued: 1 }),
  }));
  const interruptedHeader = harness.panelEl.querySelector('tr.tl-transfer-center__batch[data-batch-id="batch-1"]');
  const marker = interruptedHeader.querySelector('.tl-transfer-center__batch-marker');
  assert.ok(marker, 'interrupted expansion shows a marker');
  assert.strictEqual(marker.textContent, '⚠ expansion interrupted');
  assert.strictEqual(marker.getAttribute('title'), 'permission denied');
  assert.ok(!interruptedHeader.textContent.includes('3/5+'), 'interrupted expansion drops the running + suffix');
  assert.ok(interruptedHeader.textContent.includes('3/5'));
  assert.ok(interruptedHeader.textContent.includes('—'), 'null speed renders the em dash placeholder');

  // Cancel-batch routes through its own confirm dialog naming the batch and
  // its member count, and only calls the runtime after confirmation.
  const cancelButton = interruptedHeader.querySelector('[data-transfer-action="cancel-batch"]');
  click(harness.panelEl, cancelButton);
  assert.deepStrictEqual(harness.runtimeCalls, [], 'cancel-batch waits for confirmation');
  const batchDialog = harness.dialogs.at(-1);
  assert.strictEqual(batchDialog.options.title, 'Cancel folder transfer?');
  assert.ok(batchDialog.bodyEl.textContent.includes('photos'));
  assert.ok(batchDialog.bodyEl.textContent.includes('2 files remaining'),
    'dialog names the remaining (not total) count once expansion is no longer running');
  assert.ok(!batchDialog.bodyEl.textContent.includes('5 files'),
    'dialog must not use the batch total as the cancelled count');
  await dialogButton(batchDialog, 'Cancel transfer').onSelect();
  assert.deepStrictEqual(harness.runtimeCalls, [{ method: 'cancelBatch', args: ['batch-1'] }]);

  // Selecting/keying through the table walks member rows only; the header is
  // never a keyboard stop.
  assert.deepStrictEqual(harness.selectionEvents, []);
  const memberRow = harness.panelEl.querySelector('tr[data-job-id="m1"]');
  let prevented = 0;
  harness.panelEl._fire('keydown', {
    target: memberRow, key: 'ArrowUp', preventDefault: () => { prevented += 1; },
  });
  assert.deepStrictEqual(harness.selectionEvents, ['m1'],
    'ArrowUp from the first member clamps to itself, never the header');
  assert.strictEqual(prevented, 1);
}

// --- Grouping interleaves by first appearance: a batchless job queued
// BEFORE a folder transfer renders above it, and the batch's members stay
// contiguous under their header wherever that header lands. ---
{
  const harness = loadHarness();
  const early = job('early', 'running', { fileName: 'early.bin', queueOrder: 1 });
  const memberA = job('m1', 'queued', { batchId: 'batch-9', fileName: 'a.jpg', queueOrder: 2 });
  const late = job('late', 'queued', { fileName: 'late.bin', queueOrder: 3 });
  const memberB = job('m2', 'queued', { batchId: 'batch-9', fileName: 'b.jpg', queueOrder: 4 });
  harness.emit(snapshot([early, memberA, late, memberB], {
    batches: [batchAgg('batch-9', { info: { name: 'photos', createdAtMs: 9 } })],
    summary: summary({ active: 4, running: 1, queued: 3 }),
  }));

  const rows = harness.panelEl.querySelector('tbody').children;
  assert.strictEqual(rows[0].dataset.jobId, 'early',
    'a batchless job queued before the folder transfer must render above its header');
  assert.strictEqual(rows[1].dataset.batchId, 'batch-9',
    'the header lands where the batch first appears in the queue');
  assert.strictEqual(rows[2].dataset.jobId, 'm1');
  assert.strictEqual(rows[3].dataset.jobId, 'm2',
    'every member of a batch stays contiguous under its header');
  assert.strictEqual(rows[4].dataset.jobId, 'late');
  assert.deepStrictEqual(rowIds(harness.panelEl), ['early', 'm1', 'm2', 'late']);
}

// --- Two batches each keep their own header at their own first appearance,
// with a batchless row between them. ---
{
  const harness = loadHarness();
  harness.emit(snapshot([
    job('p1', 'running', { batchId: 'batch-a', queueOrder: 1 }),
    job('mid', 'queued', { queueOrder: 2 }),
    job('q1', 'queued', { batchId: 'batch-b', queueOrder: 3 }),
  ], {
    // Delivered aggregate order is deliberately the reverse of the job
    // order: rendering follows the jobs, not the projection's ordering.
    batches: [
      batchAgg('batch-b', { info: { name: 'second', createdAtMs: 2 } }),
      batchAgg('batch-a', { info: { name: 'first', createdAtMs: 1 } }),
    ],
    summary: summary({ active: 3, running: 1, queued: 2 }),
  }));

  const rows = harness.panelEl.querySelector('tbody').children;
  assert.deepStrictEqual(
    rows.map((row) => row.dataset.batchId || row.dataset.jobId),
    ['batch-a', 'p1', 'mid', 'batch-b', 'q1'],
  );
}

// Grouping is patch-friendly: a progress-only update to a batch member keeps
// the same header and row DOM nodes; a true membership change (a batch
// clearing entirely, e.g. from history clearing) drops its header exactly
// like a batchless row's membership change.
{
  const harness = loadHarness();
  const staticBatch = batchAgg('batch-2', {
    info: {
      name: 'backup', direction: 'download', expansion: { kind: 'running' },
      discoveredFiles: 2, discoveredBytes: 200, createdAtMs: 5,
    },
    filesDone: 0, bytesDone: 0, speedBytesPerSecond: 512, etaSeconds: 10,
  });
  const memberB = job('x2', 'running', { batchId: 'batch-2', direction: 'download', queueOrder: 2 });
  harness.emit(snapshot([
    job('x1', 'running', { batchId: 'batch-2', direction: 'download' }),
    memberB,
  ], { batches: [staticBatch], summary: summary({ active: 2, running: 2 }) }));

  const tbody = harness.panelEl.querySelector('tbody');
  const headerBefore = harness.panelEl.querySelector('tr.tl-transfer-center__batch[data-batch-id="batch-2"]');
  const rowBefore = harness.panelEl.querySelector('tr[data-job-id="x1"]');

  // Progress-only tick: same member set, same batch id -> patch in place.
  harness.emit(snapshot([
    job('x1', 'running', { batchId: 'batch-2', direction: 'download', bytesTransferred: 50 }),
    memberB,
  ], {
    revision: 2,
    batches: [batchAgg('batch-2', {
      info: {
        name: 'backup', direction: 'download', expansion: { kind: 'running' },
        discoveredFiles: 2, discoveredBytes: 200, createdAtMs: 5,
      },
      filesDone: 0, bytesDone: 50, speedBytesPerSecond: 1024, etaSeconds: 5,
    })],
    summary: summary({ active: 2, running: 2 }),
  }));
  assert.strictEqual(harness.panelEl.querySelector('tbody'), tbody);
  assert.strictEqual(
    harness.panelEl.querySelector('tr.tl-transfer-center__batch[data-batch-id="batch-2"]'), headerBefore,
    'a progress-only tick patches the existing header row instead of replacing it',
  );
  assert.strictEqual(harness.panelEl.querySelector('tr[data-job-id="x1"]'), rowBefore,
    'member rows keep their identity across progress-only ticks');
  assert.ok(headerBefore.textContent.includes('50 B of 200 B'));

  // The batch clears entirely once every member leaves this view (history
  // clearing or batch completion) — its header disappears with them.
  harness.emit(snapshot([], { revision: 3, batches: [], summary: summary() }));
  assert.strictEqual(
    harness.panelEl.querySelector('tr.tl-transfer-center__batch[data-batch-id="batch-2"]'), null,
    'a batch with no remaining members in view renders no header',
  );
  assert.strictEqual(harness.panelEl.querySelector('tr[data-job-id="x1"]'), null);
}

// --- Cancel All lives in the toolbar, disables with nothing active, and
// routes through its own confirm dialog before touching the runtime. ---
{
  const harness = loadHarness();
  harness.emit(snapshot([], { summary: summary() }));
  const cancelAllButton = harness.panelEl.querySelector('[data-transfer-action="cancel-all"]');
  assert.ok(cancelAllButton, 'toolbar exposes a cancel-all action');
  assert.strictEqual(cancelAllButton.disabled, true, 'nothing to cancel disables the button');
  click(harness.panelEl, cancelAllButton);
  assert.deepStrictEqual(harness.dialogs, [], 'a disabled cancel-all opens no dialog');

  harness.emit(snapshot([job('a', 'running'), job('b', 'queued')], {
    revision: 2,
    summary: summary({ active: 2, running: 1, queued: 1 }),
  }));
  assert.strictEqual(cancelAllButton.disabled, false);

  click(harness.panelEl, cancelAllButton);
  assert.deepStrictEqual(harness.runtimeCalls, [], 'cancel-all waits for confirmation');
  const firstDialog = harness.dialogs.at(-1);
  assert.strictEqual(firstDialog.options.title, 'Cancel all transfers?');
  assert.ok(firstDialog.bodyEl.textContent.includes('2 transfers will be cancelled'));
  await dialogButton(firstDialog, 'Keep transfers').onSelect();
  assert.deepStrictEqual(harness.runtimeCalls, [], 'Keep transfers must not cancel anything');

  click(harness.panelEl, cancelAllButton);
  await dialogButton(harness.dialogs.at(-1), 'Cancel all').onSelect();
  assert.deepStrictEqual(harness.runtimeCalls, [{ method: 'cancelAll', args: [] }]);
}

// --- A huge batch renders only its leading members plus one "N more" row,
// and a later discovery tick patches that row's count without rebuilding
// the table. The full queue stays in the backend document; the view is a
// bounded window over it. ---
{
  const harness = loadHarness();
  const members = (count) => Array.from({ length: count }, (_, index) => (
    job(`m${index}`, 'queued', { batchId: 'batch-1' })
  ));
  harness.emit(snapshot(members(120), {
    batches: [batchAgg('batch-1', { info: { discoveredFiles: 120 } })],
    summary: summary({ active: 120, queued: 120 }),
  }));

  assert.strictEqual(rowIds(harness.panelEl).length, 100,
    'member rows are capped per batch');
  const moreRow = harness.panelEl.querySelector('tr.tl-transfer-center__more');
  assert.ok(moreRow, 'the remainder collapses into one row');
  assert.ok(moreRow.textContent.includes('+ 20 more transfers not shown'), moreRow.textContent);

  const tbody = harness.panelEl.querySelector('tbody');
  harness.emit(snapshot(members(150), {
    revision: 2,
    batches: [batchAgg('batch-1', { info: { discoveredFiles: 150 } })],
    summary: summary({ active: 150, queued: 150 }),
  }));
  assert.strictEqual(harness.panelEl.querySelector('tbody'), tbody);
  assert.strictEqual(harness.panelEl.querySelector('tr.tl-transfer-center__more'), moreRow,
    'a growing remainder patches the existing row instead of rebuilding the table');
  assert.ok(moreRow.textContent.includes('+ 50 more transfers not shown'), moreRow.textContent);
}

// --- The batch header totals the folder as a whole: bytes done, total, and
// a FileZilla-style remaining figure, marked "+" while discovery continues
// (the true total is still a moving target), unmarked once the walk ends. ---
{
  const harness = loadHarness();
  const member = job('m1', 'running', { batchId: 'batch-r' });
  harness.emit(snapshot([member], {
    batches: [batchAgg('batch-r', {
      info: { discoveredBytes: 500 },
      bytesDone: 120,
    })],
    summary: summary({ active: 1, running: 1 }),
  }));
  const header = harness.panelEl.querySelector('tr.tl-transfer-center__batch[data-batch-id="batch-r"]');
  assert.ok(header.textContent.includes('120 B of 500 B'), header.textContent);
  assert.ok(header.textContent.includes('380 B+ left'), header.textContent);

  harness.emit(snapshot([member], {
    revision: 2,
    batches: [batchAgg('batch-r', {
      info: { discoveredBytes: 500, expansion: { kind: 'complete' } },
      bytesDone: 200,
    })],
    summary: summary({ active: 1, running: 1 }),
  }));
  const settled = harness.panelEl.querySelector('tr.tl-transfer-center__batch[data-batch-id="batch-r"]');
  assert.ok(settled.textContent.includes('300 B left'), settled.textContent);
  assert.ok(!settled.textContent.includes('300 B+ left'),
    'the moving-target marker drops once discovery is finished');
}

// --- Batchless queues are also capped, with the tail collapsed. ---
{
  const harness = loadHarness();
  const jobs = Array.from({ length: 510 }, (_, index) => job(`j${index}`, 'queued'));
  harness.emit(snapshot(jobs, { summary: summary({ active: 510, queued: 510 }) }));
  assert.strictEqual(rowIds(harness.panelEl).length, 500, 'total rendered rows are capped');
  const moreRow = harness.panelEl.querySelector('tr.tl-transfer-center__more');
  assert.ok(moreRow.textContent.includes('+ 10 more transfers not shown'), moreRow.textContent);
}

console.log('transfer center: all assertions passed');
