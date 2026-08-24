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
const VIEW_PATH = path.join(FRONTEND, 'app/features/transfers/view.js');
const PANEL_PATH = path.join(FRONTEND, 'app/panels/transfer-center.js');

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
    removeChild(child) {
      const index = this.children.indexOf(child);
      if (index >= 0) this.children.splice(index, 1);
      child.parentNode = null;
      return child;
    },
    replaceChildren(...children) {
      for (const child of this.children) child.parentNode = null;
      this.children = [];
      ownText = '';
      children.forEach((child) => this.appendChild(child));
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
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
    summary: summary(),
    recoveryError: null,
    ...values,
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
  let subscriptionCount = 0;
  const runtimeCalls = [];
  const actionEvents = [];
  const selectionEvents = [];
  const runtime = {
    subscribe(handler) {
      subscriptionCount += 1;
      subscriber = handler;
      return () => { subscriber = null; };
    },
  };
  for (const method of ['pause', 'resume', 'cancel', 'retry', 'resolve', 'pauseAll', 'resumeAll', 'clearCompleted']) {
    runtime[method] = (...args) => {
      runtimeCalls.push({ method, args });
      return Promise.resolve();
    };
  }

  const sandbox = {
    console,
    Promise,
    Number,
    Math,
    Set,
    Map,
    document: {
      body,
      createElement: (tag) => makeElement(tag),
      getElementById: (id) => body.querySelector(`#${id}`),
    },
    utils: {
      formatSize(value) { return `${Number(value)} B`; },
    },
    termlabTransferRuntime: runtime,
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  vm.createContext(sandbox);

  assert.ok(fs.existsSync(VIEW_PATH), 'Transfer Center view IIFE must exist');
  assert.ok(fs.existsSync(PANEL_PATH), 'Transfer Center panel IIFE must exist');
  vm.runInContext(fs.readFileSync(VIEW_PATH, 'utf8'), sandbox, { filename: VIEW_PATH });
  vm.runInContext(fs.readFileSync(PANEL_PATH, 'utf8'), sandbox, { filename: PANEL_PATH });
  sandbox.transferCenterPanel.init({
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
    actionEvents,
    selectionEvents,
    emit(value) {
      assert.ok(subscriber, 'controller must subscribe to the shared transfer runtime');
      subscriber(value);
    },
    subscriptionCount: () => subscriptionCount,
  };
}

function rowIds(panelEl) {
  return panelEl.querySelectorAll('tr[data-job-id]').map((row) => row.dataset.jobId);
}

function click(panelEl, target) {
  panelEl._fire('click', { target });
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

// Row actions and selection are exposed through the one delegated surface so
// Task 12 can bind dialogs/commands without replacing this renderer. They do
// not call the runtime by themselves in this read-only slice.
{
  const harness = loadHarness();
  harness.emit(snapshot([job('a', 'running')], { summary: summary({ active: 1, running: 1 }) }));
  const details = harness.panelEl.querySelector('[data-transfer-action="details"]');
  click(harness.panelEl, details);
  assert.deepStrictEqual(harness.selectionEvents, ['a']);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(harness.actionEvents)), [{ action: 'details', jobId: 'a' }]);
  assert.deepStrictEqual(harness.runtimeCalls, []);
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
  assert.strictEqual(progress.getAttribute('aria-label'), 'running progress: 25%');
  assert.ok(harness.panelEl.querySelector('[aria-live="polite"]').textContent.includes('3 active'));
  assert.strictEqual(harness.shellSentinel.parentNode, harness.sandbox.document.body,
    'runtime updates stay inside the panel surface and do not replace shell chrome');
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

console.log('transfer center: all assertions passed');
