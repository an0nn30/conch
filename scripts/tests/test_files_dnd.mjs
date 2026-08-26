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
// the real DataTransfer does.
function fakeDataTransfer(initialTypes = []) {
  const data = new Map();
  return {
    types: initialTypes.slice(),
    data,
    setData(type, value) {
      data.set(type, value);
      if (this.types.indexOf(type) === -1) this.types.push(type);
    },
    getData(type) { return data.has(type) ? data.get(type) : ''; },
  };
}

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

  assert.deepEqual(dt.types, [ENTRY_MIME], 'dragstart must write exactly the one entry MIME type');
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

// -- dragover from the opposite pane kind: preventDefault + is-drop-target -
{
  const view = loadPaneView();
  const localPane = pane({ isLocal: true, currentPath: '/home/user' });
  const { paneRoot, tableWrap } = renderWithDnd(view, localPane, {});
  const dt = fakeDataTransfer([ENTRY_MIME]);
  dt.setData(ENTRY_MIME, JSON.stringify({ paneKind: 'remote', paneId: 3, path: '/srv/a', isDir: false }));

  const evt = dragEvent('dragover', dt);
  tableWrap.dispatchEvent(evt);
  assert.equal(evt.defaultPrevented, true, 'opposite-kind dragover must preventDefault');
  assert.equal(paneRoot.classList.contains('is-drop-target'), true, 'opposite-kind dragover marks the pane root');
}

// -- dragover from the SAME pane kind: neither preventDefault nor class ----
{
  const view = loadPaneView();
  const localPane = pane({ isLocal: true, currentPath: '/home/user' });
  const { paneRoot, tableWrap } = renderWithDnd(view, localPane, {});
  const dt = fakeDataTransfer([ENTRY_MIME]);
  dt.setData(ENTRY_MIME, JSON.stringify({ paneKind: 'local', paneId: null, path: '/home/user/other', isDir: false }));

  const evt = dragEvent('dragover', dt);
  tableWrap.dispatchEvent(evt);
  assert.equal(evt.defaultPrevented, false, 'same-kind dragover must not preventDefault (no intra-pane move in v1)');
  assert.equal(paneRoot.classList.contains('is-drop-target'), false, 'same-kind dragover must not mark the pane root');
}

// -- foreign dragover (types lacks the entry MIME — an OS file drop is
// Task 8's territory): neither preventDefault nor class --------------------
{
  const view = loadPaneView();
  const remotePaneObj = pane({ isLocal: false, currentPath: '/srv' });
  const { paneRoot, tableWrap } = renderWithDnd(view, remotePaneObj, { activeRemotePaneId: 1 });
  const dt = fakeDataTransfer(['Files']);

  const evt = dragEvent('dragover', dt);
  tableWrap.dispatchEvent(evt);
  assert.equal(evt.defaultPrevented, false, 'a drag without our MIME must fall through untouched');
  assert.equal(paneRoot.classList.contains('is-drop-target'), false);
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

  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    if (cmd === 'get_home_dir') return Promise.resolve('/home/demo');
    if (cmd === 'get_all_settings') return Promise.resolve({});
    if (cmd === 'local_list_dir') return Promise.resolve([]);
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
    initialLocalRender, initialRemoteRender,
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

console.log('files dnd (files-panel routing): all assertions passed');
