// Run: node scripts/tests/test_file_dialog.mjs
//
// Exercises app/features/editor/file-dialog.js against the REAL
// app/ui/tl-dialog.js and the REAL app/features/editor/file-dialog-model.js,
// with a minimal DOM stub (no jsdom in this repo — same approach as
// test_tl_dialog.mjs / test_tl_combo.mjs).
//
// The load-bearing behaviours, in the order the task brief ranks them:
//   1. remote_get_sessions' ACTUAL shape ({key,host,user,port}) becomes a
//      scope list — including the "key is {window_label}:{pane_id}" parse and
//      the host label that must match files-panel.js byte for byte.
//   2. A listing that rejects renders inline in the dialog body — not a
//      toast, not a blank list, not a close — with the scope bar still live.
//   3. Enter on a directory descends, Enter on a file opens, and the
//      double-enter race (descend, then a second Enter before the listing
//      lands) fires ZERO opens rather than two.
// Plus: Escape/backdrop cancel come from tl-dialog (verified, not
// re-implemented), the Open button gates on a file selection, and the routing
// into termlabEditorService.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

let failures = 0;
function check(name, fn) {
  try {
    fn();
    return true;
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
    return false;
  }
}
async function checkAsync(name, fn) {
  try {
    await fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}

const tick = () => new Promise((resolve) => setImmediate(resolve));
const settle = async (n = 8) => { for (let i = 0; i < n; i++) await tick(); };

function deferred() {
  const d = {};
  d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
  return d;
}

// ---------------------------------------------------------------------------
// Minimal DOM
// ---------------------------------------------------------------------------

function classesOf(el) {
  return String(el.className || '').split(' ').filter(Boolean);
}

// Supports only what the modules under test use: a descendant chain of class
// selectors (".tl-dialog__footer .tl-btn") or a single class.
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
    // Test-only: fire a listener directly with a synthetic event.
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
      // De-dupe: the walk above can reach a node by more than one path.
      return out.filter((node, idx) => out.indexOf(node) === idx);
    },
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; },
    focus() { if (doc) doc.activeElement = el; },
    contains(node) { let n = node; while (n) { if (n === el) return true; n = n.parentNode; } return false; },
    classList: {
      add() {}, remove() {}, contains(c) { return classesOf(el).includes(c); },
    },
  };
  return el;
}

function makeDocument() {
  const doc = {
    activeElement: null,
    addEventListener() {},
    removeEventListener() {},
  };
  doc.createElement = (tag) => makeElement(tag, doc);
  doc.body = makeElement('body', doc);
  doc.body.isConnected = true;
  return doc;
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

function load(sandbox, relPath) {
  vm.runInContext(fs.readFileSync(path.join(APP, relPath), 'utf8'), sandbox, { filename: relPath });
}

function makeHarness(options) {
  const opts = options || {};
  const doc = makeDocument();
  const sandbox = { console, document: doc, setTimeout, clearTimeout, Promise };
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const calls = { listLocal: [], listRemote: [], realpath: [], openLocal: [], openRemote: [] };
  const toasts = [];

  // Escape handlers registered by tl-dialog through the keyboard router.
  const routed = [];
  sandbox.termlabKeyboardRouter = {
    register(spec) {
      routed.push(spec);
      return () => { const i = routed.indexOf(spec); if (i >= 0) routed.splice(i, 1); };
    },
  };

  sandbox.toast = {
    error: (title, body) => toasts.push({ level: 'error', title, body }),
    success: (title, body) => toasts.push({ level: 'success', title, body }),
  };
  sandbox.utils = { formatSize: (n) => `${n}B`, formatDate: () => '' };
  sandbox.termlabServices = { tauriClient: { invoke: () => Promise.reject(new Error('no raw invoke expected')) } };

  const listLocal = opts.listLocal || (() => Promise.resolve([]));
  const listRemote = opts.listRemote || (() => Promise.resolve([]));

  sandbox.termlabFilesFeatureDataService = {
    getHomeDir: () => Promise.resolve(opts.home || '/home/u'),
    getCurrentWindowLabel: () => Promise.resolve(
      Object.prototype.hasOwnProperty.call(opts, 'windowLabel') ? opts.windowLabel : 'main',
    ),
    getSessions: () => Promise.resolve(opts.sessions || []),
    listLocalDir: (invoke, p) => { calls.listLocal.push(p); return listLocal(p); },
    listRemoteDir: (invoke, paneId, p) => { calls.listRemote.push([paneId, p]); return listRemote(paneId, p); },
    getRemoteRealPath: (invoke, paneId, p) => {
      calls.realpath.push([paneId, p]);
      return opts.realpath ? opts.realpath(paneId, p) : Promise.resolve('/home/remote');
    },
  };

  sandbox.termlabEditorService = {
    openLocalFile: (p) => { calls.openLocal.push(p); return Promise.resolve(); },
    openRemoteFile: (d) => { calls.openRemote.push(d); return Promise.resolve(); },
  };

  load(sandbox, 'ui/tl-dialog.js');
  load(sandbox, 'features/editor/file-dialog-model.js');
  load(sandbox, 'features/editor/file-dialog.js');

  return { sandbox, doc, calls, toasts, routed };
}

// Locate the live dialog's parts by class, from document.body.
function parts(doc) {
  const overlay = doc.body.children[doc.body.children.length - 1] || null;
  if (!overlay) return null;
  const find = (cls) => overlay.querySelectorAll(`.${cls}`);
  return {
    overlay,
    panel: overlay.children[0],
    scopes: find('tl-filedlg__scope'),
    crumbs: find('tl-filedlg__crumb'),
    rows: find('tl-filedlg__row'),
    error: find('tl-filedlg__error')[0] || null,
    empty: find('tl-filedlg__empty')[0] || null,
    list: find('tl-filedlg__list')[0] || null,
    pathInput: find('tl-filedlg__path')[0] || null,
    filterInput: find('tl-filedlg__filter')[0] || null,
    hiddenBox: (find('tl-filedlg__hidden')[0] || { children: [] }).children[0] || null,
    buttons: overlay.querySelectorAll('.tl-dialog__footer .tl-btn'),
  };
}

const rowName = (row) => row.children[0].textContent;

// ---------------------------------------------------------------------------
// 1. Pure derivations from remote_get_sessions' real shape
// ---------------------------------------------------------------------------

console.log('file dialog: session -> scope derivation');
{
  const { sandbox } = makeHarness({});
  const fd = sandbox.termlabFileDialog;

  check('key "{window_label}:{pane_id}" yields the pane id', () => {
    assert.strictEqual(fd._paneIdFromSessionKey('main:3', 'main'), 3);
    assert.strictEqual(fd._paneIdFromSessionKey('main:0', 'main'), 0);
  });
  check('a session from another window is not addressable and is rejected', () => {
    assert.strictEqual(fd._paneIdFromSessionKey('window-2:3', 'main'), null);
  });
  check('a non-numeric tail or an unknown window label is rejected', () => {
    assert.strictEqual(fd._paneIdFromSessionKey('main:abc', 'main'), null);
    assert.strictEqual(fd._paneIdFromSessionKey('main:3', null), null);
    assert.strictEqual(fd._paneIdFromSessionKey(null, 'main'), null);
  });

  check('host label matches files-panel: user@host, port dropped when 22', () => {
    assert.strictEqual(fd._sessionHostLabel({ host: 'h1', user: 'ubuntu', port: 22 }, 3), 'ubuntu@h1');
    assert.strictEqual(fd._sessionHostLabel({ host: 'h1', user: 'ubuntu', port: 2222 }, 3), 'ubuntu@h1:2222');
    assert.strictEqual(fd._sessionHostLabel({ host: 'h1', user: '', port: 22 }, 3), 'h1');
    assert.strictEqual(fd._sessionHostLabel({ host: '', user: 'u', port: 22 }, 3), 'pane-3');
  });

  check('scopes: local first, only this window, sorted, hostLabel kept clean', () => {
    const sessions = [
      { key: 'main:7', host: 'zeta', user: 'u', port: 22 },
      { key: 'window-2:1', host: 'other', user: 'u', port: 22 },
      { key: 'main:2', host: 'alpha', user: 'u', port: 22 },
      { key: 'main:5', host: 'alpha', user: 'u', port: 22 },
    ];
    const scopes = sandbox.termlabFileDialog._buildScopes(sessions, 'main', '/home/u');
    assert.strictEqual(scopes.length, 4, 'local + 3 same-window sessions');
    assert.strictEqual(scopes[0].kind, 'local');
    assert.strictEqual(scopes[0].label, 'This Mac');
    assert.strictEqual(scopes[0].start, '/home/u');
    assert.strictEqual(scopes[1].paneId, 2);
    assert.strictEqual(scopes[2].paneId, 5);
    assert.strictEqual(scopes[3].paneId, 7);
    // Duplicated host: the BUTTON disambiguates, the hostLabel does not —
    // hostLabel is hashed into the temp path by editor_temp_path, so
    // changing it would split one remote file across two editor tabs.
    assert.strictEqual(scopes[1].label, 'u@alpha (pane 2)');
    assert.strictEqual(scopes[1].hostLabel, 'u@alpha');
    assert.strictEqual(scopes[2].hostLabel, 'u@alpha');
    assert.strictEqual(scopes[3].label, 'u@zeta');
    assert.strictEqual(scopes[3].hostLabel, 'u@zeta');
  });

  check('no window label => no remote scopes offered', () => {
    const scopes = sandbox.termlabFileDialog._buildScopes(
      [{ key: 'main:1', host: 'h', user: 'u', port: 22 }], null, '/home/u',
    );
    assert.strictEqual(scopes.length, 1);
    assert.strictEqual(scopes[0].kind, 'local');
  });
}

// ---------------------------------------------------------------------------
// 2. Rendering, filter, hidden toggle, Open gating
// ---------------------------------------------------------------------------

console.log('file dialog: listing, filter, hidden toggle, Open gating');
await checkAsync('renders scopes and sorted rows; Open gates on a file', async () => {
  const entries = [
    { name: 'zz.txt', is_dir: false, size: 10, modified: null },
    { name: '.hidden', is_dir: false, size: 1, modified: null },
    { name: 'sub', is_dir: true, size: 0, modified: null },
    { name: 'aa.txt', is_dir: false, size: 20, modified: null },
  ];
  const { sandbox, doc } = makeHarness({
    sessions: [{ key: 'main:4', host: 'h1', user: 'ubuntu', port: 22 }],
    listLocal: () => Promise.resolve(entries),
  });
  sandbox.termlabFileDialog._chooseFile();
  await settle();

  const p = parts(doc);
  assert.deepStrictEqual(p.scopes.map((b) => b.textContent), ['This Mac', 'ubuntu@h1']);
  assert.strictEqual(p.scopes[0].getAttribute('aria-pressed'), 'true');
  assert.deepStrictEqual(p.rows.map(rowName), ['sub', 'aa.txt', 'zz.txt'], 'dirs first, then name; dotfile hidden');
  assert.strictEqual(p.pathInput.value, '/home/u');
  assert.deepStrictEqual(p.crumbs.map((c) => c.textContent), ['/', 'home', 'u']);

  const openBtn = p.buttons.find((b) => b.textContent === 'Open');
  assert.ok(openBtn, 'the footer has an Open button');
  assert.strictEqual(openBtn.disabled, true, 'Open starts disabled');

  p.rows[0].fire('click'); // the directory
  assert.strictEqual(openBtn.disabled, true, 'a selected DIRECTORY does not enable Open');
  p.rows[1].fire('click'); // aa.txt
  assert.strictEqual(openBtn.disabled, false, 'a selected FILE enables Open');
  assert.strictEqual(openBtn.getAttribute('aria-disabled'), 'false');
});

await checkAsync('hidden toggle and filter re-render through the model', async () => {
  const entries = [
    { name: '.bashrc', is_dir: false, size: 1, modified: null },
    { name: 'notes.txt', is_dir: false, size: 1, modified: null },
    { name: 'nope.md', is_dir: false, size: 1, modified: null },
  ];
  const { sandbox, doc } = makeHarness({ listLocal: () => Promise.resolve(entries) });
  sandbox.termlabFileDialog._chooseFile();
  await settle();

  let p = parts(doc);
  assert.deepStrictEqual(p.rows.map(rowName), ['nope.md', 'notes.txt']);

  p.hiddenBox.checked = true;
  p.hiddenBox.fire('change');
  p = parts(doc);
  assert.deepStrictEqual(p.rows.map(rowName), ['.bashrc', 'nope.md', 'notes.txt'], 'hidden toggle reveals dotfiles');

  p.filterInput.value = 'notes';
  p.filterInput.fire('input');
  p = parts(doc);
  assert.deepStrictEqual(p.rows.map(rowName), ['notes.txt']);
});

// ---------------------------------------------------------------------------
// 3. Enter semantics + the double-enter race
// ---------------------------------------------------------------------------

console.log('file dialog: Enter descends on a directory, opens on a file');
await checkAsync('Enter on a directory descends and lists the child path', async () => {
  const { sandbox, doc, calls } = makeHarness({
    listLocal: (p) => Promise.resolve(p === '/home/u'
      ? [{ name: 'sub', is_dir: true, size: 0, modified: null }]
      : [{ name: 'deep.txt', is_dir: false, size: 3, modified: null }]),
  });
  const choice = sandbox.termlabFileDialog._chooseFile();
  let resolved = false;
  choice.then(() => { resolved = true; });
  await settle();

  parts(doc).rows[0].fire('click');
  parts(doc).list.fire('keydown', { key: 'Enter' });
  await settle();

  assert.deepStrictEqual(calls.listLocal, ['/home/u', '/home/u/sub']);
  assert.deepStrictEqual(parts(doc).rows.map(rowName), ['deep.txt']);
  assert.strictEqual(parts(doc).pathInput.value, '/home/u/sub');
  assert.strictEqual(resolved, false, 'descending into a directory does not resolve the chooser');
});

await checkAsync('Enter on a file resolves once, even pressed twice', async () => {
  const { sandbox, doc } = makeHarness({
    listLocal: () => Promise.resolve([{ name: 'a.txt', is_dir: false, size: 5, modified: null }]),
  });
  const choice = sandbox.termlabFileDialog._chooseFile();
  const seen = [];
  choice.then((v) => seen.push(v));
  await settle();

  const p = parts(doc);
  p.rows[0].fire('click');
  p.list.fire('keydown', { key: 'Enter' });
  p.list.fire('keydown', { key: 'Enter' });
  await settle();

  assert.strictEqual(seen.length, 1, 'exactly one resolution');
  assert.strictEqual(seen[0].path, '/home/u/a.txt');
  assert.strictEqual(seen[0].entry.size, 5);
  assert.strictEqual(seen[0].scope.kind, 'local');
  assert.strictEqual(doc.body.children.length, 0, 'the dialog closed itself');
});

await checkAsync('THE RACE: Enter on a dir then Enter again mid-listing opens nothing', async () => {
  const pending = deferred();
  let nth = 0;
  const { sandbox, doc, calls } = makeHarness({
    listLocal: (p) => {
      nth++;
      if (nth === 1) {
        return Promise.resolve([
          { name: 'sub', is_dir: true, size: 0, modified: null },
          { name: 'a.txt', is_dir: false, size: 5, modified: null },
        ]);
      }
      return pending.promise; // the descent hangs
    },
  });
  const choice = sandbox.termlabFileDialog._chooseFile();
  let resolved = null;
  choice.then((v) => { resolved = v; });
  await settle();

  let p = parts(doc);
  // Select the FILE first, so a stale selection would open it if it survived.
  p.rows[1].fire('click');
  const openBtn = p.buttons.find((b) => b.textContent === 'Open');
  assert.strictEqual(openBtn.disabled, false, 'precondition: a file is selected');

  // Now select the directory and descend.
  p.rows[0].fire('click');
  p.list.fire('keydown', { key: 'Enter' });

  // Second Enter, synchronously, while the listing is still in flight.
  p.list.fire('keydown', { key: 'Enter' });
  p.list.fire('keydown', { key: 'Enter' });
  await settle();

  assert.deepStrictEqual(calls.listLocal, ['/home/u', '/home/u/sub'],
    'the extra Enters started no further listings');
  assert.strictEqual(resolved, null, 'no open fired while the descent was in flight');
  assert.strictEqual(openBtn.disabled, true, 'the selection was dropped synchronously on descent');
  assert.strictEqual(doc.body.children.length, 1, 'the dialog is still open');

  pending.resolve([{ name: 'deep.txt', is_dir: false, size: 1, modified: null }]);
  await settle();
  p = parts(doc);
  assert.deepStrictEqual(p.rows.map(rowName), ['deep.txt']);
  assert.strictEqual(resolved, null, 'the landed listing still opened nothing');
});

// The same race on the path-field route, where the selection is the ONLY
// stale state a second Enter could act on — and where acting on it would be a
// real open of a file in a directory the user has already left.
await checkAsync('THE RACE, path-field route: a stale file selection cannot be opened', async () => {
  const pending = deferred();
  const { sandbox, doc, calls } = makeHarness({
    listLocal: (p) => (p === '/home/u'
      ? Promise.resolve([{ name: 'a.txt', is_dir: false, size: 5, modified: null }])
      : pending.promise),
  });
  const choice = sandbox.termlabFileDialog._chooseFile();
  let resolved = null;
  choice.then((v) => { resolved = v; });
  await settle();

  const p = parts(doc);
  p.rows[0].fire('click');
  assert.strictEqual(p.buttons.find((b) => b.textContent === 'Open').disabled, false,
    'precondition: /home/u/a.txt is selected and Open is live');

  // Jump elsewhere; the listing hangs. The rows for /home/u are still on
  // screen, so a retained selectedIndex would still resolve to a.txt.
  p.pathInput.value = '/var/log';
  p.pathInput.fire('keydown', { key: 'Enter' });
  p.list.fire('keydown', { key: 'Enter' });
  p.list.fire('keydown', { key: 'Enter' });
  await settle();

  assert.strictEqual(resolved, null, 'no file was opened out from under the jump');
  assert.strictEqual(calls.openLocal.length, 0);
  assert.strictEqual(doc.body.children.length, 1, 'the dialog is still open');

  pending.resolve([{ name: 'syslog', is_dir: false, size: 1, modified: null }]);
  await settle();
  assert.strictEqual(resolved, null);
  assert.deepStrictEqual(parts(doc).rows.map(rowName), ['syslog']);
});

// ---------------------------------------------------------------------------
// 4. Failure modes: inline error, disconnect mid-browse
// ---------------------------------------------------------------------------

console.log('file dialog: failure modes render inline');
await checkAsync('a rejected listing renders inline, not as a toast, and keeps the dialog open', async () => {
  const { sandbox, doc, toasts } = makeHarness({
    listLocal: () => Promise.reject(new Error('Permission denied (os error 13)')),
  });
  sandbox.termlabFileDialog._chooseFile();
  await settle();

  const p = parts(doc);
  assert.strictEqual(p.error.hidden, false, 'the inline error is visible');
  assert.match(p.error.textContent, /Permission denied/);
  assert.strictEqual(p.error.getAttribute('role'), 'alert');
  assert.strictEqual(p.empty.hidden, true, 'the "No matches" placeholder does not double up on the error');
  assert.strictEqual(toasts.length, 0, 'no toast was raised');
  assert.strictEqual(doc.body.children.length, 1, 'the dialog did not close');
});

await checkAsync('a host that drops mid-browse errors inline and the scope bar still switches', async () => {
  let remoteUp = true;
  const { sandbox, doc, calls } = makeHarness({
    sessions: [{ key: 'main:4', host: 'h1', user: 'ubuntu', port: 22 }],
    listLocal: () => Promise.resolve([{ name: 'local.txt', is_dir: false, size: 1, modified: null }]),
    listRemote: () => (remoteUp
      ? Promise.resolve([{ name: 'remote.txt', is_dir: false, size: 2, modified: null }])
      : Promise.reject(new Error('No SSH connection for pane 4'))),
  });
  sandbox.termlabFileDialog._chooseFile();
  await settle();

  // Switch to the host: realpath('.') resolves ~ once at scope entry.
  parts(doc).scopes[1].fire('click');
  await settle();
  assert.deepStrictEqual(calls.realpath, [[4, '.']], 'sftp_realpath once, at scope entry');
  assert.deepStrictEqual(parts(doc).rows.map(rowName), ['remote.txt']);
  assert.strictEqual(parts(doc).error.hidden, true);

  // Host drops; navigate up.
  remoteUp = false;
  parts(doc).crumbs[0].fire('click');
  await settle();
  let p = parts(doc);
  assert.strictEqual(p.error.hidden, false);
  assert.match(p.error.textContent, /No SSH connection/);
  assert.strictEqual(p.rows.length, 0);
  assert.strictEqual(p.scopes.length, 2, 'the scope bar is still there to switch away with');

  // Switching back to This Mac clears the error and lists again.
  p.scopes[0].fire('click');
  await settle();
  p = parts(doc);
  assert.strictEqual(p.error.hidden, true, 'switching scope clears the error');
  assert.deepStrictEqual(p.rows.map(rowName), ['local.txt']);
});

await checkAsync('a scope-entry realpath failure errors inline without closing', async () => {
  const { sandbox, doc } = makeHarness({
    sessions: [{ key: 'main:9', host: 'h9', user: 'u', port: 22 }],
    listLocal: () => Promise.resolve([]),
    realpath: () => Promise.reject(new Error('sftp channel closed')),
  });
  sandbox.termlabFileDialog._chooseFile();
  await settle();
  parts(doc).scopes[1].fire('click');
  await settle();

  const p = parts(doc);
  assert.strictEqual(p.error.hidden, false);
  assert.match(p.error.textContent, /sftp channel closed/);
  assert.strictEqual(doc.body.children.length, 1);
});

// ---------------------------------------------------------------------------
// 5. Cancel paths come from tl-dialog (verified, not re-implemented)
// ---------------------------------------------------------------------------

console.log('file dialog: cancel paths');
await checkAsync('Escape resolves null through tl-dialog\'s own router registration', async () => {
  const { sandbox, doc, routed } = makeHarness({ listLocal: () => Promise.resolve([]) });
  const choice = sandbox.termlabFileDialog._chooseFile();
  let resolved = 'unset';
  choice.then((v) => { resolved = v; });
  await settle();

  const escape = routed.find((r) => r.name === 'tl-dialog');
  assert.ok(escape, 'tl-dialog registered an Escape handler with the keyboard router');
  assert.strictEqual(escape.isActive(), true);
  assert.strictEqual(escape.onKeyDown({ key: 'Escape' }), true, 'Escape was consumed');
  await settle();
  assert.strictEqual(resolved, null);
  assert.strictEqual(doc.body.children.length, 0);
});

await checkAsync('a backdrop mousedown resolves null', async () => {
  const { sandbox, doc } = makeHarness({ listLocal: () => Promise.resolve([]) });
  const choice = sandbox.termlabFileDialog._chooseFile();
  let resolved = 'unset';
  choice.then((v) => { resolved = v; });
  await settle();

  const overlay = doc.body.children[0];
  overlay.dispatchEvent({ type: 'mousedown', target: overlay });
  await settle();
  assert.strictEqual(resolved, null);
  assert.strictEqual(doc.body.children.length, 0);
});

await checkAsync('Cancel resolves null and opens nothing', async () => {
  const { sandbox, doc, calls } = makeHarness({
    listLocal: () => Promise.resolve([{ name: 'a.txt', is_dir: false, size: 1, modified: null }]),
  });
  const choice = sandbox.termlabFileDialog.openForOpen();
  await settle();
  const p = parts(doc);
  p.rows[0].fire('click');
  p.buttons.find((b) => b.textContent === 'Cancel').fire('click');
  const result = await choice;
  assert.strictEqual(result, null);
  assert.strictEqual(calls.openLocal.length, 0, 'cancel opens no tab');
  assert.strictEqual(calls.openRemote.length, 0);
});

// ---------------------------------------------------------------------------
// 6. Routing into the editor service
// ---------------------------------------------------------------------------

console.log('file dialog: routing into the editor service');
await checkAsync('a local pick goes to openLocalFile(path)', async () => {
  const { sandbox, doc, calls } = makeHarness({
    listLocal: () => Promise.resolve([{ name: 'a.txt', is_dir: false, size: 5, modified: null }]),
  });
  const done = sandbox.termlabFileDialog.openForOpen();
  await settle();
  parts(doc).rows[0].fire('dblclick');
  await done;
  assert.deepStrictEqual(calls.openLocal, ['/home/u/a.txt']);
  assert.strictEqual(calls.openRemote.length, 0);
});

await checkAsync('a remote pick goes to openRemoteFile with paneId/hostLabel/size', async () => {
  const { sandbox, doc, calls } = makeHarness({
    sessions: [{ key: 'main:4', host: 'h1', user: 'ubuntu', port: 2222 }],
    listLocal: () => Promise.resolve([]),
    listRemote: () => Promise.resolve([{ name: 'app.log', is_dir: false, size: 4242, modified: null }]),
    realpath: () => Promise.resolve('/home/ubuntu'),
  });
  const done = sandbox.termlabFileDialog.openForOpen();
  await settle();
  parts(doc).scopes[1].fire('click');
  await settle();
  parts(doc).rows[0].fire('dblclick');
  await done;
  assert.strictEqual(calls.openLocal.length, 0);
  assert.strictEqual(calls.openRemote.length, 1);
  const d = calls.openRemote[0];
  assert.strictEqual(d.paneId, 4);
  assert.strictEqual(d.remotePath, '/home/ubuntu/app.log');
  assert.strictEqual(d.hostLabel, 'ubuntu@h1:2222');
  assert.strictEqual(d.size, 4242);
});

// ---------------------------------------------------------------------------
// 7. Path field + the Task 6 stub
// ---------------------------------------------------------------------------

console.log('file dialog: path field and openForSave stub');
await checkAsync('Enter in the path field jumps to a directory', async () => {
  const { sandbox, doc, calls } = makeHarness({
    listLocal: (p) => (p === '/etc'
      ? Promise.resolve([{ name: 'hosts', is_dir: false, size: 1, modified: null }])
      : Promise.resolve([])),
  });
  sandbox.termlabFileDialog._chooseFile();
  await settle();
  const p = parts(doc);
  p.pathInput.value = '/etc';
  p.pathInput.fire('keydown', { key: 'Enter' });
  await settle();
  assert.ok(calls.listLocal.includes('/etc'));
  assert.deepStrictEqual(parts(doc).rows.map(rowName), ['hosts']);
});

await checkAsync('a full FILE path in the path field resolves via its parent and opens', async () => {
  const { sandbox, doc } = makeHarness({
    listLocal: (p) => (p === '/etc'
      ? Promise.resolve([{ name: 'hosts', is_dir: false, size: 7, modified: null }])
      : (p === '/etc/hosts' ? Promise.reject(new Error('Not a directory')) : Promise.resolve([]))),
  });
  const choice = sandbox.termlabFileDialog._chooseFile();
  await settle();
  const p = parts(doc);
  p.pathInput.value = '/etc/hosts';
  p.pathInput.fire('keydown', { key: 'Enter' });
  const picked = await choice;
  assert.strictEqual(picked.path, '/etc/hosts');
  assert.strictEqual(picked.entry.size, 7);
});

await checkAsync('openForSave rejects with a message naming Task 6', async () => {
  const { sandbox } = makeHarness({});
  await assert.rejects(
    () => sandbox.termlabFileDialog.openForSave({}),
    /Task 6/,
  );
});

check('the public surface is exactly openForOpen + openForSave (plus test hooks)', () => {
  const { sandbox } = makeHarness({});
  const fd = sandbox.termlabFileDialog;
  assert.strictEqual(typeof fd.openForOpen, 'function');
  assert.strictEqual(typeof fd.openForSave, 'function');
});

// ---------------------------------------------------------------------------

if (failures) {
  console.error(`file dialog: ${failures} check(s) FAILED`);
  process.exit(1);
}
console.log('file dialog: all checks passed');
