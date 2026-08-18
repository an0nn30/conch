// Run: node scripts/tests/test_editor_save_as.mjs
//
// Save As — the one operation that changes a pane's IDENTITY.
//
// `pane.filePath` and `pane.remote` are what the dirty guards,
// `focusExistingEditor`, `opensInFlight` and the temp cleanup all key on. A
// rebind that lands halfway — new filePath but old `remote`, or a cleared
// dirty flag over bytes that never reached the host — is this feature's
// data-loss shape. So the ordering, not just the outcome, is what is pinned
// here:
//
//   write to the NEW location first  ->  upload  ->  only then mutate the
//   pane, in ONE synchronous block  ->  only then clean the OLD temp.
//
// The fixture makes the old and new bindings differ in EVERY field — host,
// pane id, directory, basename and language — so a partial rebind cannot pass
// by looking like the other half.
//
// Stub shapes are the real backend's (same list as
// test_editor_remote_transfer.mjs, whose transfer stub layer this copies):
//   editor_temp_path     Result<String, String> -> an absolute path,
//                        deterministic per (hostLabel, remotePath)
//   editor_write_file    Result<(), String>     -> resolves null
//   editor_temp_cleanup  Result<(), String>     -> resolves null
//   transfer_upload      Result<String, String> -> a transfer id; the outcome
//                        arrives on the shared 'transfer-progress' event
//   local_stat/sftp_stat Result<FileEntry, String> -> a FileEntry, or a bare
//                        String rejection when the path does not exist
//   local_mkdir/sftp_mkdir Result<(), String>   -> resolves null
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');
const SERVICE_PATH = path.join(APP, 'features/editor/editor-service.js');

let failures = 0;
let ran = 0;
function check(name, fn) {
  ran++;
  try {
    fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}
async function checkAsync(name, fn) {
  ran++;
  try {
    await fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = async (n = 6) => { for (let i = 0; i < n; i++) await tick(); };

// Every await on a service promise goes through this: the failure these tests
// exist to catch includes a promise that never settles, and an unguarded await
// on one wedges node with no message instead of failing.
const TIMEOUT_MS = 2000;
function settles(promise, what) {
  let timer = null;
  const bomb = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`timed out after ${TIMEOUT_MS}ms waiting for ${what}`)), TIMEOUT_MS);
  });
  return Promise.race([promise, bomb]).finally(() => { if (timer) clearTimeout(timer); });
}

// ---------------------------------------------------------------------------
// THE FIXTURE — old and new differ in every field
// ---------------------------------------------------------------------------

const OLD = { paneId: 4, remotePath: '/srv/legacy/alpha.conf', hostLabel: 'ada@alpha' };
const NEW = { paneId: 9, remotePath: '/opt/fresh/beta.py', hostLabel: 'bob@beta' };
const LOCAL_TARGET = '/home/dev/notes/gamma.md';

// Same stand-in for the real EditorView as test_editor_save_race.mjs: `dirty`
// is a plain boolean and the update listener stops firing once it is set.
function makeView(initialDoc, onDirtyChange) {
  let doc = initialDoc;
  let dirty = false;
  return {
    get state() { return { doc: { toString: () => doc } }; },
    termlabResetDirty() { dirty = false; onDirtyChange(false); },
    change(nextDoc) {
      if (nextDoc === doc) return;
      doc = nextDoc;
      if (dirty) return;
      dirty = true;
      onDirtyChange(true);
    },
    type(text) { this.change(doc + text); },
  };
}

// Stands in for editor_temp_path's real (host hash)/(path hash)/basename
// layout; only the property the service depends on is reproduced — one path
// per (hostLabel, remotePath) pair, never shared across pairs.
const tempPathFor = (hostLabel, remotePath) => {
  const base = String(remotePath).split('/').filter(Boolean).pop() || 'untitled';
  const key = (s) => String(s).replace(/[^a-zA-Z0-9]/g, '_');
  return `/tmp/termlab-sftp-edits/${key(hostLabel)}/${key(remotePath)}/${base}`;
};

function load(sandbox, relPath) {
  vm.runInContext(fs.readFileSync(path.join(APP, relPath), 'utf8'), sandbox, { filename: relPath });
}

function makeHarness(options = {}) {
  const sandbox = { console, setTimeout, clearTimeout };
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const calls = [];
  const toasts = [];
  const listeners = [];
  const started = [];
  const writes = [];
  const cleanups = [];
  const labelCalls = [];
  const languageCalls = [];
  const panes = new Map();
  const tabs = new Map();
  let nextTransferId = 1;

  sandbox.CM6 = {};
  sandbox.toast = {
    error: (title, body) => { toasts.push({ kind: 'error', title, body }); },
    success: (title, body) => { toasts.push({ kind: 'success', title, body }); },
    warn: (title, body) => { toasts.push({ kind: 'warn', title, body }); },
    info: (title, body) => { toasts.push({ kind: 'info', title, body }); },
  };

  // The single pane under test, built directly rather than through an open:
  // the fixture has to be exact.
  const spec = options.pane || {};
  const pane = {
    paneId: 1,
    tabId: 11,
    kind: 'editor',
    filePath: spec.filePath || null,
    remote: spec.remote || null,
    dirty: false,
  };
  pane.view = makeView(typeof spec.doc === 'string' ? spec.doc : 'body text', (d) => { pane.dirty = d; });
  panes.set(pane.paneId, pane);
  tabs.set(pane.tabId, { label: spec.tabLabel || '', tooltip: spec.tabTooltip || '' });

  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        calls.push({ command, args });
        switch (command) {
          case 'editor_temp_path':
            return Promise.resolve(tempPathFor(args.hostLabel, args.remotePath));
          case 'editor_write_file': {
            const entry = { path: args.path, contents: args.contents };
            writes.push(entry);
            if (!options.parkWrites) {
              return options.failWrite
                ? Promise.reject(options.failWrite)
                : Promise.resolve(null);
            }
            return new Promise((resolve, reject) => {
              entry.resolve = () => resolve(null);
              entry.reject = (e) => reject(e);
            });
          }
          case 'editor_temp_cleanup':
            // Snapshot the pane AT THE MOMENT of the call: "clean the old temp
            // only AFTER a successful rebind" is an ordering claim, and this
            // is the only way to see the ordering rather than the end state.
            cleanups.push({
              path: args.path,
              paneFilePathAtCall: pane.filePath,
              paneRemoteAtCall: pane.remote ? { ...pane.remote } : null,
            });
            return Promise.resolve(null);
          case 'transfer_upload': {
            const id = `transfer-${nextTransferId++}`;
            if (options.failUploadStart) return Promise.reject(options.failUploadStart);
            started.push({ id, command, args });
            return Promise.resolve(id);
          }
          case 'transfer_cancel':
            return Promise.resolve(true);
          default:
            return Promise.resolve(null);
        }
      },
      listen(eventName, handler) {
        const entry = { eventName, handler, active: true };
        listeners.push(entry);
        return Promise.resolve(() => { entry.active = false; });
      },
    },
  };

  sandbox.__termlabPaneAccess = {
    currentPane: () => pane,
    allPanes: () => panes,
    setFocusedPane: () => {},
    activateTab: () => {},
    // The real one (manager-compose-runtime.js) writes the tab button's label
    // span and its title, and returns whether the tab was found.
    setTabLabel: (tabId, label, tooltip) => {
      labelCalls.push({ tabId, label, tooltip });
      const tab = tabs.get(tabId);
      if (!tab) return false;
      tab.label = label;
      tab.tooltip = tooltip;
      return true;
    },
  };

  // Real module: the label composition is the thing being applied.
  load(sandbox, 'features/editor/tab-label.js');

  // editor-pane needs CodeMirror and a DOM; only the one entry point Save As
  // calls is stubbed, and it returns what the real one returns (undefined).
  sandbox.termlabEditorPane = {
    setLanguage: (view, filename) => { languageCalls.push({ isPaneView: view === pane.view, filename }); },
  };

  vm.runInContext(fs.readFileSync(SERVICE_PATH, 'utf8'), sandbox, { filename: SERVICE_PATH });

  // Seed the tab label from the pane's starting identity, exactly as
  // createEditorTab does.
  const seed = sandbox.termlabEditorTabLabel.editorTabLabel(pane);
  tabs.set(pane.tabId, { label: seed.label, tooltip: seed.tooltip });

  return {
    sandbox,
    service: sandbox.termlabEditorService,
    pane,
    tab: () => tabs.get(pane.tabId),
    calls,
    writes,
    cleanups,
    toasts,
    started,
    labelCalls,
    languageCalls,
    commandsNamed: (name) => calls.filter((c) => c.command === name),
    emit: (progress) => {
      for (const entry of listeners) {
        if (entry.active && entry.eventName === 'transfer-progress') entry.handler({ payload: progress });
      }
    },
    activeListeners: () => listeners.filter((l) => l.active).length,
  };
}

const progress = (id, status, extra = {}) => ({
  transfer_id: id,
  kind: 'upload',
  status,
  bytes_transferred: 0,
  total_bytes: 0,
  file_name: 'beta.py',
  ...extra,
});

const remotePane = () => ({
  filePath: tempPathFor(OLD.hostLabel, OLD.remotePath),
  remote: { ...OLD },
  doc: 'alpha body',
});

const localPane = () => ({ filePath: '/tmp/termlab-scratch/scratch-1.txt', doc: 'scratch body' });

// Every identity field of the OLD binding, asserted as one lump. Used by the
// failure cases, where "unchanged" has to mean all of it.
function assertOldBinding(h, why) {
  assert.strictEqual(h.pane.filePath, tempPathFor(OLD.hostLabel, OLD.remotePath), `${why}: filePath`);
  assert.deepEqual(h.pane.remote, { ...OLD }, `${why}: remote`);
  assert.strictEqual(h.pane.remote.paneId, OLD.paneId, `${why}: remote.paneId`);
  assert.strictEqual(h.pane.remote.remotePath, OLD.remotePath, `${why}: remote.remotePath`);
  assert.strictEqual(h.pane.remote.hostLabel, OLD.hostLabel, `${why}: remote.hostLabel`);
  assert.strictEqual(h.tab().label, 'alpha.conf — ada@alpha', `${why}: tab label`);
  assert.strictEqual(h.tab().tooltip, 'ada@alpha:/srv/legacy/alpha.conf', `${why}: tab tooltip`);
  assert.strictEqual(h.pane.dirty, true, `${why}: still dirty`);
}

// ---------------------------------------------------------------------------
// (a) Local target — write, rebind, clear remote, refresh label, clear dirty
// ---------------------------------------------------------------------------

console.log('editor save as: local target');

await checkAsync('(a) a local scratch saved to a new local path rebinds completely', async () => {
  const h = makeHarness({ pane: localPane() });
  h.pane.view.type('!');
  assert.strictEqual(h.pane.dirty, true, 'precondition: the pane is dirty');

  await settles(h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET }), 'a local Save As');

  assert.strictEqual(h.writes.length, 1, 'exactly one write');
  assert.strictEqual(h.writes[0].path, LOCAL_TARGET, 'written to the NEW path');
  assert.strictEqual(h.writes[0].contents, 'scratch body!', 'with the buffer contents');
  assert.strictEqual(h.pane.filePath, LOCAL_TARGET, 'filePath rebound');
  assert.strictEqual(h.pane.remote, null, 'remote cleared');
  assert.strictEqual(h.tab().label, 'gamma.md', 'tab label is the new basename');
  assert.strictEqual(h.tab().tooltip, LOCAL_TARGET, 'tooltip is the new absolute path');
  assert.strictEqual(h.pane.dirty, false, 'dirty cleared');
  assert.strictEqual(h.commandsNamed('transfer_upload').length, 0, 'no upload for a local target');
  assert.strictEqual(h.cleanups.length, 0, 'a local pane owns no temp file, so nothing is deleted');
});

await checkAsync('(a) the label goes through editorTabLabel, not a private basename', async () => {
  const h = makeHarness({ pane: remotePane() });
  h.pane.view.type('!');
  await settles(h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET }), 'remote -> local Save As');

  assert.strictEqual(h.labelCalls.length, 1, 'the tab label is refreshed exactly once');
  const expected = h.sandbox.termlabEditorTabLabel.editorTabLabel(h.pane);
  assert.strictEqual(h.labelCalls[0].tabId, h.pane.tabId);
  assert.strictEqual(h.labelCalls[0].label, expected.label);
  assert.strictEqual(h.labelCalls[0].tooltip, expected.tooltip);
  // Crossing from remote to local must drop the host half of the label.
  assert.strictEqual(h.labelCalls[0].label, 'gamma.md', 'no host suffix once the pane is local');
});

await checkAsync('(a) leaving a remote binding cleans the old temp — after the rebind', async () => {
  const h = makeHarness({ pane: remotePane() });
  h.pane.view.type('!');
  await settles(h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET }), 'remote -> local Save As');

  assert.strictEqual(h.cleanups.length, 1, 'the abandoned temp file is removed');
  assert.strictEqual(h.cleanups[0].path, tempPathFor(OLD.hostLabel, OLD.remotePath), 'the OLD temp path');
  assert.strictEqual(h.cleanups[0].paneFilePathAtCall, LOCAL_TARGET,
    'and only once the pane had already rebound — never before');
  assert.strictEqual(h.cleanups[0].paneRemoteAtCall, null, 'the pane was already local when it ran');
});

// ---------------------------------------------------------------------------
// (c) Remote target — temp path, write, upload, then rebind
// ---------------------------------------------------------------------------

console.log('editor save as: remote target');

await checkAsync('(c) a remote Save As rebinds every field only after the upload lands', async () => {
  const h = makeHarness({ pane: remotePane() });
  h.pane.view.type('!');

  const saving = h.service.saveAs(h.pane, { scope: 'remote', ...NEW });
  await settle();

  // Mid-flight: the bytes are staged but not delivered, so the pane must
  // still be the OLD file in every respect.
  assert.strictEqual(h.writes.length, 1, 'the new temp file is written first');
  assert.strictEqual(h.writes[0].path, tempPathFor(NEW.hostLabel, NEW.remotePath), 'at the NEW temp path');
  assert.strictEqual(h.writes[0].contents, 'alpha body!');
  assertOldBinding(h, 'while the upload is in flight');
  assert.strictEqual(h.labelCalls.length, 0, 'the tab is not relabelled ahead of the upload');

  const upload = h.started[h.started.length - 1];
  assert.strictEqual(upload.command, 'transfer_upload');
  assert.deepEqual(upload.args, {
    paneId: NEW.paneId,
    localPath: tempPathFor(NEW.hostLabel, NEW.remotePath),
    remotePath: NEW.remotePath,
  }, 'the upload goes to the NEW pane id and path');

  h.emit(progress(upload.id, 'completed'));
  await settles(saving, 'a remote Save As');

  assert.strictEqual(h.pane.filePath, tempPathFor(NEW.hostLabel, NEW.remotePath), 'filePath is the new temp');
  assert.deepEqual(h.pane.remote, { paneId: NEW.paneId, remotePath: NEW.remotePath, hostLabel: NEW.hostLabel },
    'remote rebound to the new host, pane id and path');
  assert.strictEqual(h.tab().label, 'beta.py — bob@beta', 'the tab names the new file and host');
  assert.strictEqual(h.tab().tooltip, 'bob@beta:/opt/fresh/beta.py');
  assert.strictEqual(h.pane.dirty, false, 'clean once the bytes reached the host');
  assert.strictEqual(h.activeListeners(), 0, 'the progress listener was removed');
});

await checkAsync('(c) the OLD temp is cleaned exactly once, after the rebind', async () => {
  const h = makeHarness({ pane: remotePane() });
  h.pane.view.type('!');
  const saving = h.service.saveAs(h.pane, { scope: 'remote', ...NEW });
  await settle();
  h.emit(progress(h.started[0].id, 'completed'));
  await settles(saving, 'a remote Save As');

  assert.strictEqual(h.cleanups.length, 1, 'one cleanup');
  assert.strictEqual(h.cleanups[0].path, tempPathFor(OLD.hostLabel, OLD.remotePath), 'of the OLD temp path');
  assert.notStrictEqual(h.cleanups[0].path, h.pane.filePath, 'never the file the pane now lives in');
  assert.strictEqual(h.cleanups[0].paneFilePathAtCall, tempPathFor(NEW.hostLabel, NEW.remotePath),
    'the pane had already rebound when the cleanup ran');
});

await checkAsync('(c) a local pane saved to a host owns no temp, so nothing is deleted', async () => {
  const h = makeHarness({ pane: localPane() });
  h.pane.view.type('!');
  const saving = h.service.saveAs(h.pane, { scope: 'remote', ...NEW });
  await settle();
  h.emit(progress(h.started[0].id, 'completed'));
  await settles(saving, 'a scratch saved to a host');

  assert.strictEqual(h.cleanups.length, 0, 'the scratch file is the user’s, not a temp — left alone');
  assert.strictEqual(h.pane.filePath, tempPathFor(NEW.hostLabel, NEW.remotePath));
  assert.deepEqual(h.pane.remote, { paneId: NEW.paneId, remotePath: NEW.remotePath, hostLabel: NEW.hostLabel });
  assert.strictEqual(h.tab().label, 'beta.py — bob@beta');
  assert.strictEqual(h.pane.dirty, false);
});

// ---------------------------------------------------------------------------
// (d) THE ONE THAT MATTERS — a failed upload never half-rebinds
// ---------------------------------------------------------------------------

console.log('editor save as: a failed upload leaves the OLD binding');

await checkAsync('(d) upload fails: filePath, remote, label and dirty all stay OLD', async () => {
  const h = makeHarness({ pane: remotePane() });
  h.pane.view.type('!');

  const saving = h.service.saveAs(h.pane, { scope: 'remote', ...NEW });
  await settle();
  const upload = h.started[h.started.length - 1];
  h.emit(progress(upload.id, 'failed', { error: 'connection lost' }));

  await assert.rejects(() => settles(saving, 'a failed remote Save As'), /connection lost/,
    'the Save As rejects rather than resolving');

  assertOldBinding(h, 'after a failed upload');
  assert.strictEqual(h.labelCalls.length, 0, 'the tab was never relabelled');
  assert.strictEqual(h.languageCalls.length, 0, 'the language was never re-derived');
  assert.strictEqual(h.cleanups.length, 0,
    'nothing is deleted: the old temp still holds the pane’s only saved copy');
  const failures = h.toasts.filter((t) => t.kind === 'error');
  assert.strictEqual(failures.length, 1, 'the user is told');
  assert.match(failures[0].body, /connection lost/);
});

await checkAsync('(d) after a failed Save As, a plain save still goes to the OLD binding', async () => {
  const h = makeHarness({ pane: remotePane() });
  h.pane.view.type('!');
  const saving = h.service.saveAs(h.pane, { scope: 'remote', ...NEW });
  await settle();
  h.emit(progress(h.started[0].id, 'failed', { error: 'connection lost' }));
  await assert.rejects(() => settles(saving, 'a failed remote Save As'), /connection lost/);

  // The identity proof: ⌘S now must write and upload the OLD file. A pane
  // that had half-rebound would send the user's edit to the new host, or
  // write the new temp and upload it to the old path.
  const savingAgain = h.service.savePane(h.pane);
  await settle();
  const write = h.writes[h.writes.length - 1];
  assert.strictEqual(write.path, tempPathFor(OLD.hostLabel, OLD.remotePath), 'writes the OLD temp');
  const upload = h.started[h.started.length - 1];
  assert.deepEqual(upload.args, {
    paneId: OLD.paneId,
    localPath: tempPathFor(OLD.hostLabel, OLD.remotePath),
    remotePath: OLD.remotePath,
  }, 'and uploads to the OLD host and path');
  h.emit(progress(upload.id, 'completed'));
  await settles(savingAgain, 'the plain save after a failed Save As');
  assert.strictEqual(h.pane.dirty, false);
});

await checkAsync('(d) a write that fails never reaches the upload, and never rebinds', async () => {
  const h = makeHarness({ pane: remotePane(), failWrite: 'Permission denied (os error 13)' });
  h.pane.view.type('!');

  await assert.rejects(
    () => settles(h.service.saveAs(h.pane, { scope: 'remote', ...NEW }), 'a Save As whose write failed'),
    /Permission denied/,
  );
  assert.strictEqual(h.commandsNamed('transfer_upload').length, 0, 'nothing was uploaded');
  assertOldBinding(h, 'after a failed write');
  assert.strictEqual(h.labelCalls.length, 0);
  assert.strictEqual(h.cleanups.length, 0);
});

await checkAsync('(d) a local Save As whose write fails leaves the pane where it was', async () => {
  const h = makeHarness({ pane: localPane(), failWrite: 'Read-only file system (os error 30)' });
  h.pane.view.type('!');

  await assert.rejects(
    () => settles(h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET }), 'a failed local Save As'),
    /Read-only/,
  );
  assert.strictEqual(h.pane.filePath, '/tmp/termlab-scratch/scratch-1.txt', 'filePath unchanged');
  assert.strictEqual(h.pane.remote, null);
  assert.strictEqual(h.pane.dirty, true, 'and still dirty');
  assert.strictEqual(h.labelCalls.length, 0);
});

// ---------------------------------------------------------------------------
// (e) The in-flight guard — the existing one, not a new queue
// ---------------------------------------------------------------------------

console.log('editor save as: the in-flight guard');

await checkAsync('(e) a Save As waits for the save already in flight before writing', async () => {
  const h = makeHarness({ pane: localPane(), parkWrites: true });
  h.pane.view.type('!');

  const saving = h.service.savePane(h.pane);
  await settle();
  assert.strictEqual(h.writes.length, 1, 'the plain save issued its write');
  assert.strictEqual(h.writes[0].path, '/tmp/termlab-scratch/scratch-1.txt');

  const savingAs = h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET });
  await settle();
  assert.strictEqual(h.writes.length, 1, 'Save As starts no overlapping write');
  assert.strictEqual(h.pane.filePath, '/tmp/termlab-scratch/scratch-1.txt', 'and rebinds nothing yet');

  h.writes[0].resolve();
  await settles(saving, 'the plain save');
  await settle();

  assert.strictEqual(h.writes.length, 2, 'once it is free, Save As writes');
  assert.strictEqual(h.writes[1].path, LOCAL_TARGET, 'to the new path');
  h.writes[1].resolve();
  await settles(savingAs, 'the queued Save As');
  assert.strictEqual(h.pane.filePath, LOCAL_TARGET);
  assert.strictEqual(h.pane.dirty, false);
});

await checkAsync('(e) a plain save during a Save As joins it instead of overlapping', async () => {
  const h = makeHarness({ pane: localPane(), parkWrites: true });
  h.pane.view.type('!');

  const savingAs = h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET });
  await settle();
  assert.strictEqual(h.writes.length, 1, 'the Save As write is out');

  const saving = h.service.savePane(h.pane);
  await settle();
  assert.strictEqual(h.writes.length, 1, 'the plain save joined it — no second write');

  h.writes[0].resolve();
  await settles(savingAs, 'the Save As');
  await settles(saving, 'the joined plain save');
  assert.strictEqual(h.writes.length, 1, 'still one write for both calls');
  assert.strictEqual(h.pane.filePath, LOCAL_TARGET, 'and the rebind happened once');
  assert.strictEqual(h.pane.dirty, false);
});

await checkAsync('(e) the guard is released, so a later Save As is not blocked', async () => {
  const h = makeHarness({ pane: localPane() });
  h.pane.view.type('!');
  await settles(h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET }), 'the first Save As');
  h.pane.view.type('?');
  await settles(h.service.saveAs(h.pane, { scope: 'local', path: '/home/dev/notes/delta.txt' }), 'a second Save As');
  assert.strictEqual(h.pane.filePath, '/home/dev/notes/delta.txt');
  assert.strictEqual(h.writes.length, 2);
});

// ---------------------------------------------------------------------------
// (f) The language mode follows the new name
// ---------------------------------------------------------------------------

console.log('editor save as: language mode');

await checkAsync('(f) the language is re-derived from the NEW basename', async () => {
  const h = makeHarness({ pane: remotePane() });
  h.pane.view.type('!');
  const saving = h.service.saveAs(h.pane, { scope: 'remote', ...NEW });
  await settle();
  h.emit(progress(h.started[0].id, 'completed'));
  await settles(saving, 'a remote Save As');

  assert.strictEqual(h.languageCalls.length, 1, 'setLanguage ran once');
  assert.strictEqual(h.languageCalls[0].isPaneView, true, 'on this pane’s view');
  assert.strictEqual(h.languageCalls[0].filename, 'beta.py',
    'with the remote basename — not the temp path, and not the old .conf name');
});

await checkAsync('(f) a local target re-derives from its basename too', async () => {
  const h = makeHarness({ pane: localPane() });
  h.pane.view.type('!');
  await settles(h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET }), 'a local Save As');
  assert.strictEqual(h.languageCalls.length, 1);
  assert.strictEqual(h.languageCalls[0].filename, 'gamma.md');
});

// ---------------------------------------------------------------------------
// Editing during the save — the race the existing suites pin, for this path
// ---------------------------------------------------------------------------

await checkAsync('a keystroke during a Save As leaves the rebound pane honestly dirty', async () => {
  const h = makeHarness({ pane: localPane(), parkWrites: true });
  h.pane.view.type('!');
  const savingAs = h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET });
  await settle();
  h.pane.view.type('?');       // races the write
  h.writes[0].resolve();
  await settles(savingAs, 'a Save As raced by a keystroke');

  assert.strictEqual(h.pane.filePath, LOCAL_TARGET, 'the rebind still happened');
  assert.strictEqual(h.pane.dirty, true, 'but the un-written keystroke keeps the pane dirty');
});

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

await checkAsync('a non-editor pane and a malformed target are refused, not half-applied', async () => {
  const h = makeHarness({ pane: localPane() });
  await settles(h.service.saveAs({ kind: 'terminal' }, { scope: 'local', path: LOCAL_TARGET }), 'a terminal pane');
  assert.strictEqual(h.writes.length, 0, 'a terminal pane writes nothing');

  await assert.rejects(
    () => settles(h.service.saveAs(h.pane, { scope: 'sideways', path: '/x' }), 'an unknown scope'),
    /scope/i,
  );
  assert.strictEqual(h.writes.length, 0, 'and an unknown scope writes nothing');
  assert.strictEqual(h.pane.filePath, '/tmp/termlab-scratch/scratch-1.txt');
});

await checkAsync('Save As onto a path another editor already holds is refused', async () => {
  // Two editors on one path is exactly what focusExistingEditor exists to
  // prevent: each holds its own doc and the last save silently wins. Save As
  // is the one path that could create it after the fact.
  const h = makeHarness({ pane: localPane() });
  const other = {
    paneId: 2, tabId: 12, kind: 'editor', filePath: LOCAL_TARGET, remote: null, dirty: false,
  };
  other.view = makeView('other body', () => {});
  h.sandbox.__termlabPaneAccess.allPanes().set(2, other);
  h.pane.view.type('!');

  await assert.rejects(
    () => settles(h.service.saveAs(h.pane, { scope: 'local', path: LOCAL_TARGET }), 'a collision'),
    /open in another tab/i,
  );
  assert.strictEqual(h.writes.length, 0, 'nothing was written over it');
  assert.strictEqual(h.pane.filePath, '/tmp/termlab-scratch/scratch-1.txt', 'and nothing rebound');
});

// ---------------------------------------------------------------------------
// The rebind block itself — a SOURCE-TEXT CANARY
// ---------------------------------------------------------------------------
//
// A canary, not a proof — the same standing as the host-formula canary in
// test_file_dialog.mjs. Every behavioural check above probes the ordering
// BETWEEN the steps (write, then upload, then rebind, then cleanup). None of
// them can see an await inserted INSIDE the rebind, because at every await
// point the checks reach, the pane is already fully rebound.
//
// That gap matters: `await Promise.resolve();` between `pane.filePath =` and
// `pane.remote =` leaves the whole suite green while a savePane joiner waking
// in that window sees filePath = NEW and remote = OLD — and writes the new
// local file, then uploads it to the OLD host. So the block is read as text
// and required to contain no await at all.
//
// What it cannot catch: a synchronous helper called from the block that
// awaits internally, or a rename of the anchor comments into something this
// no longer finds (which fails loudly rather than silently — see the first
// assertion).
check('SOURCE CANARY: the rebind block contains no await', () => {
  const src = fs.readFileSync(SERVICE_PATH, 'utf8');
  const START = '----- the rebind: one synchronous block, no awaits -----';
  const END = '----- end of the rebind -----';
  const start = src.indexOf(START);
  const end = src.indexOf(END);
  assert.ok(
    start > 0 && end > start,
    'the rebind block is no longer delimited by its anchor comments — this canary '
    + 'cannot see the block it exists to guard; re-anchor it rather than deleting it',
  );
  const block = src.slice(start + START.length, end);

  // Guard the guard: an empty or gutted block must not pass by having nothing
  // in it to fail on.
  assert.ok(/pane\.filePath\s*=/.test(block), 'the block still assigns filePath');
  assert.ok(/pane\.remote\s*=/.test(block), 'the block still assigns remote');
  assert.ok(/termlabResetDirty/.test(block), 'the block still resets dirty');

  assert.ok(
    !/\bawait\b/.test(block),
    'the rebind must be one synchronous block: an await between the identity '
    + 'assignments lets a joining save wake on a half-rebound pane (new filePath, '
    + 'old remote) and upload the new file to the old host',
  );
});

// ---------------------------------------------------------------------------
// (b) The dialog half — existence check, overwrite prompt, New Folder
// ---------------------------------------------------------------------------
//
// Minimal DOM (no jsdom in this repo), same stub as test_file_dialog.mjs.

function classesOf(el) { return String(el.className || '').split(' ').filter(Boolean); }

function matchesSimple(el, token) {
  if (token.charAt(0) === '.') return classesOf(el).includes(token.slice(1));
  return String(el.tagName || '').toLowerCase() === token.toLowerCase();
}

function makeElement(tag, doc) {
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    className: '',
    children: [],
    style: {},
    disabled: false,
    hidden: false,
    checked: false,
    value: '',
    title: '',
    type: '',
    tabIndex: 0,
    textContent: '',
    isConnected: false,
    parentNode: null,
    get lastChild() { return this.children.length ? this.children[this.children.length - 1] : null; },
    appendChild(child) {
      this.children.push(child);
      child.parentNode = this;
      child.isConnected = this.isConnected;
      return child;
    },
    removeChild(child) {
      const idx = this.children.indexOf(child);
      if (idx >= 0) this.children.splice(idx, 1);
      child.parentNode = null;
      child.isConnected = false;
      return child;
    },
    setAttribute(name, value) { attrs.set(name, String(value)); },
    getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
    removeAttribute(name) { attrs.delete(name); },
    hasAttribute(name) { return attrs.has(name); },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    removeEventListener(name, fn) {
      const arr = listeners.get(name) || [];
      const idx = arr.indexOf(fn);
      if (idx >= 0) arr.splice(idx, 1);
    },
    dispatchEvent(evt) {
      for (const fn of (listeners.get(evt.type) || []).slice()) fn(evt);
      return true;
    },
    fire(type, evt) {
      const event = Object.assign({ type, target: el, preventDefault() {} }, evt || {});
      return this.dispatchEvent(event);
    },
    querySelectorAll(selector) {
      const tokens = String(selector).trim().split(/\s+/);
      const out = [];
      const walk = (node, depth) => {
        for (const child of node.children) {
          if (matchesSimple(child, tokens[depth])) {
            if (depth === tokens.length - 1) out.push(child);
            else walk(child, depth + 1);
          }
          walk(child, depth);
        }
      };
      walk(this, 0);
      return out.filter((node, idx) => out.indexOf(node) === idx);
    },
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; },
    focus() { if (doc) doc.activeElement = el; },
    contains(node) { let n = node; while (n) { if (n === el) return true; n = n.parentNode; } return false; },
    classList: { add() {}, remove() {}, contains(c) { return classesOf(el).includes(c); } },
  };
  return el;
}

function makeDocument() {
  const doc = { activeElement: null, addEventListener() {}, removeEventListener() {} };
  doc.createElement = (tag) => makeElement(tag, doc);
  doc.body = makeElement('body', doc);
  doc.body.isConnected = true;
  return doc;
}

const dialogTick = () => new Promise((resolve) => setImmediate(resolve));
const dialogSettle = async (n = 10) => { for (let i = 0; i < n; i++) await dialogTick(); };

function makeDialogHarness(options = {}) {
  const opts = options;
  const doc = makeDocument();
  const sandbox = { console, document: doc, setTimeout, clearTimeout, Promise };
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const calls = {
    listLocal: [], listRemote: [], statLocal: [], statRemote: [],
    mkdirLocal: [], mkdirRemote: [], saveAs: [],
  };
  const toasts = [];

  sandbox.termlabKeyboardRouter = { register: () => () => {} };
  sandbox.toast = {
    error: (title, body) => toasts.push({ level: 'error', title, body }),
    success: (title, body) => toasts.push({ level: 'success', title, body }),
  };
  sandbox.utils = { formatSize: (n) => `${n}B`, formatDate: () => '' };
  sandbox.termlabServices = { tauriClient: { invoke: () => Promise.reject(new Error('no raw invoke expected')) } };

  // The REAL data service with only its invoke-backed IO stubbed, so
  // sessionHostLabel stays the shared formula (Task 5's fix-round pattern).
  load(sandbox, 'features/files/data-service.js');
  const realFilesData = sandbox.termlabFilesFeatureDataService;
  sandbox.termlabFilesFeatureDataService = Object.assign({}, realFilesData, {
    getHomeDir: () => Promise.resolve('/home/u'),
    getCurrentWindowLabel: () => Promise.resolve('main'),
    getSessions: () => Promise.resolve(opts.sessions || []),
    listLocalDir: (invoke, p) => { calls.listLocal.push(p); return (opts.listLocal || (() => Promise.resolve([])))(p); },
    listRemoteDir: (invoke, paneId, p) => {
      calls.listRemote.push([paneId, p]);
      return (opts.listRemote || (() => Promise.resolve([])))(paneId, p);
    },
    getRemoteRealPath: () => Promise.resolve('/home/remote'),
    // FileEntry on success; a bare String rejection when it does not exist.
    statLocal: (invoke, p) => {
      calls.statLocal.push(p);
      return (opts.statLocal || (() => Promise.reject('No such file or directory')))(p);
    },
    statRemote: (invoke, paneId, p) => {
      calls.statRemote.push([paneId, p]);
      return (opts.statRemote || (() => Promise.reject('No such file')))(paneId, p);
    },
    localMkdir: (invoke, p) => { calls.mkdirLocal.push(p); return Promise.resolve(null); },
    remoteMkdir: (invoke, paneId, p) => { calls.mkdirRemote.push([paneId, p]); return Promise.resolve(null); },
  });

  sandbox.termlabEditorService = {
    openLocalFile: () => Promise.resolve(),
    openRemoteFile: () => Promise.resolve(),
    saveAs: (pane, target) => { calls.saveAs.push(target); return Promise.resolve(); },
  };

  load(sandbox, 'ui/tl-dialog.js');
  load(sandbox, 'features/editor/file-dialog-model.js');
  load(sandbox, 'features/editor/file-dialog.js');

  return { sandbox, doc, calls, toasts };
}

// The topmost dialog on screen.
function topDialog(doc) {
  const overlay = doc.body.children[doc.body.children.length - 1] || null;
  if (!overlay) return null;
  return {
    overlay,
    panel: overlay.children[0],
    title: (overlay.querySelectorAll('.tl-dialog__title')[0] || {}).textContent,
    rows: overlay.querySelectorAll('.tl-filedlg__row'),
    nameInput: overlay.querySelectorAll('.tl-filedlg__name')[0] || null,
    newFolderBtn: overlay.querySelectorAll('.tl-filedlg__newfolder')[0] || null,
    promptInput: overlay.querySelectorAll('.tl-filedlg__prompt-input')[0] || null,
    buttons: overlay.querySelectorAll('.tl-dialog__footer .tl-btn'),
    button: (label) => overlay.querySelectorAll('.tl-dialog__footer .tl-btn').find((b) => b.textContent === label) || null,
  };
}

const dialogCount = (doc) => doc.body.children.length;

console.log('editor save as: the dialog');

await checkAsync('(b) an existing target prompts, and declining changes nothing', async () => {
  const h = makeDialogHarness({
    listLocal: () => Promise.resolve([{ name: 'gamma.md', is_dir: false, size: 4, modified: null }]),
    statLocal: () => Promise.resolve({ name: 'gamma.md', is_dir: false, size: 4, modified: null }),
  });
  const pane = { kind: 'editor', tabId: 1, filePath: '/tmp/termlab-scratch/scratch-1.txt', remote: null };
  h.sandbox.termlabFileDialog.openForSave(pane);
  await dialogSettle();

  const dlg = topDialog(h.doc);
  assert.ok(dlg.nameInput, 'save mode has a filename field');
  assert.strictEqual(dlg.nameInput.value, 'scratch-1.txt', 'prefilled from the pane’s current basename');
  assert.ok(dlg.button('Save'), 'the primary button reads Save');

  dlg.nameInput.value = 'gamma.md';
  dlg.button('Save').fire('click');
  await dialogSettle();

  assert.deepStrictEqual(h.calls.statLocal, ['/home/u/gamma.md'], 'existence is checked before resolving');
  const prompt = topDialog(h.doc);
  assert.match(String(prompt.title), /overwrite/i, 'an overwrite prompt is on top');
  assert.strictEqual(dialogCount(h.doc), 2, 'stacked over the chooser, which stays open');

  prompt.button('Cancel').fire('click');
  await dialogSettle();
  assert.strictEqual(h.calls.saveAs.length, 0, 'declining saves nothing');
  assert.strictEqual(dialogCount(h.doc), 1, 'and leaves the chooser up to pick somewhere else');
});

await checkAsync('(b) accepting the overwrite resolves the target', async () => {
  const h = makeDialogHarness({
    listLocal: () => Promise.resolve([{ name: 'gamma.md', is_dir: false, size: 4, modified: null }]),
    statLocal: () => Promise.resolve({ name: 'gamma.md', is_dir: false, size: 4, modified: null }),
  });
  const pane = { kind: 'editor', tabId: 1, filePath: '/tmp/x/scratch-1.txt', remote: null };
  const saving = h.sandbox.termlabFileDialog.openForSave(pane);
  await dialogSettle();
  const dlg = topDialog(h.doc);
  dlg.nameInput.value = 'gamma.md';
  dlg.button('Save').fire('click');
  await dialogSettle();
  topDialog(h.doc).button('Overwrite').fire('click');
  await dialogSettle();
  await settles(saving, 'the Save As routed by the dialog');

  assert.deepEqual(h.calls.saveAs, [{ scope: 'local', path: '/home/u/gamma.md' }]);
  assert.strictEqual(dialogCount(h.doc), 0, 'both dialogs are gone');
});

await checkAsync('(b) a target that does not exist saves with no prompt', async () => {
  const h = makeDialogHarness({ listLocal: () => Promise.resolve([]) });
  const pane = { kind: 'editor', tabId: 1, filePath: '/tmp/x/scratch-1.txt', remote: null };
  const saving = h.sandbox.termlabFileDialog.openForSave(pane);
  await dialogSettle();
  const dlg = topDialog(h.doc);
  dlg.nameInput.value = 'brand-new.txt';
  dlg.button('Save').fire('click');
  await dialogSettle();
  await settles(saving, 'a Save As to a fresh path');

  assert.deepEqual(h.calls.saveAs, [{ scope: 'local', path: '/home/u/brand-new.txt' }]);
});

await checkAsync('(b) a remote target routes the CLEAN host label and the pane id', async () => {
  // Two panes on ONE host, so the scope button carries the " (pane N)" display
  // suffix and the clean identity string is distinguishable from it. That
  // suffix must never reach editor_temp_path.
  const h = makeDialogHarness({
    sessions: [
      { key: 'main:1', host: 'beta', user: 'bob', port: 2222 },
      { key: 'main:2', host: 'beta', user: 'bob', port: 2222 },
    ],
    listRemote: () => Promise.resolve([]),
    statRemote: () => Promise.reject('No such file'),
  });
  const pane = { kind: 'editor', tabId: 1, filePath: '/tmp/x/scratch-1.txt', remote: null };
  const saving = h.sandbox.termlabFileDialog.openForSave(pane);
  await dialogSettle();

  const overlay = h.doc.body.children[0];
  const scopes = overlay.querySelectorAll('.tl-filedlg__scope');
  assert.deepStrictEqual(scopes.map((b) => b.textContent),
    ['This Mac', 'bob@beta:2222 (pane 1)', 'bob@beta:2222 (pane 2)'],
    'precondition: neither remote button caption is the bare host label');
  scopes[2].fire('click');
  await dialogSettle();

  const dlg = topDialog(h.doc);
  dlg.nameInput.value = 'beta.py';
  dlg.button('Save').fire('click');
  await dialogSettle();
  await settles(saving, 'a remote Save As routed by the dialog');

  assert.deepEqual(h.calls.saveAs, [{
    scope: 'remote', paneId: 2, hostLabel: 'bob@beta:2222', remotePath: '/home/remote/beta.py',
  }]);
  assert.ok(!/pane/.test(h.calls.saveAs[0].hostLabel), 'the display suffix never reaches the temp path');
});

await checkAsync('(b) New Folder creates the directory and re-lists', async () => {
  let listings = 0;
  const h = makeDialogHarness({
    listLocal: () => { listings++; return Promise.resolve([]); },
  });
  const pane = { kind: 'editor', tabId: 1, filePath: '/tmp/x/scratch-1.txt', remote: null };
  h.sandbox.termlabFileDialog.openForSave(pane);
  await dialogSettle();
  const before = listings;

  const dlg = topDialog(h.doc);
  assert.ok(dlg.newFolderBtn, 'save mode offers New Folder');
  dlg.newFolderBtn.fire('click');
  await dialogSettle();

  const prompt = topDialog(h.doc);
  assert.ok(prompt.promptInput, 'it asks for a name');
  prompt.promptInput.value = 'drafts';
  prompt.button('Create').fire('click');
  await dialogSettle();

  assert.deepStrictEqual(h.calls.mkdirLocal, ['/home/u/drafts'], 'mkdir under the current directory');
  assert.ok(listings > before, 'and the listing is refreshed');
});

await checkAsync('(b) Save is inert with an empty filename', async () => {
  const h = makeDialogHarness({ listLocal: () => Promise.resolve([]) });
  const pane = { kind: 'editor', tabId: 1, filePath: '/tmp/x/scratch-1.txt', remote: null };
  h.sandbox.termlabFileDialog.openForSave(pane);
  await dialogSettle();
  const dlg = topDialog(h.doc);
  dlg.nameInput.value = '   ';
  dlg.button('Save').fire('click');
  await dialogSettle();
  assert.strictEqual(h.calls.statLocal.length, 0, 'nothing is even stat-ed');
  assert.strictEqual(h.calls.saveAs.length, 0);
  assert.ok(dialogCount(h.doc) >= 1, 'the chooser stays open');
});

await checkAsync('(b) a remote pane prefills the REMOTE basename, not the temp file', async () => {
  const h = makeDialogHarness({ listLocal: () => Promise.resolve([]) });
  const pane = {
    kind: 'editor',
    tabId: 1,
    filePath: tempPathFor(OLD.hostLabel, OLD.remotePath),
    remote: { ...OLD },
  };
  h.sandbox.termlabFileDialog.openForSave(pane);
  await dialogSettle();
  assert.strictEqual(topDialog(h.doc).nameInput.value, 'alpha.conf');
});

// ---------------------------------------------------------------------------

if (failures) {
  console.error(`editor save as: ${failures} of ${ran} check(s) FAILED`);
  process.exit(1);
}
console.log(`editor save as: all ${ran} checks passed`);
