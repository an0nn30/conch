// Run: node scripts/tests/test_lsp_editor_lifecycle.mjs
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const FRONTEND = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(FRONTEND, 'app');

let ran = 0;
let failures = 0;
async function check(name, fn) {
  ran += 1;
  try {
    await fn();
    console.log(`  ok ${name}`);
  } catch (error) {
    failures += 1;
    console.error(`  FAIL ${name}\n       ${error.stack || error.message}`);
  }
}

function load(sandbox, relative) {
  const filename = path.join(APP, relative);
  vm.runInContext(fs.readFileSync(filename, 'utf8'), sandbox, { filename });
}

function response(pathname, documentId = 'doc-1') {
  return {
    documentId,
    version: 1,
    projectCandidates: [],
    status: {
      revision: 1,
      documentId,
      sessionId: null,
      adapterId: 'typescript',
      projectRootUri: null,
      state: 'choosingProject',
      message: null,
      capabilities: { completion: false, hover: false, signatureHelp: false, definition: false, diagnostics: false },
      errorCount: 0,
      warningCount: 0,
      pathname,
    },
  };
}

function harness(options = {}) {
  const calls = [];
  const listeners = new Map();
  const panes = new Map();
  const focused = [];
  const windowListeners = new Map();
  let nextPane = 1;
  let pane = options.pane || null;
  const invoke = async (command, args = {}) => {
    calls.push({ command, args });
    if (typeof options.invoke === 'function') {
      const custom = await options.invoke(command, args, calls);
      if (custom !== undefined) return custom;
    }
    if (command === 'current_window_label') return 'main';
    if (command === 'editor_reserve_document') {
      return { kind: 'reserved', reservationId: `reservation-${calls.length}`, canonicalPath: args.path };
    }
    if (command === 'editor_read_file') return options.contents || 'let value = 1;';
    if (command === 'lsp_open_document') return response(args.path || '/repo/main.ts', `doc-${calls.length}`);
    if (command === 'lsp_apply_changes') return { kind: 'applied', version: args.batch.nextVersion };
    return null;
  };
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    Promise,
    Map,
    WeakMap,
    Date,
    CustomEvent: class CustomEvent { constructor(type, init) { this.type = type; this.detail = init && init.detail; } },
    dispatchEvent() {},
    addEventListener(type, handler) { windowListeners.set(type, handler); },
  };
  sandbox.window = sandbox;
  sandbox.CM6 = {};
  sandbox.toast = { error() {}, success() {}, info() {}, warn() {} };
  sandbox.termlabServices = {
    tauriClient: {
      invoke,
      listen(event, handler) {
        listeners.set(event, handler);
        return Promise.resolve(() => listeners.delete(event));
      },
      listenOnCurrentWindow(event, handler) {
        listeners.set(event, handler);
        return Promise.resolve(() => listeners.delete(event));
      },
      currentWindow: { setFocus: async () => { focused.push('window'); } },
    },
  };
  sandbox.__termlabPaneAccess = {
    currentPane: () => pane,
    allPanes: () => panes,
    activateTab: (id) => focused.push(`tab:${id}`),
    setFocusedPane: (id) => focused.push(`pane:${id}`),
    setTabLabel: () => true,
  };
  sandbox.termlabEditorPane = { setLanguage() {} };
  sandbox.termlabEditorTabLabel = {
    editorTabLabel(value) {
      return { label: path.basename(value.filePath || 'Untitled'), tooltip: value.filePath || 'Untitled' };
    },
  };
  sandbox.__termlabCreateEditorTab = (opts) => {
    const id = nextPane++;
    const doc = { value: opts.contents || '', toString() { return this.value; } };
    const created = {
      paneId: id,
      tabId: id + 100,
      kind: 'editor',
      filePath: opts.filePath || null,
      remote: opts.remote || null,
      dirty: false,
      view: {
        state: { doc },
        termlabResetDirty() { created.dirty = false; },
        focus() { focused.push(`view:${id}`); },
      },
    };
    panes.set(id, created);
    pane = created;
    if (typeof opts.onPaneCreated === 'function') opts.onPaneCreated(created);
    return created.tabId;
  };
  vm.createContext(sandbox);
  load(sandbox, 'features/editor/language-map.js');
  load(sandbox, 'features/editor/lsp-state.js');
  load(sandbox, 'features/editor/lsp-bridge.js');
  sandbox.termlabLspBridge.configure({ windowLabel: 'main', paneAccess: sandbox.__termlabPaneAccess });
  load(sandbox, 'features/editor/editor-service.js');
  sandbox.termlabLspBridge.configure({
    windowLabel: 'main',
    paneAccess: sandbox.__termlabPaneAccess,
    onReservationFailed: (canonicalPath) => sandbox.termlabEditorService.openLocalFile(canonicalPath),
  });
  if (pane) {
    panes.set(pane.paneId, pane);
  }
  return { sandbox, service: sandbox.termlabEditorService, state: sandbox.termlabLspState, calls, listeners, windowListeners, panes, focused, get pane() { return pane; } };
}

function ownershipCalls(calls) {
  return calls.filter((entry) => /^(?:lsp_|editor_(?:reserve|release|transfer)_document)/.test(entry.command));
}

await check('local open reserves before reading and commits only after pane construction', async () => {
  let constructed = false;
  const h = harness({
    invoke(command) {
      if (command === 'lsp_open_document') {
        assert.equal(constructed, true);
        const opened = response('/repo/main.ts', 'doc-committed-degraded');
        opened.status.state = 'failed';
        opened.status.message = 'Language features are unavailable; editing continues';
        return opened;
      }
    },
  });
  const original = h.sandbox.__termlabCreateEditorTab;
  h.sandbox.__termlabCreateEditorTab = (opts) => {
    const result = original(opts);
    constructed = true;
    return result;
  };
  await h.service.openLocalFile('/repo/main.ts');
  assert.deepEqual(h.calls.slice(0, 3).map((entry) => entry.command), [
    'editor_reserve_document', 'editor_read_file', 'lsp_open_document',
  ]);
  assert.equal(h.calls[2].args.languageId, 'typescript');
  assert.ok(h.state.get(h.pane).documentId);
  assert.equal(h.state.get(h.pane).status.state, 'failed');
  assert.equal(h.calls.some((entry) => entry.command === 'editor_release_document'), false);
});

await check('focus-owner skips disk reads and focuses the owning pane', async () => {
  const owner = { paneId: 8, tabId: 18, kind: 'editor', view: { focus() {} } };
  const h = harness({
    pane: owner,
    invoke(command) {
      if (command === 'editor_reserve_document') {
        return { kind: 'focusOwner', documentId: 'doc-owner', windowLabel: 'main', paneId: '8' };
      }
    },
  });
  await h.service.openLocalFile('/repo/main.ts');
  assert.equal(h.calls.some((entry) => entry.command === 'editor_read_file'), false);
  assert.deepEqual(h.focused.slice(-2), ['tab:18', 'pane:8']);
});

await check('typing while open ownership commits is reconciled with a full current-text resync', async () => {
  let resolveOpen;
  const h = harness({
    invoke(command) {
      if (command === 'lsp_open_document') {
        return new Promise((resolve) => { resolveOpen = resolve; });
      }
    },
  });
  const opening = h.service.openLocalFile('/repo/delayed.ts');
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.ok(h.pane, 'the real lifecycle constructs the pane before awaiting manager commit');
  h.pane.view.state.doc.value = 'let value = "💡";\nnext();';
  h.service.documentTransaction(h.pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(12, 13, 12, 14, { toString: () => '💡' }); } },
  });
  resolveOpen(response('/repo/delayed.ts', 'doc-delayed'));
  await opening;
  const resync = h.calls.find((entry) => entry.command === 'lsp_resync_document');
  assert.ok(resync, 'the edit made before attachment is not silently dropped');
  assert.equal(resync.args.contents, 'let value = "💡";\nnext();');
  assert.equal(resync.args.version, 2);
  assert.equal(h.state.get(h.pane).version, 2);
});

await check('read failure releases exactly the uncommitted reservation', async () => {
  const h = harness({
    invoke(command) {
      if (command === 'editor_reserve_document') {
        return { kind: 'reserved', reservationId: 'target-r', canonicalPath: '/repo/bad.ts' };
      }
      if (command === 'editor_read_file') throw new Error('denied');
    },
  });
  await h.service.openLocalFile('/repo/bad.ts');
  assert.deepEqual(ownershipCalls(h.calls).map((entry) => entry.command), [
    'editor_reserve_document', 'editor_release_document',
  ]);
  assert.equal(h.calls.find((entry) => entry.command === 'editor_release_document').args.reservationId, 'target-r');
});

await check('local close flushes and closes ownership before view destruction', async () => {
  const order = [];
  const pane = {
    paneId: 1, tabId: 2, kind: 'editor', filePath: '/repo/main.ts', remote: null, dirty: false,
    view: { state: { doc: { toString: () => 'x' } }, destroy: () => order.push('destroy') },
  };
  const h = harness({ pane, invoke(command) { if (command === 'lsp_close_document') order.push('close'); } });
  h.state.attach(pane, response('/repo/main.ts', 'doc-close'));
  await h.service.closeDocument(pane);
  pane.view.destroy();
  assert.deepEqual(order, ['close', 'destroy']);
  assert.equal(h.state.get(pane), null);
});

await check('local-to-local Save As reserves, writes, then transfers ownership', async () => {
  const pane = {
    paneId: 3, tabId: 4, kind: 'editor', filePath: '/repo/a.ts', remote: null, dirty: true,
    view: { state: { doc: { toString: () => 'changed' } }, termlabResetDirty() { pane.dirty = false; } },
  };
  const h = harness({ pane });
  h.state.attach(pane, response('/repo/a.ts', 'doc-source'));
  await h.service.saveAs(pane, { scope: 'local', path: '/repo/b.ts' });
  assert.deepEqual(h.calls.filter((entry) => [
    'editor_reserve_document', 'editor_write_file', 'editor_transfer_document',
  ].includes(entry.command)).map((entry) => entry.command), [
    'editor_reserve_document', 'editor_write_file', 'editor_transfer_document',
  ]);
  assert.equal(pane.filePath, '/repo/b.ts');
  assert.equal(h.state.get(pane).documentId, 'doc-source');
});

await check('failed local Save As releases only its target and preserves source ownership', async () => {
  const pane = {
    paneId: 3, tabId: 4, kind: 'editor', filePath: '/repo/a.ts', remote: null, dirty: true,
    view: { state: { doc: { toString: () => 'changed' } }, termlabResetDirty() {} },
  };
  const h = harness({ pane, invoke(command) { if (command === 'editor_write_file') throw new Error('disk full'); } });
  h.state.attach(pane, response('/repo/a.ts', 'doc-source'));
  await assert.rejects(h.service.saveAs(pane, { scope: 'local', path: '/repo/b.ts' }), /disk full/);
  assert.equal(pane.filePath, '/repo/a.ts');
  assert.equal(h.state.get(pane).documentId, 'doc-source');
  assert.equal(h.calls.filter((entry) => entry.command === 'editor_release_document').length, 1);
  assert.equal(h.calls.some((entry) => entry.command === 'lsp_close_document'), false);
});

await check('post-transfer UI cleanup cannot turn a committed Save As into failure', async () => {
  const pane = {
    paneId: 3, tabId: 4, kind: 'editor', filePath: '/repo/a.ts', remote: null, dirty: true,
    view: {
      state: { doc: { toString: () => 'changed' } },
      termlabResetDirty() { throw new Error('broken dirty indicator'); },
    },
  };
  const h = harness({ pane });
  h.state.attach(pane, response('/repo/a.ts', 'doc-source'));
  await h.service.saveAs(pane, { scope: 'local', path: '/repo/b.ts' });
  assert.equal(pane.filePath, '/repo/b.ts');
  assert.equal(h.calls.filter((entry) => entry.command === 'editor_transfer_document').length, 1);
  assert.equal(h.calls.some((entry) => entry.command === 'editor_release_document'), false);
});

for (const source of ['untitled', 'remote']) {
  await check(`${source}-to-local Save As reserves and commits the new local document`, async () => {
    const pane = {
      paneId: 5, tabId: 6, kind: 'editor', filePath: source === 'remote' ? '/tmp/r.ts' : null,
      remote: source === 'remote' ? { paneId: 9, remotePath: '/r.ts', hostLabel: 'host' } : null,
      dirty: true,
      view: { state: { doc: { toString: () => 'body' } }, termlabResetDirty() { pane.dirty = false; } },
    };
    const h = harness({ pane });
    await h.service.saveAs(pane, { scope: 'local', path: `/repo/${source}.ts` });
    assert.deepEqual(ownershipCalls(h.calls).map((entry) => entry.command), [
      'editor_reserve_document', 'lsp_open_document',
    ]);
    assert.ok(h.state.get(pane).documentId);
    assert.equal(pane.remote, null);
  });
}

await check('local-to-remote Save As closes local ownership after the remote write', async () => {
  const pane = {
    paneId: 7, tabId: 8, kind: 'editor', filePath: '/repo/a.ts', remote: null, dirty: true,
    view: { state: { doc: { toString: () => 'body' } }, termlabResetDirty() { pane.dirty = false; } },
  };
  const h = harness({
    pane,
    invoke(command, args) {
      if (command === 'editor_temp_path') return '/tmp/new.ts';
      if (command === 'transfer_upload') {
        setTimeout(() => h.listeners.get('transfer-progress')({ payload: { transfer_id: 'upload-1', status: 'completed' } }), 0);
        return 'upload-1';
      }
    },
  });
  h.state.attach(pane, response('/repo/a.ts', 'doc-local'));
  await h.service.saveAs(pane, { scope: 'remote', paneId: 2, remotePath: '/new.ts', hostLabel: 'host' });
  assert.equal(h.calls.some((entry) => entry.command === 'editor_reserve_document'), false);
  assert.equal(h.calls.filter((entry) => entry.command === 'lsp_close_document').length, 1);
  assert.equal(h.state.get(pane), null);
});

await check('remote open and remote-to-remote Save As never cross the ownership boundary', async () => {
  const pane = {
    paneId: 7, tabId: 8, kind: 'editor', filePath: '/tmp/old.ts',
    remote: { paneId: 1, remotePath: '/old.ts', hostLabel: 'old' }, dirty: true,
    view: { state: { doc: { toString: () => 'body' } }, termlabResetDirty() { pane.dirty = false; } },
  };
  const h = harness({
    pane,
    invoke(command) {
      if (command === 'editor_can_open') return null;
      if (command === 'editor_temp_path') return '/tmp/new.ts';
      if (command === 'transfer_upload') {
        setTimeout(() => h.listeners.get('transfer-progress')({ payload: { transfer_id: 'upload-1', status: 'completed' } }), 0);
        return 'upload-1';
      }
    },
  });
  await h.service.saveAs(pane, { scope: 'remote', paneId: 2, remotePath: '/new.ts', hostLabel: 'new' });
  assert.deepEqual(ownershipCalls(h.calls), []);
});

await check('CodeMirror edits flush descending within 40ms and resync on a version mismatch', async () => {
  const pane = {
    paneId: 9, tabId: 10, kind: 'editor', filePath: '/repo/a.ts', remote: null, dirty: true,
    view: { state: { doc: { toString: () => 'aXbcY' } } },
  };
  let mismatch = true;
  const h = harness({
    pane,
    invoke(command, args) {
      if (command === 'lsp_apply_changes' && mismatch) {
        mismatch = false;
        return { kind: 'resyncRequired', expectedVersion: 1, receivedBaseVersion: args.batch.baseVersion };
      }
      if (command === 'lsp_resync_document') return { documentId: 'doc-edit', version: args.version, status: response('', 'doc-edit').status };
    },
  });
  h.state.attach(pane, response('/repo/a.ts', 'doc-edit'));
  const changes = {
    iterChanges(fn) {
      fn(1, 1, 1, 2, { toString: () => 'X' });
      fn(3, 3, 4, 5, { toString: () => 'Y' });
    },
  };
  h.service.documentTransaction(pane, { docChanged: true, changes });
  await h.service.flushDocument(pane);
  const batch = h.calls.find((entry) => entry.command === 'lsp_apply_changes').args.batch;
  assert.deepEqual(JSON.parse(JSON.stringify(batch.changes)), [
    { fromUtf16: 3, toUtf16: 3, insertedText: 'Y' },
    { fromUtf16: 1, toUtf16: 1, insertedText: 'X' },
  ]);
  const resync = h.calls.find((entry) => entry.command === 'lsp_resync_document');
  assert.equal(resync.args.contents, 'aXbcY');
  assert.equal(resync.args.version, 2);
});

await check('version-mismatch full resync subsumes later queued edits instead of replaying them', async () => {
  const pane = {
    paneId: 10, tabId: 11, kind: 'editor', filePath: '/repo/mismatch.ts', remote: null, dirty: true,
    view: { state: { doc: { toString: () => 'aX\n💡' } } },
  };
  const h = harness({
    pane,
    invoke(command, args) {
      if (command === 'lsp_apply_changes') {
        return { kind: 'resyncRequired', expectedVersion: 4, receivedBaseVersion: args.batch.baseVersion };
      }
      if (command === 'lsp_resync_document') {
        return { documentId: 'doc-mismatch', version: args.version, status: response('', 'doc-mismatch').status };
      }
    },
  });
  h.state.attach(pane, response('/repo/mismatch.ts', 'doc-mismatch'));
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(1, 1, 1, 2, { toString: () => 'X' }); } },
  });
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(2, 2, 2, 5, { toString: () => '\n💡' }); } },
  });
  await h.service.flushDocument(pane);
  assert.equal(h.calls.filter((entry) => entry.command === 'lsp_apply_changes').length, 1);
  const resync = h.calls.find((entry) => entry.command === 'lsp_resync_document');
  assert.equal(resync.args.contents, 'aX\n💡');
  assert.equal(resync.args.version, 5);
});

await check('continuous composed edits keep the first 40ms deadline with UTF-16 multiline payloads', async () => {
  const pane = {
    paneId: 10, tabId: 12, kind: 'editor', filePath: '/repo/deadline.ts', remote: null, dirty: true,
    view: { state: { doc: { toString: () => 'a\n💡' } } },
  };
  const h = harness({ pane });
  h.state.attach(pane, response('/repo/deadline.ts', 'doc-deadline'));
  const combined = {
    iterChanges(fn) {
      fn(1, 1, 1, 2, { toString: () => '\n' });
      fn(2, 2, 2, 4, { toString: () => '💡' });
    },
  };
  const first = {
    compose() { return combined; },
    iterChanges(fn) { fn(1, 1, 1, 2, { toString: () => '\n' }); },
  };
  h.service.documentTransaction(pane, { docChanged: true, changes: first });
  await new Promise((resolve) => setTimeout(resolve, 30));
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(2, 2, 2, 4, { toString: () => '💡' }); } },
  });
  await new Promise((resolve) => setTimeout(resolve, 20));
  const apply = h.calls.find((entry) => entry.command === 'lsp_apply_changes');
  assert.ok(apply, 'the second edit did not reset the first edit’s 40ms timer');
  assert.deepEqual(JSON.parse(JSON.stringify(apply.args.batch.changes)), [
    { fromUtf16: 2, toUtf16: 2, insertedText: '💡' },
    { fromUtf16: 1, toUtf16: 1, insertedText: '\n' },
  ]);
});

await check('save waits for a timer-dispatched change flush already in flight', async () => {
  const pane = {
    paneId: 11, tabId: 12, kind: 'editor', filePath: '/repo/a.ts', remote: null, dirty: true,
    view: {
      state: { doc: { toString: () => 'xy' } },
      termlabResetDirty() { pane.dirty = false; },
    },
  };
  let releaseApply;
  const h = harness({
    pane,
    invoke(command, args) {
      if (command === 'lsp_apply_changes') {
        return new Promise((resolve) => { releaseApply = () => resolve({ kind: 'applied', version: args.batch.nextVersion }); });
      }
    },
  });
  h.state.attach(pane, response('/repo/a.ts', 'doc-race'));
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(1, 1, 1, 2, { toString: () => 'y' }); } },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  const saving = h.service.savePane(pane);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(h.calls.some((entry) => entry.command === 'editor_write_file'), false);
  releaseApply();
  await saving;
  assert.equal(h.calls.some((entry) => entry.command === 'editor_write_file'), true);
});

await check('save and feature barriers serialize a transaction admitted during an in-flight flush', async () => {
  const pane = {
    paneId: 13, tabId: 14, kind: 'editor', filePath: '/repo/barrier.ts', remote: null, dirty: true,
    view: {
      state: { doc: { value: 'abc', toString() { return this.value; } } },
      termlabResetDirty() { pane.dirty = false; },
    },
  };
  let releaseFirst;
  let applies = 0;
  const h = harness({
    pane,
    invoke(command, args) {
      if (command === 'lsp_apply_changes') {
        applies += 1;
        if (applies === 1) {
          return new Promise((resolve) => {
            releaseFirst = () => resolve({ kind: 'applied', version: args.batch.nextVersion });
          });
        }
        return { kind: 'applied', version: args.batch.nextVersion };
      }
      if (command === 'lsp_completion') return { documentId: 'doc-barrier', sourceVersion: 3, items: [] };
    },
  });
  h.state.attach(pane, response('/repo/barrier.ts', 'doc-barrier'));
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(1, 1, 1, 2, { toString: () => 'b' }); } },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  const saving = h.service.savePane(pane);
  pane.view.state.doc.value = 'abc\n💡';
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(3, 3, 3, 6, { toString: () => '\n💡' }); } },
  });
  releaseFirst();
  await saving;
  const ordered = h.calls.filter((entry) => ['lsp_apply_changes', 'editor_write_file'].includes(entry.command));
  assert.deepEqual(ordered.map((entry) => entry.command), [
    'lsp_apply_changes', 'lsp_apply_changes', 'editor_write_file',
  ]);

  pane.view.state.doc.value += '!';
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(6, 6, 6, 7, { toString: () => '!' }); } },
  });
  await h.service.requestFeature(pane, 'completion', { line: 1, character: 3 });
  const featureOrder = h.calls.filter((entry) => ['lsp_apply_changes', 'lsp_completion'].includes(entry.command));
  assert.equal(featureOrder.at(-2).command, 'lsp_apply_changes');
  assert.equal(featureOrder.at(-1).command, 'lsp_completion');
});

await check('close gates editing and aborts destruction when a transaction arrives during its drain', async () => {
  const readOnly = [];
  const pane = {
    paneId: 14, tabId: 15, kind: 'editor', filePath: '/repo/closing.ts', remote: null, dirty: false,
    view: {
      state: { doc: { value: 'a', toString() { return this.value; } } },
      termlabSetReadOnly(value) { readOnly.push(value); },
    },
  };
  let releaseApply;
  let applyCount = 0;
  const h = harness({
    pane,
    invoke(command, args) {
      if (command === 'lsp_apply_changes') {
        applyCount += 1;
        if (applyCount > 1) return { kind: 'applied', version: args.batch.nextVersion };
        return new Promise((resolve) => {
          releaseApply = () => resolve({ kind: 'applied', version: args.batch.nextVersion });
        });
      }
    },
  });
  h.state.attach(pane, response('/repo/closing.ts', 'doc-closing'));
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(0, 1, 0, 1, { toString: () => 'b' }); } },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  const closing = h.service.closeDocument(pane);
  pane.view.state.doc.value = 'bc';
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(1, 1, 1, 2, { toString: () => 'c' }); } },
  });
  releaseApply();
  assert.equal(await closing, false);
  assert.deepEqual(readOnly, [true, false]);
  assert.equal(h.calls.some((entry) => entry.command === 'lsp_close_document'), false);
  assert.equal(h.state.get(pane).documentId, 'doc-closing');
});

await check('a rejected incremental change forces full-text resync before the next feature', async () => {
  const pane = {
    paneId: 15, tabId: 16, kind: 'editor', filePath: '/repo/desync.ts', remote: null, dirty: true,
    view: { state: { doc: { value: 'a', toString() { return this.value; } } } },
  };
  let reject = true;
  const h = harness({
    pane,
    invoke(command, args) {
      if (command === 'lsp_apply_changes' && reject) {
        reject = false;
        throw new Error('delivery uncertain');
      }
      if (command === 'lsp_resync_document') return { documentId: 'doc-desync', version: args.version, status: response('', 'doc-desync').status };
      if (command === 'lsp_hover') return { documentId: 'doc-desync', sourceVersion: args.version, blocks: [] };
    },
  });
  h.state.attach(pane, response('/repo/desync.ts', 'doc-desync'));
  pane.view.state.doc.value = 'é';
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(0, 1, 0, 2, { toString: () => 'é' }); } },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  pane.view.state.doc.value = 'é\n💡';
  h.service.documentTransaction(pane, {
    docChanged: true,
    changes: { iterChanges(fn) { fn(1, 1, 1, 4, { toString: () => '\n💡' }); } },
  });
  await h.service.requestFeature(pane, 'hover', { line: 1, character: 2 });
  const afterRejection = h.calls.filter((entry) => ['lsp_resync_document', 'lsp_hover'].includes(entry.command));
  assert.deepEqual(afterRejection.map((entry) => entry.command), ['lsp_resync_document', 'lsp_hover']);
  assert.equal(afterRejection[0].args.contents, 'é\n💡');
  assert.equal(afterRejection[0].args.version, 2);
});

await check('cross-project local Save As replaces project metadata without changing document identity', async () => {
  const pane = {
    paneId: 17, tabId: 18, kind: 'editor', filePath: '/project-a/a.ts', remote: null, dirty: true,
    view: { state: { doc: { toString: () => 'body' } }, termlabResetDirty() { pane.dirty = false; } },
  };
  const candidate = {
    rootUri: 'file:///project-b', canonicalPath: '/project-b', displayName: 'project-b',
    marker: 'tsconfig.json', reason: 'tsconfig.json identifies a TypeScript project', confidence: 100, isFallback: false,
  };
  const h = harness({
    pane,
    invoke(command) {
      if (command === 'editor_transfer_document') {
        const next = response('/project-b/b.ts', 'doc-same');
        next.version = 4;
        next.projectCandidates = [candidate];
        next.status.projectRootUri = 'file:///project-b';
        next.status.state = 'failed';
        next.status.message = 'Language features are unavailable; editing continues';
        return next;
      }
    },
  });
  h.state.attach(pane, response('/project-a/a.ts', 'doc-same'));
  await h.service.saveAs(pane, { scope: 'local', path: '/project-b/b.ts' });
  const state = h.state.get(pane);
  assert.equal(state.documentId, 'doc-same');
  assert.equal(state.version, 4);
  assert.equal(state.projectCandidates[0].canonicalPath, '/project-b');
  assert.equal(state.selectedRoot, '/project-b');
  assert.equal(state.status.state, 'failed');
});

await check('pending duplicate-open failure retries the exact canonical path instead of disappearing', async () => {
  let reservations = 0;
  const h = harness({
    invoke(command, args) {
      if (command === 'editor_reserve_document') {
        reservations += 1;
        if (reservations === 1) return { kind: 'focusPending', windowLabel: 'owner' };
        return { kind: 'reserved', reservationId: 'retry-reservation', canonicalPath: args.path };
      }
    },
  });
  await h.service.openLocalFile('/repo/pending.ts');
  assert.equal(h.calls.some((entry) => entry.command === 'editor_read_file'), false);
  h.listeners.get('editor-document-owner-focused')({
    payload: { documentId: null, paneId: null, canonicalPath: '/repo/pending.ts', reservationFailed: true },
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(reservations, 2);
  assert.equal(h.calls.filter((entry) => entry.command === 'editor_read_file').length, 1);
  assert.equal(h.calls.filter((entry) => entry.command === 'lsp_open_document').length, 1);
});

await check('lsp-bridge is the only production frontend invoker of ownership/LSP commands', async () => {
  const bridge = path.join(APP, 'features/editor/lsp-bridge.js');
  const offenders = [];
  const walk = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const filename = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(filename);
      else if (entry.name.endsWith('.js') && filename !== bridge) {
        const source = fs.readFileSync(filename, 'utf8');
        if (/invoke\s*\(\s*['"](?:lsp_|editor_(?:reserve|release|transfer)_document)/.test(source)) {
          offenders.push(path.relative(FRONTEND, filename));
        }
      }
    }
  };
  walk(APP);
  assert.deepEqual(offenders, []);
});

await check('bridge disposal unsubscribes listeners even when registration resolves later', async () => {
  const h = harness();
  assert.equal(typeof h.windowListeners.get('unload'), 'function');
  h.windowListeners.get('unload')();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(h.listeners.size, 0);
});

console.log(`\n${ran - failures}/${ran} LSP editor lifecycle checks passed`);
if (failures) process.exit(1);
