// Run: node scripts/tests/test_files_dnd.mjs
//
// Task 7: drag-and-drop between panes.
//
// Two harnesses:
//
//   Part A — pane-view.js DOM harness. Extends the recording-element
//   pattern test_pane_toolbar_layout.mjs established (a fake `el`/`document`
//   good enough for renderPane's post-render querySelector calls) with a
//   fake `dataTransfer` ({ types, data: Map, setData, getData }) and manual
//   drag-event dispatch, so it can drive dragstart/dragover/dragleave/drop
//   without a real DOM.
//
//   Part B — files-panel.js logic harness, the same one test_sftp_connect.mjs
//   established: load the REAL files-panel.js, stub termlabFilesPaneView
//   .renderPane as a spy that records (pane, el, deps), then call the
//   captured deps.onDropEntries directly. Since deps is the actual closure
//   files-panel.js wires up, this exercises the real onDropEntries routing
//   (transferRecursive / transferUpload / transferDownload, the
//   not-connected guard, error toasts) against the real invoke() calls —
//   full wire-level coverage, not just the data-service seam.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..');
const FRONTEND = path.join(repoRoot, 'crates/termlab_tauri/frontend/app');
const PANE_VIEW_PATH = path.join(FRONTEND, 'features/files/pane-view.js');
const BREADCRUMBS_PATH = path.join(FRONTEND, 'features/files/breadcrumbs.js');
const FILES_PANEL_PATH = path.join(FRONTEND, 'panels/files-panel.js');
const FILES_DATA_SERVICE_PATH = path.join(FRONTEND, 'features/files/data-service.js');
const FILES_PANE_STORE_PATH = path.join(FRONTEND, 'features/files/pane-store.js');
const FILES_ACTIONS_PATH = path.join(FRONTEND, 'features/files/actions.js');
const NATIVE_DROP_PATH = path.join(FRONTEND, 'features/files/native-drop.js');
const DRAGDROP_RUNTIME_PATH = path.join(FRONTEND, 'dragdrop-runtime.js');

const ENTRY_MIME = 'application/x-termlab-entry';

// Objects built inside a vm context have that context's own Object.prototype
// — structurally identical to an outer-realm object but not
// reference-equal, which trips assert.deepEqual's strict prototype check.
// Round-tripping through JSON strips that (same idiom test_files_transfers.mjs
// uses for its captured invoke() call args).
const j = (v) => JSON.parse(JSON.stringify(v));

// ===========================================================================
// Part A — pane-view.js: dragstart / dragover / dragleave / drop
// ===========================================================================

function loadPaneView() {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(BREADCRUMBS_PATH, 'utf8'), sandbox, { filename: BREADCRUMBS_PATH });
  vm.runInContext(fs.readFileSync(PANE_VIEW_PATH, 'utf8'), sandbox, { filename: PANE_VIEW_PATH });
  assert.ok(sandbox.termlabFilesPaneView, 'pane-view IIFE must expose window.termlabFilesPaneView');
  sandbox.document = { createElement: (tag) => fakeElement(tag) };
  return sandbox.termlabFilesPaneView;
}

// A minimal element good enough for renderPane's row/table-wrap wiring:
// classList, addEventListener/dispatchEvent, and a querySelector callers can
// override to hand back fixed children (tbody, .fp-table-wrap) — same idiom
// test_sftp_connect.mjs's DOM harness and test_files_transfers.mjs's
// eventElement() use for this exact module.
function fakeElement(tag) {
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    className: '',
    dataset: {},
    style: {},
    textContent: '',
    title: '',
    children: [],
    innerHTML: '',
    classList: {
      _set: new Set(),
      add(...names) { names.forEach((n) => this._set.add(n)); },
      remove(...names) { names.forEach((n) => this._set.delete(n)); },
      contains(n) { return this._set.has(n); },
    },
    appendChild(child) { this.children.push(child); child.parentNode = el; return child; },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    dispatchEvent(evt) {
      for (const fn of (listeners.get(evt.type) || []).slice()) fn(evt);
      return true;
    },
    setAttribute() {},
    getAttribute() { return null; },
    querySelector() { return null; },
    querySelectorAll() { return []; },
  };
  return el;
}

function dragEvent(type, dataTransfer) {
  return {
    type,
    dataTransfer,
    defaultPrevented: false,
    preventDefault() { this.defaultPrevented = true; },
  };
}

// A fake DataTransfer good enough for setData/getData/types — the surface
// pane-view.js's dnd helpers use. `types` mutates on setData exactly like
// the real DataTransfer does. `getDataCallCount` lets tests prove dragover
// never calls getData (finding 2: some engines restrict it mid-drag, so
// the accept decision must be derivable from `types` alone).
function fakeDataTransfer(initialTypes = []) {
  const data = new Map();
  const dt = {
    types: initialTypes.slice(),
    data,
    getDataCallCount: 0,
    setData(type, value) {
      data.set(type, value);
      if (this.types.indexOf(type) === -1) this.types.push(type);
    },
    getData(type) {
      dt.getDataCallCount += 1;
      return data.has(type) ? data.get(type) : '';
    },
  };
  return dt;
}

const KIND_MIME_LOCAL = 'application/x-termlab-entry-kind-local';
const KIND_MIME_REMOTE = 'application/x-termlab-entry-kind-remote';

function pane(overrides) {
  return {
    isLocal: true,
    pathInput: '/home/user',
    currentPath: '/home/user',
    backStack: [],
    forwardStack: [],
    entries: [],
    showHidden: false,
    colExt: false,
    colSize: true,
    colModified: false,
    error: null,
    transferStatus: null,
    ...overrides,
  };
}

// Renders `p` against a paneRoot wired with a real tbody + table-wrap (so
// dragstart/dragover/drop have somewhere to land), returns
// { paneRoot, tbody, tableWrap, rows }.
function renderWithDnd(view, p, deps) {
  const tbody = fakeElement('tbody');
  const tableWrap = fakeElement('div');
  const paneRoot = fakeElement('div');
  paneRoot.querySelector = (sel) => {
    if (sel === 'tbody') return tbody;
    if (sel === '.fp-table-wrap') return tableWrap;
    return null;
  };
  view.renderPane(p, paneRoot, deps || {});
  return { paneRoot, tbody, tableWrap, rows: tbody.children };
}

// -- dragstart: exact payload, full joined path, isDir flag ----------------
{
  const view = loadPaneView();
  const localFolder = pane({
    isLocal: true,
    currentPath: '/home/user',
    entries: [{ name: 'docs', is_dir: true, size: 0, modified: 0 }],
  });
  const { rows } = renderWithDnd(view, localFolder, {});
  const dt = fakeDataTransfer();
  rows[0].dispatchEvent(dragEvent('dragstart', dt));

  assert.deepEqual(
    dt.types,
    [ENTRY_MIME, KIND_MIME_LOCAL],
    'dragstart must write the JSON payload plus an empty local-kind marker',
  );
  assert.equal(dt.getData(KIND_MIME_LOCAL), '', 'the kind marker carries no data, only its presence in types matters');
  const payload = JSON.parse(dt.getData(ENTRY_MIME));
  assert.deepEqual(payload, {
    paneKind: 'local',
    paneId: null,
    path: '/home/user/docs',
    isDir: true,
  }, 'local dragstart payload: joined path, isDir true, no per-session pane id');
}

// -- dragstart on the remote pane: paneKind 'remote', a file (isDir false),
// paneId taken from activeRemotePaneId -------------------------------------
{
  const view = loadPaneView();
  const remoteFile = pane({
    isLocal: false,
    currentPath: '/srv/app',
    entries: [{ name: 'config.yml', is_dir: false, size: 12, modified: 0 }],
  });
  const { rows } = renderWithDnd(view, remoteFile, { activeRemotePaneId: 7 });
  const dt = fakeDataTransfer();
  rows[0].dispatchEvent(dragEvent('dragstart', dt));

  assert.deepEqual(dt.types, [ENTRY_MIME, KIND_MIME_REMOTE]);
  const payload = JSON.parse(dt.getData(ENTRY_MIME));
  assert.deepEqual(payload, {
    paneKind: 'remote',
    paneId: 7,
    path: '/srv/app/config.yml',
    isDir: false,
  });
}

// -- dragstart uses the panel's own join helper when supplied ---------------
{
  const view = loadPaneView();
  const joinCalls = [];
  const customJoin = (base, name) => { joinCalls.push([base, name]); return `${base}::${name}`; };
  const p = pane({ currentPath: '/x', entries: [{ name: 'y', is_dir: false, size: 0, modified: 0 }] });
  const { rows } = renderWithDnd(view, p, { joinPath: customJoin });
  const dt = fakeDataTransfer();
  rows[0].dispatchEvent(dragEvent('dragstart', dt));
  assert.deepEqual(joinCalls, [['/x', 'y']], 'renderPane must call the supplied joinPath dep, not a private copy');
  assert.equal(JSON.parse(dt.getData(ENTRY_MIME)).path, '/x::y');
}

// -- dragover from the opposite pane kind: preventDefault + is-drop-target,
// decided from `types` alone — getData is never called (finding 2) --------
{
  const view = loadPaneView();
  const localPane = pane({ isLocal: true, currentPath: '/home/user' });
  const { paneRoot, tableWrap } = renderWithDnd(view, localPane, {});
  // A real dragstart would also carry the JSON payload under ENTRY_MIME,
  // but dragover must not need it — only the marker type is set here, to
  // prove the accept decision doesn't depend on it.
  const dt = fakeDataTransfer([ENTRY_MIME, KIND_MIME_REMOTE]);

  const evt = dragEvent('dragover', dt);
  tableWrap.dispatchEvent(evt);
  assert.equal(evt.defaultPrevented, true, 'opposite-kind dragover must preventDefault');
  assert.equal(paneRoot.classList.contains('is-drop-target'), true, 'opposite-kind dragover marks the pane root');
  assert.equal(dt.getDataCallCount, 0, 'dragover must decide accept/reject from types alone, never call getData');
}

// -- dragover from the SAME pane kind: neither preventDefault nor class,
// and still no getData call -------------------------------------------------
{
  const view = loadPaneView();
  const localPane = pane({ isLocal: true, currentPath: '/home/user' });
  const { paneRoot, tableWrap } = renderWithDnd(view, localPane, {});
  const dt = fakeDataTransfer([ENTRY_MIME, KIND_MIME_LOCAL]);

  const evt = dragEvent('dragover', dt);
  tableWrap.dispatchEvent(evt);
  assert.equal(evt.defaultPrevented, false, 'same-kind dragover must not preventDefault (no intra-pane move in v1)');
  assert.equal(paneRoot.classList.contains('is-drop-target'), false, 'same-kind dragover must not mark the pane root');
  assert.equal(dt.getDataCallCount, 0, 'same-kind rejection must also be decided from types alone');
}

// -- foreign dragover (types carries neither kind marker — an OS file drop
// is Task 8's territory): neither preventDefault nor class, no getData ----
{
  const view = loadPaneView();
  const remotePaneObj = pane({ isLocal: false, currentPath: '/srv' });
  const { paneRoot, tableWrap } = renderWithDnd(view, remotePaneObj, { activeRemotePaneId: 1 });
  const dt = fakeDataTransfer(['Files']);

  const evt = dragEvent('dragover', dt);
  tableWrap.dispatchEvent(evt);
  assert.equal(evt.defaultPrevented, false, 'a drag without either kind marker must fall through untouched');
  assert.equal(paneRoot.classList.contains('is-drop-target'), false);
  assert.equal(dt.getDataCallCount, 0);
}

// -- drop: parses the payload and calls onDropEntries with the exact shape;
// class is cleared -----------------------------------------------------
{
  const view = loadPaneView();
  const remotePaneObj = pane({ isLocal: false, currentPath: '/srv/uploads' });
  const dropCalls = [];
  const { paneRoot, tableWrap } = renderWithDnd(view, remotePaneObj, {
    activeRemotePaneId: 9,
    onDropEntries: (payload) => dropCalls.push(payload),
  });
  paneRoot.classList.add('is-drop-target'); // simulate a prior dragover having marked it

  const source = { paneKind: 'local', paneId: null, path: '/home/user/report.pdf', isDir: false };
  const dt = fakeDataTransfer([ENTRY_MIME]);
  dt.setData(ENTRY_MIME, JSON.stringify(source));

  const evt = dragEvent('drop', dt);
  tableWrap.dispatchEvent(evt);

  assert.equal(dropCalls.length, 1, 'an opposite-kind drop must call onDropEntries exactly once');
  assert.deepEqual(j(dropCalls[0]), {
    source,
    targetPaneKind: 'remote',
    targetPath: '/srv/uploads',
  });
  assert.equal(paneRoot.classList.contains('is-drop-target'), false, 'drop must clear the drop-target class');
}

// -- drop of the SAME pane kind: no-op (spec: "no intra-pane move
// semantics in v1"), class still cleared ------------------------------------
{
  const view = loadPaneView();
  const localPaneObj = pane({ isLocal: true, currentPath: '/home/user' });
  const dropCalls = [];
  const { paneRoot, tableWrap } = renderWithDnd(view, localPaneObj, {
    onDropEntries: (payload) => dropCalls.push(payload),
  });
  paneRoot.classList.add('is-drop-target');

  const dt = fakeDataTransfer([ENTRY_MIME]);
  dt.setData(ENTRY_MIME, JSON.stringify({ paneKind: 'local', paneId: null, path: '/home/user/x', isDir: false }));
  tableWrap.dispatchEvent(dragEvent('drop', dt));

  assert.equal(dropCalls.length, 0, 'a same-kind drop must not call onDropEntries');
  assert.equal(paneRoot.classList.contains('is-drop-target'), false, 'drop always clears the class, even as a no-op');
}

// -- dragleave clears the class without requiring a drop --------------------
{
  const view = loadPaneView();
  const localPaneObj = pane({ isLocal: true, currentPath: '/home/user' });
  const { paneRoot, tableWrap } = renderWithDnd(view, localPaneObj, {});
  paneRoot.classList.add('is-drop-target');
  tableWrap.dispatchEvent(dragEvent('dragleave'));
  assert.equal(paneRoot.classList.contains('is-drop-target'), false);
}

// -- dragend on the SOURCE row (review finding 1): a drag cancelled mid-
// flight (Escape, dropped outside any target) fires dragend on the dragged
// row without necessarily firing dragleave on whichever pane is lit.
// dragstart -> dragover (class set on the opposite/target pane) -> dragend
// on the source row, with NO dragleave/drop -> the class must still clear.
// pane-view.js has no handle to the sibling pane's root, so it delegates to
// d.onDragEnd (files-panel.js's job — it can reach both #fp-local/#fp-remote);
// here we simulate that dep clearing every known pane root, exactly as
// files-panel.js's real clearDropTargets() does.
{
  const view = loadPaneView();
  const localPaneObj = pane({ isLocal: true, currentPath: '/home/user', entries: [{ name: 'a.txt', is_dir: false, size: 0, modified: 0 }] });
  const remotePaneObj = pane({ isLocal: false, currentPath: '/srv' });

  let localRoot;
  let remoteRoot;
  const onDragEnd = () => {
    if (localRoot) localRoot.classList.remove('is-drop-target');
    if (remoteRoot) remoteRoot.classList.remove('is-drop-target');
  };

  const localRendered = renderWithDnd(view, localPaneObj, { onDragEnd });
  localRoot = localRendered.paneRoot;
  const remoteRendered = renderWithDnd(view, remotePaneObj, { activeRemotePaneId: 1, onDragEnd });
  remoteRoot = remoteRendered.paneRoot;

  const dt = fakeDataTransfer();
  localRendered.rows[0].dispatchEvent(dragEvent('dragstart', dt));

  // dragover on the opposite (remote) pane accepts and lights its root.
  const overEvt = dragEvent('dragover', dt);
  remoteRendered.tableWrap.dispatchEvent(overEvt);
  assert.equal(overEvt.defaultPrevented, true);
  assert.equal(remoteRoot.classList.contains('is-drop-target'), true, 'setup: dragover must have lit the target pane');

  // Cancelled mid-drag: dragend fires on the SOURCE row, no dragleave/drop.
  localRendered.rows[0].dispatchEvent(dragEvent('dragend', dt));
  assert.equal(remoteRoot.classList.contains('is-drop-target'), false, 'dragend must clear a lit target pane even with no dragleave/drop');
}

console.log('files dnd (pane-view): all assertions passed');

// ===========================================================================
// Part B — files-panel.js: onDropEntries routing (wire-level)
// ===========================================================================
//
// This reuses test_sftp_connect.mjs's "logic harness" idiom: load the real
// files-panel.js (plus its real pane-store/data-service/actions modules),
// stub termlabFilesPaneView.renderPane as a spy that records (pane, el,
// deps), then drive the module by calling the captured deps directly.
// deps.onDropEntries is the real closure files-panel.js wires up in its
// renderPane() wrapper, so calling it here exercises the actual routing —
// not just the data-service call-shape (already covered by
// test_files_transfers.mjs) but the guard, the direction/basename/join
// logic, and the toasts around it.

function makeElement(tag) {
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    dataset: {},
    className: '',
    title: '',
    textContent: '',
    parentNode: null,
    classList: {
      _set: new Set(),
      add(...c) { c.forEach((x) => this._set.add(x)); },
      remove(...c) { c.forEach((x) => this._set.delete(x)); },
      contains(c) { return this._set.has(c); },
    },
    appendChild(child) {
      this.children.push(child);
      if (child && typeof child === 'object') child.parentNode = this;
      return child;
    },
    removeChild(child) {
      const i = this.children.indexOf(child);
      if (i >= 0) this.children.splice(i, 1);
      if (child && typeof child === 'object') child.parentNode = null;
      return child;
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener() {},
    removeEventListener() {},
    dispatchEvent() { return true; },
    setAttribute(n, v) { el._attrs = el._attrs || new Map(); el._attrs.set(n, String(v)); },
    getAttribute(n) { return el._attrs && el._attrs.has(n) ? el._attrs.get(n) : null; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }; },
  };
  return el;
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = async (times = 4) => { for (let i = 0; i < times; i += 1) await tick(); };

async function setupLogicHarness() {
  const invokeCalls = [];
  const toastCalls = [];
  const listeners = {};
  // Task 8: per-path local_stat responses for native-drop tests, keyed by
  // path. A path with no entry rejects with a generic ENOENT-shaped error —
  // tests that need a specific dir/file answer register it here before
  // triggering the drop.
  const localStatByPath = new Map();

  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    if (cmd === 'get_home_dir') return Promise.resolve('/home/demo');
    if (cmd === 'get_all_settings') return Promise.resolve({});
    if (cmd === 'local_list_dir') return Promise.resolve([]);
    if (cmd === 'local_stat') {
      const p = args && args.path;
      if (localStatByPath.has(p)) {
        const entry = localStatByPath.get(p);
        return entry instanceof Error ? Promise.reject(entry) : Promise.resolve(entry);
      }
      return Promise.reject(new Error(`ENOENT: no such file or directory, stat '${p}'`));
    }
    if (cmd === 'remote_get_servers') return Promise.resolve({ folders: [], ungrouped: [], ssh_config: [] });
    if (cmd === 'remote_get_sessions') return Promise.resolve([]);
    if (cmd === 'sftp_realpath') return Promise.resolve('/srv/pinned');
    if (cmd === 'sftp_list_dir') return Promise.resolve([]);
    if (cmd === 'transfer_upload') return Promise.resolve('upload-id');
    if (cmd === 'transfer_download') return Promise.resolve('download-id');
    if (cmd === 'transfer_enqueue_recursive') return Promise.resolve('batch-id');
    return Promise.resolve(undefined);
  };

  const sandbox = {
    console, setTimeout, clearTimeout,
    setInterval: () => 0,
    clearInterval: () => {},
    Promise, Math, Array, JSON, Object, String, Number,
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  sandbox.utils = {
    formatSize: () => '', formatDate: () => '',
    esc: (v) => String(v == null ? '' : v), attr: (v) => String(v == null ? '' : v),
  };
  sandbox.toast = {
    error: (...args) => toastCalls.push({ kind: 'error', args }),
    info: (...args) => toastCalls.push({ kind: 'info', args }),
    warn: (...args) => toastCalls.push({ kind: 'warn', args }),
    success: (...args) => toastCalls.push({ kind: 'success', args }),
  };
  sandbox.toolWindowManager = { isVisible: () => true, activate() {}, deactivate() {} };
  sandbox.tlCombo = { attach: () => ({ button: makeElement('button'), refresh() {} }) };
  const renderCalls = [];
  sandbox.termlabFilesPaneView = {
    renderPane: (pane, el, deps) => { renderCalls.push({ pane, el, deps }); },
    showColumnMenu: () => {},
    showRowContextMenu: () => {},
  };
  sandbox.termlabFilesTransfers = {};

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(FILES_PANE_STORE_PATH, 'utf8'), sandbox, { filename: FILES_PANE_STORE_PATH });
  vm.runInContext(fs.readFileSync(FILES_DATA_SERVICE_PATH, 'utf8'), sandbox, { filename: FILES_DATA_SERVICE_PATH });
  vm.runInContext(fs.readFileSync(FILES_ACTIONS_PATH, 'utf8'), sandbox, { filename: FILES_ACTIONS_PATH });
  // native-drop.js must load before files-panel.js — files-panel.js reaches
  // it as window.termlabNativeDrop, same load-order requirement index.html
  // encodes (native-drop.js's <script> tag precedes files-panel.js's).
  vm.runInContext(fs.readFileSync(NATIVE_DROP_PATH, 'utf8'), sandbox, { filename: NATIVE_DROP_PATH });
  vm.runInContext(fs.readFileSync(FILES_PANEL_PATH, 'utf8'), sandbox, { filename: FILES_PANEL_PATH });

  const panelEl = makeElement('div');
  const localRootEl = makeElement('div');
  const remoteRootEl = makeElement('div');
  panelEl.querySelector = (sel) => {
    if (sel === '#fp-local') return localRootEl;
    if (sel === '#fp-remote') return remoteRootEl;
    return null;
  };

  sandbox.filesPanel.init({
    invoke,
    panelEl,
    panelWrapEl: makeElement('div'),
    resizeHandleEl: makeElement('div'),
    layoutService: null,
    fitActiveTab: () => {},
    getActiveTab: () => null,
    listen: (name, handler) => { listeners[name] = handler; },
  });
  await settle();
  const initialLocalRender = lastCall(renderCalls, 'local');
  const initialRemoteRender = lastCall(renderCalls, 'remote');
  invokeCalls.length = 0;
  toastCalls.length = 0;
  renderCalls.length = 0;

  return {
    sandbox, invoke, invokeCalls, toastCalls, renderCalls, listeners,
    initialLocalRender, initialRemoteRender, localStatByPath,
  };
}

function lastCall(renderCalls, prefix) {
  for (let i = renderCalls.length - 1; i >= 0; i -= 1) {
    if (renderCalls[i].pane.prefix === prefix) return renderCalls[i];
  }
  throw new Error(`no ${prefix}-pane renderPane call recorded yet`);
}

// -- not connected: dropping onto either pane with no active remote session
// shows the guard toast and calls neither transfer command -----------------
{
  const h = await setupLogicHarness();
  const { deps } = h.initialLocalRender;
  await deps.onDropEntries({
    source: { paneKind: 'remote', paneId: 3, path: '/srv/a/file.txt', isDir: false },
    targetPaneKind: 'local',
    targetPath: '/home/demo',
  });
  assert.deepEqual(h.invokeCalls, [], 'no transfer command may fire without an active remote session');
  assert.equal(h.toastCalls.length, 1);
  assert.equal(h.toastCalls[0].kind, 'warn');
  console.log('B1. not-connected guard: ok');
}

// -- connected: pin a remote session, then drop a local FILE onto the
// remote pane -> transfer_upload with the pre-joined destination path,
// mirroring doUpload exactly --------------------------------------------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;

  const { deps } = h.initialRemoteRender;
  await deps.onDropEntries({
    source: { paneKind: 'local', paneId: null, path: '/home/demo/notes.txt', isDir: false },
    targetPaneKind: 'remote',
    targetPath: '/srv/pinned',
  });
  await settle();

  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['transfer_upload']);
  assert.deepEqual(j(h.invokeCalls[0].args), {
    paneId: 1000007,
    localPath: '/home/demo/notes.txt',
    remotePath: '/srv/pinned/notes.txt',
    origin: 'filesPanel',
    conflictPolicy: { kind: 'ask' },
  }, 'single-file drop must pre-join the destination filename, exactly like doUpload');
  console.log('B2. connected file drop (local -> remote, upload): ok');
}

// -- connected: drop a remote FILE onto the local pane -> transfer_download,
// mirroring doDownload exactly ----------------------------------------------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;

  const { deps } = h.initialLocalRender;
  await deps.onDropEntries({
    source: { paneKind: 'remote', paneId: 1000007, path: '/srv/pinned/report.pdf', isDir: false },
    targetPaneKind: 'local',
    targetPath: '/home/demo',
  });
  await settle();

  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['transfer_download']);
  assert.deepEqual(j(h.invokeCalls[0].args), {
    paneId: 1000007,
    remotePath: '/srv/pinned/report.pdf',
    localPath: '/home/demo/report.pdf',
    origin: 'filesPanel',
    conflictPolicy: { kind: 'ask' },
  });
  console.log('B3. connected file drop (remote -> local, download): ok');
}

// -- connected: drop a local FOLDER onto the remote pane ->
// transfer_enqueue_recursive with destPath AS-IS (no basename join),
// mirroring doUploadFolder exactly ------------------------------------------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;

  const { deps } = h.initialRemoteRender;
  await deps.onDropEntries({
    source: { paneKind: 'local', paneId: null, path: '/home/demo/photos', isDir: true },
    targetPaneKind: 'remote',
    targetPath: '/srv/pinned',
  });
  await settle();

  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['transfer_enqueue_recursive']);
  assert.deepEqual(j(h.invokeCalls[0].args), {
    paneId: 1000007,
    direction: 'upload',
    sourcePath: '/home/demo/photos',
    destPath: '/srv/pinned',
  }, 'folder drop must hand destPath AS-IS — the backend appends the basename');
  assert.equal(h.toastCalls.some((c) => c.kind === 'info'), true, 'a folder drop announces the started transfer');
  console.log('B4. connected folder drop (local -> remote, recursive upload): ok');
}

// -- connected: drop a remote FOLDER onto the local pane ->
// transfer_enqueue_recursive (download), mirroring doDownloadFolder --------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;

  const { deps } = h.initialLocalRender;
  await deps.onDropEntries({
    source: { paneKind: 'remote', paneId: 1000007, path: '/srv/pinned/logs', isDir: true },
    targetPaneKind: 'local',
    targetPath: '/home/demo',
  });
  await settle();

  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['transfer_enqueue_recursive']);
  assert.deepEqual(j(h.invokeCalls[0].args), {
    paneId: 1000007,
    direction: 'download',
    sourcePath: '/srv/pinned/logs',
    destPath: '/home/demo',
  });
  console.log('B5. connected folder drop (remote -> local, recursive download): ok');
}

// -- a transfer command rejection surfaces as an error toast, not a crash --
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;
  h.toastCalls.length = 0;

  const { deps } = h.initialRemoteRender;
  h.sandbox.termlabFilesFeatureDataService.transferUpload = () => Promise.reject(new Error('disk full'));
  await deps.onDropEntries({
    source: { paneKind: 'local', paneId: null, path: '/home/demo/big.iso', isDir: false },
    targetPaneKind: 'remote',
    targetPath: '/srv/pinned',
  });
  await settle();

  assert.equal(h.toastCalls.some((c) => c.kind === 'error'), true, 'a rejected transfer must surface an error toast');
  assert.equal(
    JSON.stringify(h.toastCalls).includes('disk full'),
    true,
    'the error toast should include the underlying failure reason',
  );
  console.log('B6. transfer failure surfaces an error toast: ok');
}

// -- onDragEnd (review finding 1, files-panel.js side): clears the
// is-drop-target class on BOTH #fp-local and #fp-remote roots
// unconditionally — this is the real clearDropTargets() pane-view.js's
// dragend handler delegates to, since it has no handle to the sibling
// pane's root itself. ---------------------------------------------------
{
  const h = await setupLogicHarness();
  const localEl = h.initialLocalRender.el;
  const remoteEl = h.initialRemoteRender.el;
  localEl.classList.add('is-drop-target');
  remoteEl.classList.add('is-drop-target');

  h.initialLocalRender.deps.onDragEnd();

  assert.equal(localEl.classList.contains('is-drop-target'), false, 'onDragEnd must clear the local pane root');
  assert.equal(remoteEl.classList.contains('is-drop-target'), false, 'onDragEnd must clear the remote pane root too, unconditionally');
  console.log('B7. onDragEnd clears both pane roots: ok');
}

console.log('files dnd (files-panel routing): all assertions passed');

// ===========================================================================
// Part C — features/files/native-drop.js: pure hit-test + routing (Task 8)
// ===========================================================================
//
// OS (Finder/Explorer) drops arrive as window-level Tauri v2 events carrying
// real filesystem paths — a different delivery mechanism from Part A/B's
// intra-app DOM drag/drop. native-drop.js is deliberately dependency-free
// (no DOM, no `invoke`, no files-panel.js module state) so these can run
// against nothing but the file itself, loaded standalone.

function loadNativeDrop() {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(NATIVE_DROP_PATH, 'utf8'), sandbox, { filename: NATIVE_DROP_PATH });
  assert.ok(sandbox.termlabNativeDrop, 'native-drop IIFE must expose window.termlabNativeDrop');
  return sandbox.termlabNativeDrop;
}

const PANE_RECT = { left: 10, top: 20, right: 110, bottom: 220 };

// -- resolveNativeDrop: inside the rect + a session -> 'accept' ------------
{
  const nativeDrop = loadNativeDrop();
  assert.equal(nativeDrop.resolveNativeDrop({ x: 50, y: 50 }, PANE_RECT, true), 'accept');
  console.log('C1. resolveNativeDrop: hit + session -> accept: ok');
}

// -- resolveNativeDrop: inside the rect, no session -> 'no-session' --------
{
  const nativeDrop = loadNativeDrop();
  assert.equal(nativeDrop.resolveNativeDrop({ x: 50, y: 50 }, PANE_RECT, false), 'no-session');
  console.log('C2. resolveNativeDrop: hit + no session -> no-session: ok');
}

// -- resolveNativeDrop: outside the rect -> 'ignore', session state doesn't
// matter --------------------------------------------------------------------
{
  const nativeDrop = loadNativeDrop();
  assert.equal(nativeDrop.resolveNativeDrop({ x: 5, y: 50 }, PANE_RECT, true), 'ignore', 'left of the rect');
  assert.equal(nativeDrop.resolveNativeDrop({ x: 500, y: 50 }, PANE_RECT, true), 'ignore', 'right of the rect');
  assert.equal(nativeDrop.resolveNativeDrop({ x: 50, y: 5 }, PANE_RECT, true), 'ignore', 'above the rect');
  assert.equal(nativeDrop.resolveNativeDrop({ x: 50, y: 500 }, PANE_RECT, true), 'ignore', 'below the rect');
  assert.equal(nativeDrop.resolveNativeDrop({ x: 5, y: 50 }, PANE_RECT, false), 'ignore', 'a miss ignores even with no session');
  console.log('C3. resolveNativeDrop: miss on every side -> ignore: ok');
}

// -- resolveNativeDrop: rect edges — left/top inclusive, right/bottom
// exclusive (the pinned convention; matches getBoundingClientRect()'s own
// half-open box) --------------------------------------------------------
{
  const nativeDrop = loadNativeDrop();
  assert.equal(nativeDrop.resolveNativeDrop({ x: 10, y: 20 }, PANE_RECT, true), 'accept', 'top-left corner is inside (inclusive)');
  assert.equal(nativeDrop.resolveNativeDrop({ x: 110, y: 100 }, PANE_RECT, true), 'ignore', 'right edge is outside (exclusive)');
  assert.equal(nativeDrop.resolveNativeDrop({ x: 50, y: 220 }, PANE_RECT, true), 'ignore', 'bottom edge is outside (exclusive)');
  console.log('C4. resolveNativeDrop: edge inclusivity pinned: ok');
}

// -- resolveNativeDrop: missing position or rect -> 'ignore', never throws -
{
  const nativeDrop = loadNativeDrop();
  assert.equal(nativeDrop.resolveNativeDrop(null, PANE_RECT, true), 'ignore');
  assert.equal(nativeDrop.resolveNativeDrop({ x: 50, y: 50 }, null, true), 'ignore');
  console.log('C5. resolveNativeDrop: missing position/rect -> ignore: ok');
}

// -- routeNativeDropPaths: a directory routes to transferRecursive with the
// destination CONTAINER as-is (no basename join — the backend appends it) -
{
  const nativeDrop = loadNativeDrop();
  const recursiveCalls = [];
  const uploadCalls = [];
  const toastCalls = [];
  await nativeDrop.routeNativeDropPaths(['/home/demo/photos'], {
    statPath: async () => ({ isDir: true }),
    transferRecursive: async (paneId, sourcePath, destPath) => { recursiveCalls.push({ paneId, sourcePath, destPath }); },
    transferUpload: async (paneId, sourcePath, destPath) => { uploadCalls.push({ paneId, sourcePath, destPath }); },
    targetPaneId: 1000007,
    targetPath: '/srv/pinned',
    toast: { error: (...a) => toastCalls.push({ kind: 'error', a }), info: (...a) => toastCalls.push({ kind: 'info', a }) },
  });
  assert.deepEqual(recursiveCalls, [{ paneId: 1000007, sourcePath: '/home/demo/photos', destPath: '/srv/pinned' }]);
  assert.equal(uploadCalls.length, 0);
  assert.equal(toastCalls.some((c) => c.kind === 'info'), true, 'a folder route announces the started transfer');
  console.log('C6. routeNativeDropPaths: directory -> transferRecursive, container dest: ok');
}

// -- routeNativeDropPaths: a file routes to transferUpload with the
// destination filename pre-joined, mirroring the single-file path exactly -
{
  const nativeDrop = loadNativeDrop();
  const uploadCalls = [];
  await nativeDrop.routeNativeDropPaths(['/home/demo/notes.txt'], {
    statPath: async () => ({ isDir: false }),
    transferRecursive: async () => { throw new Error('must not be called for a file'); },
    transferUpload: async (paneId, sourcePath, destPath) => { uploadCalls.push({ paneId, sourcePath, destPath }); },
    targetPaneId: 1000007,
    targetPath: '/srv/pinned',
    toast: null,
  });
  assert.deepEqual(uploadCalls, [{ paneId: 1000007, sourcePath: '/home/demo/notes.txt', destPath: '/srv/pinned/notes.txt' }]);
  console.log('C7. routeNativeDropPaths: file -> transferUpload, joined dest: ok');
}

// -- routeNativeDropPaths: a mixed list routes each path independently, in
// order ----------------------------------------------------------------
{
  const nativeDrop = loadNativeDrop();
  const recursiveCalls = [];
  const uploadCalls = [];
  const stats = { '/home/demo/photos': { isDir: true }, '/home/demo/notes.txt': { isDir: false } };
  await nativeDrop.routeNativeDropPaths(['/home/demo/photos', '/home/demo/notes.txt'], {
    statPath: async (p) => stats[p],
    transferRecursive: async (paneId, sourcePath, destPath) => { recursiveCalls.push({ paneId, sourcePath, destPath }); },
    transferUpload: async (paneId, sourcePath, destPath) => { uploadCalls.push({ paneId, sourcePath, destPath }); },
    targetPaneId: 1000007,
    targetPath: '/srv/pinned',
    toast: { error() {}, info() {} },
  });
  assert.deepEqual(recursiveCalls, [{ paneId: 1000007, sourcePath: '/home/demo/photos', destPath: '/srv/pinned' }]);
  assert.deepEqual(uploadCalls, [{ paneId: 1000007, sourcePath: '/home/demo/notes.txt', destPath: '/srv/pinned/notes.txt' }]);
  console.log('C8. routeNativeDropPaths: mixed list routes each entry: ok');
}

// -- routeNativeDropPaths: a stat failure on one path reports via
// toast.error for THAT path and continues routing the rest ----------------
{
  const nativeDrop = loadNativeDrop();
  const uploadCalls = [];
  const toastCalls = [];
  await nativeDrop.routeNativeDropPaths(['/home/demo/missing.txt', '/home/demo/notes.txt'], {
    statPath: async (p) => {
      if (p === '/home/demo/missing.txt') throw new Error('ENOENT');
      return { isDir: false };
    },
    transferRecursive: async () => { throw new Error('must not be called'); },
    transferUpload: async (paneId, sourcePath, destPath) => { uploadCalls.push({ paneId, sourcePath, destPath }); },
    targetPaneId: 1000007,
    targetPath: '/srv/pinned',
    toast: { error: (...a) => toastCalls.push({ kind: 'error', a }), info: () => {} },
  });
  assert.deepEqual(uploadCalls, [{ paneId: 1000007, sourcePath: '/home/demo/notes.txt', destPath: '/srv/pinned/notes.txt' }],
    'the second path must still be routed after the first path\'s stat failed');
  assert.equal(toastCalls.length, 1);
  assert.equal(toastCalls[0].kind, 'error');
  assert.ok(String(toastCalls[0].a.join(' ')).includes('missing.txt'), 'the error toast should identify which path failed');
  console.log('C9. routeNativeDropPaths: stat failure toasts and continues: ok');
}

// -- routeNativeDropPaths: a transfer rejection (not just a stat failure)
// also toasts and continues to the next path --------------------------------
{
  const nativeDrop = loadNativeDrop();
  const toastCalls = [];
  const uploadCalls = [];
  await nativeDrop.routeNativeDropPaths(['/home/demo/big.iso', '/home/demo/notes.txt'], {
    statPath: async () => ({ isDir: false }),
    transferRecursive: async () => {},
    transferUpload: async (paneId, sourcePath, destPath) => {
      if (sourcePath === '/home/demo/big.iso') throw new Error('disk full');
      uploadCalls.push({ paneId, sourcePath, destPath });
    },
    targetPaneId: 1000007,
    targetPath: '/srv/pinned',
    toast: { error: (...a) => toastCalls.push({ kind: 'error', a }), info: () => {} },
  });
  assert.deepEqual(uploadCalls, [{ paneId: 1000007, sourcePath: '/home/demo/notes.txt', destPath: '/srv/pinned/notes.txt' }]);
  assert.equal(toastCalls.some((c) => c.kind === 'error' && String(c.a.join(' ')).includes('disk full')), true);
  console.log('C10. routeNativeDropPaths: a rejected transfer toasts and continues: ok');
}

console.log('files dnd (native-drop pure): all assertions passed');

// ===========================================================================
// Part D — files-panel.js: native OS drop wiring (Task 8, wire-level)
// ===========================================================================
//
// Same logic-harness idiom as Part B, but exercising the FOUR listeners
// init() registers via opts.listen for 'tauri://drag-enter/-over/-leave/
// -drop' — captured in h.listeners exactly like 'transfer-progress' is —
// instead of calling deps.onDropEntries directly. This is what proves the
// real hit-test-against-getBoundingClientRect + devicePixelRatio-scaling +
// routing wiring in files-panel.js's handleNativeDrop/-DragHover/-DragLeave,
// not just native-drop.js's own pure functions (Part C).

// -- not connected: a drop that hits the remote pane rect with no active
// session shows the guard toast and calls neither stat nor transfer -------
{
  const h = await setupLogicHarness();
  h.initialRemoteRender.el.getBoundingClientRect = () => ({ left: 0, top: 0, right: 200, bottom: 200 });

  await h.listeners['tauri://drag-drop']({
    payload: { paths: ['/home/demo/notes.txt'], position: { x: 50, y: 50 } },
  });

  assert.deepEqual(h.invokeCalls, [], 'no local_stat/transfer command may fire without an active remote session');
  assert.equal(h.toastCalls.length, 1);
  assert.equal(h.toastCalls[0].kind, 'warn');
  console.log('D1. native drop, not connected: guard toast, no commands: ok');
}

// -- connected, drop hits the rect, dropped path is a directory ->
// local_stat then transfer_enqueue_recursive with the container dest ------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;
  h.toastCalls.length = 0;
  h.initialRemoteRender.el.getBoundingClientRect = () => ({ left: 0, top: 0, right: 200, bottom: 200 });
  h.localStatByPath.set('/home/demo/photos', { name: 'photos', is_dir: true, size: 0, modified: 0 });

  await h.listeners['tauri://drag-drop']({
    payload: { paths: ['/home/demo/photos'], position: { x: 50, y: 50 } },
  });
  await settle();

  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['local_stat', 'transfer_enqueue_recursive']);
  assert.deepEqual(j(h.invokeCalls[1].args), {
    paneId: 1000007,
    direction: 'upload',
    sourcePath: '/home/demo/photos',
    destPath: '/srv/pinned',
  });
  assert.equal(h.toastCalls.some((c) => c.kind === 'info'), true, 'a folder drop announces the started transfer');
  console.log('D2. native drop, connected, directory: stats then recurses: ok');
}

// -- connected, drop hits the rect, dropped path is a file -> local_stat
// then transfer_upload with the destination filename pre-joined -----------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;
  h.toastCalls.length = 0;
  h.initialRemoteRender.el.getBoundingClientRect = () => ({ left: 0, top: 0, right: 200, bottom: 200 });
  h.localStatByPath.set('/home/demo/notes.txt', { name: 'notes.txt', is_dir: false, size: 12, modified: 0 });

  await h.listeners['tauri://drag-drop']({
    payload: { paths: ['/home/demo/notes.txt'], position: { x: 50, y: 50 } },
  });
  await settle();

  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['local_stat', 'transfer_upload']);
  assert.deepEqual(j(h.invokeCalls[1].args), {
    paneId: 1000007,
    localPath: '/home/demo/notes.txt',
    remotePath: '/srv/pinned/notes.txt',
    origin: 'filesPanel',
    conflictPolicy: { kind: 'ask' },
  });
  console.log('D3. native drop, connected, file: stats then uploads to joined dest: ok');
}

// -- connected, but the drop position misses the remote pane rect entirely:
// ignored silently — no toast, no local_stat, no transfer ------------------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;
  h.toastCalls.length = 0;
  h.initialRemoteRender.el.getBoundingClientRect = () => ({ left: 0, top: 0, right: 200, bottom: 200 });

  await h.listeners['tauri://drag-drop']({
    payload: { paths: ['/home/demo/notes.txt'], position: { x: 999, y: 999 } },
  });
  await settle();

  assert.deepEqual(h.invokeCalls, [], 'a drop that misses the remote pane must be silently ignored');
  assert.deepEqual(h.toastCalls, []);
  console.log('D4. native drop, miss: ignored silently: ok');
}

// -- drag-enter/-over: mark is-drop-target only when the position hits the
// remote pane AND a session is active; a miss or no-session must not mark
// (and must clear any prior mark) -------------------------------------------
{
  const h = await setupLogicHarness();
  const remoteEl = h.initialRemoteRender.el;
  remoteEl.getBoundingClientRect = () => ({ left: 0, top: 0, right: 200, bottom: 200 });

  // No session yet: even a hit must not mark.
  h.listeners['tauri://drag-enter']({ payload: { position: { x: 50, y: 50 } } });
  assert.equal(remoteEl.classList.contains('is-drop-target'), false, 'no session: a hit must not mark the pane');

  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();

  h.listeners['tauri://drag-over']({ payload: { position: { x: 50, y: 50 } } });
  assert.equal(remoteEl.classList.contains('is-drop-target'), true, 'connected + hit must mark the pane');

  h.listeners['tauri://drag-over']({ payload: { position: { x: 999, y: 999 } } });
  assert.equal(remoteEl.classList.contains('is-drop-target'), false, 'moving off the pane must clear the mark');
  console.log('D5. native drag-enter/-over: marks only on hit + session, clears on miss: ok');
}

// -- drag-leave always clears is-drop-target, unconditionally --------------
{
  const h = await setupLogicHarness();
  const remoteEl = h.initialRemoteRender.el;
  remoteEl.classList.add('is-drop-target');
  h.listeners['tauri://drag-leave']();
  assert.equal(remoteEl.classList.contains('is-drop-target'), false);
  console.log('D6. native drag-leave: clears unconditionally: ok');
}

// -- the physical-pixel trap: Tauri reports drag-drop positions in PHYSICAL
// pixels, getBoundingClientRect() is LOGICAL — the same physical position
// must hit at devicePixelRatio 2 but miss at devicePixelRatio 1 against a
// rect sized in logical px, proving files-panel.js actually divides by
// devicePixelRatio before hit-testing rather than comparing raw values ----
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;
  h.toastCalls.length = 0;
  h.initialRemoteRender.el.getBoundingClientRect = () => ({ left: 0, top: 0, right: 100, bottom: 100 });
  h.localStatByPath.set('/home/demo/notes.txt', { name: 'notes.txt', is_dir: false, size: 12, modified: 0 });

  // devicePixelRatio 1: a physical (150, 150) position stays (150, 150)
  // logical -- outside a 100x100 rect, so this drop must be ignored.
  h.sandbox.devicePixelRatio = 1;
  await h.listeners['tauri://drag-drop']({
    payload: { paths: ['/home/demo/notes.txt'], position: { x: 150, y: 150 } },
  });
  await settle();
  assert.deepEqual(h.invokeCalls, [], 'at devicePixelRatio 1, physical (150,150) misses a 100x100 logical rect');

  // devicePixelRatio 2: the SAME physical (150, 150) position scales to
  // (75, 75) logical -- inside the rect, so this drop must route.
  h.sandbox.devicePixelRatio = 2;
  await h.listeners['tauri://drag-drop']({
    payload: { paths: ['/home/demo/notes.txt'], position: { x: 150, y: 150 } },
  });
  await settle();
  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['local_stat', 'transfer_upload'],
    'at devicePixelRatio 2, the same physical (150,150) scales to logical (75,75) and hits');
  console.log('D7. native drop: devicePixelRatio scales physical position before hit-testing: ok');
}

console.log('files dnd (native drop wiring): all assertions passed');

// ===========================================================================
// Part E — core/dragdrop-runtime.js: terminal drop must not fire for an OS
// drop elsewhere in the window (Task 8 fix round)
// ===========================================================================
//
// Before this fix, dragdrop-runtime.js's `currentWindow.onDragDropEvent`
// listener reacted to EVERY window-level Tauri drag/drop event regardless of
// where it landed — so a Finder drop onto the SFTP remote pane (Part D
// above) also pasted the dropped paths into whatever terminal pane was
// focused. This section loads the real dragdrop-runtime.js (plus
// native-drop.js, which it now shares scaleNativeDropPosition/pointInRect
// with — same load-order requirement index.html encodes) in a VM, captures
// the callback it registers via a fake `currentWindow.onDragDropEvent`, and
// drives that callback directly with 'over'/'drop'/'leave' payloads — full
// wire-level coverage of the fix, not just the shared pure hit-test helpers
// (already covered standalone in Part C).

function loadDragDropRuntime() {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(NATIVE_DROP_PATH, 'utf8'), sandbox, { filename: NATIVE_DROP_PATH });
  vm.runInContext(fs.readFileSync(DRAGDROP_RUNTIME_PATH, 'utf8'), sandbox, { filename: DRAGDROP_RUNTIME_PATH });
  assert.ok(sandbox.termlabDragDropRuntime, 'dragdrop-runtime IIFE must expose window.termlabDragDropRuntime');
  return sandbox;
}

// Wires a fresh dragdrop-runtime instance against a fake terminal host
// (rect defaults to a 200x200 box at the origin) and a fake `currentWindow`
// that just captures the onDragDropEvent callback instead of registering it
// with anything real. Returns everything a test needs: the element (so its
// rect/class can be inspected or overridden), the captured callback, and the
// invoke-call log writePathsToTerminal feeds.
function setupDragDropHarness(rectOverride) {
  const sandbox = loadDragDropRuntime();
  const terminalHostEl = makeElement('div');
  terminalHostEl.getBoundingClientRect = () => (
    rectOverride || { left: 0, top: 0, right: 200, bottom: 200 }
  );
  const invokeCalls = [];
  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    return Promise.resolve();
  };
  let onDragDropEventCallback = null;
  const currentWindow = {
    onDragDropEvent: (cb) => { onDragDropEventCallback = cb; },
  };
  const pane = { paneId: 42, spawned: true, type: 'local' };
  const runtime = sandbox.termlabDragDropRuntime.create({
    terminalHostEl,
    currentWindow,
    getCurrentPane: () => pane,
    invoke,
  });
  runtime.init();
  assert.ok(onDragDropEventCallback, 'init must register a currentWindow.onDragDropEvent callback');
  return {
    sandbox, terminalHostEl, invokeCalls,
    fire: (payload) => onDragDropEventCallback({ payload }),
  };
}

// -- drop with a position INSIDE the terminal host's rect: pastes, exactly
// like before the fix -------------------------------------------------------
{
  const h = setupDragDropHarness();
  h.fire({ type: 'drop', paths: ['/home/demo/notes.txt'], position: { x: 50, y: 50 } });
  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['write_to_pty']);
  assert.deepEqual(j(h.invokeCalls[0].args), { paneId: 42, data: '/home/demo/notes.txt' });
  console.log('E1. terminal drop, position hits terminal host: pastes (unchanged behavior): ok');
}

// -- drop with a position OUTSIDE the terminal host's rect (e.g. it landed
// on the SFTP remote pane instead): must NOT paste into the terminal — this
// is the collision the fix closes -------------------------------------------
{
  const h = setupDragDropHarness();
  h.fire({ type: 'drop', paths: ['/home/demo/notes.txt'], position: { x: 999, y: 999 } });
  assert.deepEqual(h.invokeCalls, [], 'a drop that misses the terminal host must not write anything to it');
  assert.equal(h.terminalHostEl.classList.contains('drag-over'), false);
  console.log('E2. terminal drop, position misses terminal host: does not paste: ok');
}

// -- drop with NO position in the payload: today's unconditional paste
// behavior is preserved (some event shapes may omit position) -------------
{
  const h = setupDragDropHarness();
  h.fire({ type: 'drop', paths: ['/home/demo/notes.txt'] });
  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['write_to_pty'],
    'a drop event with no position payload must keep the pre-fix unconditional paste');
  console.log('E3. terminal drop, no position in payload: keeps unconditional paste: ok');
}

// -- over: marks drag-over only while the position is inside the terminal
// host; moving off it (still 'over', new position) must clear the mark ----
{
  const h = setupDragDropHarness();
  h.fire({ type: 'over', position: { x: 50, y: 50 } });
  assert.equal(h.terminalHostEl.classList.contains('drag-over'), true, 'a hit must mark the terminal host');
  h.fire({ type: 'over', position: { x: 999, y: 999 } });
  assert.equal(h.terminalHostEl.classList.contains('drag-over'), false, 'moving off the terminal host must clear the mark');
  console.log('E4. terminal drag-over: marks on hit, clears when it moves off: ok');
}

// -- leave: always clears drag-over, unaffected by the hit-test -----------
{
  const h = setupDragDropHarness();
  h.fire({ type: 'over', position: { x: 50, y: 50 } });
  assert.equal(h.terminalHostEl.classList.contains('drag-over'), true);
  h.fire({ type: 'leave' });
  assert.equal(h.terminalHostEl.classList.contains('drag-over'), false);
  console.log('E5. terminal drag-leave: clears unconditionally: ok');
}

// -- the physical-pixel trap, terminal side: the identical physical position
// hits at devicePixelRatio 2 and misses at devicePixelRatio 1 against the
// same logical rect — proves dragdrop-runtime.js's hit-test actually shares
// native-drop.js's scaling rather than comparing raw physical coordinates -
{
  const h = setupDragDropHarness({ left: 0, top: 0, right: 100, bottom: 100 });

  h.sandbox.devicePixelRatio = 1;
  h.fire({ type: 'drop', paths: ['/home/demo/notes.txt'], position: { x: 150, y: 150 } });
  assert.deepEqual(h.invokeCalls, [], 'at devicePixelRatio 1, physical (150,150) misses a 100x100 logical rect');

  h.sandbox.devicePixelRatio = 2;
  h.fire({ type: 'drop', paths: ['/home/demo/notes.txt'], position: { x: 150, y: 150 } });
  assert.deepEqual(h.invokeCalls.map((c) => c.cmd), ['write_to_pty'],
    'at devicePixelRatio 2, the same physical (150,150) scales to logical (75,75) and hits');
  console.log('E6. terminal drop: devicePixelRatio scales physical position before hit-testing: ok');
}

console.log('files dnd (terminal drop collision fix): all assertions passed');
