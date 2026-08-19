// Run: node scripts/tests/test_file_dialog.mjs
//
// Exercises app/features/editor/file-dialog-view.js — the chooser itself —
// and app/features/editor/file-dialog.js — the question it is asked — against
// the REAL app/features/editor/file-dialog-model.js and (for the paths that
// still involve a dialog) the REAL app/ui/tl-dialog.js, with a minimal DOM
// stub (no jsdom in this repo — same approach as test_tl_dialog.mjs /
// test_tl_combo.mjs).
//
// TWO HOSTS, deliberately:
//   - `mountView()` renders the view into a bare element and collects its
//     `onResolve` answers. That is the contract the chooser WINDOW will use,
//     so every rendering/navigation/race check below is written against it —
//     no dialog anywhere near them.
//   - `_chooseFile()` / `openForOpen()` still drive the tl-dialog host, and
//     the checks that are ABOUT that host (Escape through tl-dialog's router,
//     the backdrop, the one-dialog-at-a-time rule, the routing into the
//     editor service) stay there.
//
// The load-bearing behaviours, in the order the task brief ranks them:
//   1. remote_get_sessions' ACTUAL shape ({key,host,user,port}) becomes a
//      scope list — including the "key is {window_label}:{pane_id}" parse and
//      the host label that must match files-panel.js byte for byte.
//   2. A listing that rejects renders inline in the chooser body — not a
//      toast, not a blank list, not a close — with the scope bar still live.
//   3. Enter on a directory descends, Enter on a file opens, and the
//      double-enter race (descend, then a second Enter before the listing
//      lands) fires ZERO opens rather than two.
// Plus: the view answers exactly once and owns its own Escape, the window
// label it filters sessions by is the PARENT's (never its own), the Open
// button gates on a file selection, and the routing into termlabEditorService.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

let failures = 0;
// Counted, never hand-tallied: a report that states a check count should be
// quoting this runner, not counting call sites by eye.
let ran = 0;
function check(name, fn) {
  ran++;
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
  ran++;
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
    // Test-only spy: records calls so a re-sort's "scroll the held selection
    // into view" behavior (rather than just its end state) can be asserted.
    scrollIntoView(opts) {
      el.scrollIntoViewCalls = (el.scrollIntoViewCalls || 0) + 1;
      el.scrollIntoViewLastOpts = opts;
    },
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
  // app/ui/tl-icon.js reads the appearance attribute off documentElement to
  // pick a _dark variant. Stubbed (rather than tlIcon itself) so the REAL
  // icon helper runs here and the rows are checked against the paths it
  // actually emits.
  doc.documentElement = makeElement('html', doc);
  return doc;
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

function load(sandbox, relPath) {
  vm.runInContext(fs.readFileSync(path.join(APP, relPath), 'utf8'), sandbox, { filename: relPath });
}

// Each harness's stub chooser WINDOW, keyed by the document it belongs to, so
// `parts(doc)` below still reads `(doc)` at every call site it always did.
const chooserOf = new WeakMap();

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

  // THE CHOOSER WINDOW, in miniature.
  //
  // file-dialog.js is a proxy now: it asks Rust for a chooser window and waits
  // for the `chooser-resolved` event carrying the answer back. This transport
  // plays both halves — `open_file_chooser` mounts the REAL view into a root
  // of its own (deliberately NOT in this document's body: the chooser is not
  // in this window any more, and the only thing file-dialog.js may put there
  // is its scrim), and the view's `onResolve` comes back as the event.
  //
  // Any other invoke still rejects: neither the view nor the model may reach
  // for a raw command of its own.
  const chooser = { root: null, mounted: null, reqId: 0 };
  chooserOf.set(doc, chooser);
  const eventHandlers = [];
  let nextReqId = 1;

  function emitResolved(reqId, choice) {
    for (const handler of eventHandlers.slice()) handler({ payload: { reqId, choice } });
  }
  function closeChooserWindow() {
    chooser.root = null;
    chooser.mounted = null;
  }

  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        if (command === 'open_file_chooser') {
          const reqId = nextReqId++;
          chooser.reqId = reqId;
          chooser.root = doc.createElement('div');
          chooser.mounted = sandbox.termlabFileDialogView.build(chooser.root, {
            data: sandbox.termlabFilesFeatureDataService,
            mode: args.mode,
            filename: args.filename,
            selectFilename: args.selectFilename,
            // The PARENT's label, handed down — the chooser window never asks
            // for one of its own.
            parentWindowLabel: Object.prototype.hasOwnProperty.call(opts, 'windowLabel')
              ? opts.windowLabel
              : 'main',
            onResolve: (choice) => { closeChooserWindow(); emitResolved(reqId, choice); },
          });
          if (chooser.mounted && typeof chooser.mounted.focusInitial === 'function') {
            chooser.mounted.focusInitial();
          }
          return Promise.resolve(reqId);
        }
        if (command === 'cancel_file_chooser') {
          // Rust force-resolves the caller's chooser and emits the answer back
          // — it does not simply close the window behind the proxy's back.
          const reqId = chooser.reqId;
          closeChooserWindow();
          emitResolved(reqId, null);
          return Promise.resolve(null);
        }
        if (command === 'focus_file_chooser') return Promise.resolve(null);
        return Promise.reject(new Error(`no raw invoke expected (${command})`));
      },
      listenOnCurrentWindow(name, handler) {
        assert.strictEqual(name, 'chooser-resolved');
        eventHandlers.push(handler);
        return Promise.resolve(() => {
          const i = eventHandlers.indexOf(handler);
          if (i >= 0) eventHandlers.splice(i, 1);
        });
      },
    },
  };

  const listLocal = opts.listLocal || (() => Promise.resolve([]));
  const listRemote = opts.listRemote || (() => Promise.resolve([]));

  // The REAL data-service module, with only its invoke-backed IO stubbed out.
  // `sessionHostLabel` is left real on purpose: it is the shared host-identity
  // formula that files-panel.js also calls, and stubbing it would let the
  // dialog pass with a label this project's other surface would never emit.
  load(sandbox, 'features/files/data-service.js');
  const realFilesData = sandbox.termlabFilesFeatureDataService;
  assert.strictEqual(typeof realFilesData.sessionHostLabel, 'function',
    'data-service exports the shared sessionHostLabel');

  sandbox.termlabFilesFeatureDataService = Object.assign({}, realFilesData, {
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
  });

  sandbox.termlabEditorService = {
    openLocalFile: (p) => { calls.openLocal.push(p); return Promise.resolve(); },
    openRemoteFile: (d) => { calls.openRemote.push(d); return Promise.resolve(); },
  };

  // tl-dialog is still loaded: the chooser's Save-mode confirm/New Folder
  // prompts are small tl-dialogs stacked over whatever host it is in, and the
  // bridge host in file-dialog.js is one. It is no longer what renders the
  // chooser.
  load(sandbox, 'ui/tl-dialog.js');
  // The real icon helper: the chooser's rows and sidebar decorate themselves
  // through it, and stubbing it would let a wrong icon name pass.
  load(sandbox, 'ui/tl-icon.js');
  load(sandbox, 'features/editor/file-dialog-model.js');
  load(sandbox, 'features/editor/file-dialog-view.js');
  load(sandbox, 'features/editor/file-dialog.js');

  return { sandbox, doc, calls, toasts, routed };
}

// Locate the chooser's parts by class, under whatever element holds it.
function partsOf(container) {
  if (!container) return null;
  const find = (cls) => container.querySelectorAll(`.${cls}`);
  return {
    // The sidebar rows: same `.tl-filedlg__scope` button, and still in
    // scope-array order (This Mac, then hosts) — it is the section wrapper
    // and the label span that are new, not the button or its handler.
    scopes: find('tl-filedlg__scope'),
    sectionLabels: find('tl-filedlg__section-label'),
    sections: find('tl-filedlg__section'),
    heads: find('tl-filedlg__col'),
    crumbs: find('tl-filedlg__crumb'),
    rows: find('tl-filedlg__row'),
    error: find('tl-filedlg__error')[0] || null,
    empty: find('tl-filedlg__empty')[0] || null,
    list: find('tl-filedlg__list')[0] || null,
    pathInput: find('tl-filedlg__path')[0] || null,
    nameInput: find('tl-filedlg__name')[0] || null,
    filterInput: find('tl-filedlg__filter')[0] || null,
    hiddenBox: (find('tl-filedlg__hidden')[0] || { children: [] }).children[0] || null,
    // The footer is the VIEW's now, in both hosts: it renders the Hidden
    // toggle / save controls on the left and Cancel + Open|Save on the right.
    footer: find('tl-filedlg__footer')[0] || null,
    footerStart: find('tl-filedlg__footer-start')[0] || null,
    footerEnd: find('tl-filedlg__footer-end')[0] || null,
    buttons: container.querySelectorAll('.tl-filedlg__footer-end .tl-btn'),
  };
}

// Locate the chooser's parts for the proxy-driven checks. The chooser lives in
// its own window now, so this reads the stub window's root rather than an
// overlay in this document — everything those checks assert about the chooser
// is unchanged, only where it is rendered has moved.
function parts(doc) {
  const chooser = chooserOf.get(doc);
  if (!chooser || !chooser.root) return null;
  return Object.assign(partsOf(chooser.root), { root: chooser.root });
}

// NOTE for the checks below that count `doc.body.children`: what file-dialog.js
// puts on this document while a chooser is up is its SCRIM, and nothing else.
// One child means "a chooser is up"; a second means a tl-dialog stacked over
// it. The counts are the same as they were when that one child was the
// chooser's own overlay.

// THE WINDOW HOST, in miniature: an element, a `build` call, and a place for
// the one answer to land. No dialog, no overlay, nothing to close.
function mountView(h, options) {
  const opts = options || {};
  const root = h.doc.createElement('div');
  const answers = [];
  const view = h.sandbox.termlabFileDialogView.build(root, {
    data: h.sandbox.termlabFilesFeatureDataService,
    mode: opts.mode || 'open',
    filename: opts.filename,
    selectFilename: opts.selectFilename,
    // The PARENT window's label — the window whose panes those SSH sessions
    // belong to. Never the view's own; see the label fixture below.
    parentWindowLabel: Object.prototype.hasOwnProperty.call(opts, 'parentWindowLabel')
      ? opts.parentWindowLabel
      : 'main',
    onResolve: (value) => { answers.push(value); },
  });
  return { root, answers, view, parts: () => partsOf(root) };
}

// The cell selectors below are WHERE these tests read a row/scope caption, not
// WHAT they assert: a row now leads with a decorative icon element, so the
// caption is the labelled cell rather than the first child.
const cell = (row, variant) => row.querySelectorAll(`.tl-filedlg__cell--${variant}`)[0] || null;
const rowName = (row) => cell(row, 'name').textContent;
const rowSize = (row) => cell(row, 'size').textContent;
const rowTime = (row) => cell(row, 'time').textContent;
const rowIcon = (row) => (row.querySelectorAll('.tl-icon')[0] || {}).src || null;
const scopeName = (btn) => btn.querySelectorAll('.tl-filedlg__scope-label')[0].textContent;
const colVariant = (h) => classesOf(h).find((c) => c.startsWith('tl-filedlg__col--')).replace('tl-filedlg__col--', '');
const headFor = (p, variant) => p.heads.find((h) => colVariant(h) === variant);
const sortState = (p) => p.heads.map((h) => `${colVariant(h)}:${h.getAttribute('aria-sort')}`);

// ---------------------------------------------------------------------------
// 1. Pure derivations from remote_get_sessions' real shape
// ---------------------------------------------------------------------------

console.log('file dialog: session -> scope derivation');
{
  const { sandbox } = makeHarness({});
  const fd = sandbox.termlabFileDialog;
  // The derivations moved out with the view — they are the chooser's, not the
  // entry point's, and the chooser window will need them without ever loading
  // file-dialog.js.
  const fdv = sandbox.termlabFileDialogView;
  const filesData = sandbox.termlabFilesFeatureDataService;
  const sessionHostLabel = filesData.sessionHostLabel;

  check('key "{window_label}:{pane_id}" yields the pane id', () => {
    assert.strictEqual(fdv._paneIdFromSessionKey('main:3', 'main'), 3);
    assert.strictEqual(fdv._paneIdFromSessionKey('main:0', 'main'), 0);
  });
  check('a session from another window is not addressable and is rejected', () => {
    assert.strictEqual(fdv._paneIdFromSessionKey('window-2:3', 'main'), null);
  });
  check('a non-numeric tail or an unknown window label is rejected', () => {
    assert.strictEqual(fdv._paneIdFromSessionKey('main:abc', 'main'), null);
    assert.strictEqual(fdv._paneIdFromSessionKey('main:3', null), null);
    assert.strictEqual(fdv._paneIdFromSessionKey(null, 'main'), null);
  });

  // THE host-identity formula, pinned at its single definition site. This is
  // the string editor_temp_path hashes into a remote file's temp path, so it
  // is the editor's identity for that file. Both surfaces that can open a
  // remote file — panels/files-panel.js's remoteHostLabel and this dialog's
  // buildScopes — call THIS function; when they each held a private copy,
  // editing one would have split every remote file across two tabs with the
  // whole suite green. Changing any expectation below is a data-loss change,
  // not a cosmetic one.
  check('sessionHostLabel: host + user + non-default port', () => {
    assert.strictEqual(sessionHostLabel({ host: 'h1', user: 'ubuntu', port: 2222 }, 3), 'ubuntu@h1:2222');
  });
  check('sessionHostLabel: the default port 22 is elided', () => {
    assert.strictEqual(sessionHostLabel({ host: 'h1', user: 'ubuntu', port: 22 }, 3), 'ubuntu@h1');
  });
  check('sessionHostLabel: a missing user drops the "@" entirely', () => {
    assert.strictEqual(sessionHostLabel({ host: 'h1', user: '', port: 22 }, 3), 'h1');
    assert.strictEqual(sessionHostLabel({ host: 'h1', user: '', port: 2222 }, 3), 'h1:2222');
  });
  check('sessionHostLabel: no host (or no session) falls back per pane', () => {
    assert.strictEqual(sessionHostLabel({ host: '', user: 'u', port: 22 }, 3), 'pane-3');
    assert.strictEqual(sessionHostLabel(null, 7), 'pane-7');
  });
  check('the dialog does NOT export a second copy of the formula', () => {
    assert.strictEqual(fd._sessionHostLabel, undefined,
      'the formula belongs to features/files/data-service.js alone');
    assert.strictEqual(fdv._sessionHostLabel, undefined,
      'and the view did not grow one on its way out of file-dialog.js');
  });

  // A canary, not a proof: the pin above only guards the one definition, so
  // it cannot notice a *new* private copy growing back in a caller — which is
  // exactly how the two copies came to exist. `!== 22` is the port-elision
  // tell that no other line in either caller has any reason to contain.
  check('neither caller re-grows a private copy of the formula', () => {
    for (const rel of [
      'panels/files-panel.js',
      'features/editor/file-dialog.js',
      'features/editor/file-dialog-view.js',
    ]) {
      const src = fs.readFileSync(path.join(APP, rel), 'utf8');
      assert.ok(!src.includes('!== 22'),
        `${rel} looks like it re-implements the host label; call `
        + 'filesDataService.sessionHostLabel instead');
    }
    const shared = fs.readFileSync(path.join(APP, 'features/files/data-service.js'), 'utf8');
    assert.ok(shared.includes('!== 22'), 'the shared formula is still where it belongs');
  });

  check('scopes: local first, only this window, sorted, hostLabel kept clean', () => {
    const sessions = [
      { key: 'main:7', host: 'zeta', user: 'u', port: 22 },
      { key: 'window-2:1', host: 'other', user: 'u', port: 22 },
      { key: 'main:2', host: 'alpha', user: 'u', port: 22 },
      { key: 'main:5', host: 'alpha', user: 'u', port: 22 },
    ];
    const scopes = fdv._buildScopes(filesData, sessions, 'main', '/home/u');
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
    const scopes = fdv._buildScopes(
      filesData, [{ key: 'main:1', host: 'h', user: 'u', port: 22 }], null, '/home/u',
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
  const h = makeHarness({
    sessions: [{ key: 'main:4', host: 'h1', user: 'ubuntu', port: 22 }],
    listLocal: () => Promise.resolve(entries),
  });
  const m = mountView(h);
  await settle();

  const p = m.parts();
  assert.deepStrictEqual(p.scopes.map(scopeName), ['This Mac', 'ubuntu@h1']);
  assert.strictEqual(p.scopes[0].getAttribute('aria-pressed'), 'true');
  assert.deepStrictEqual(p.rows.map(rowName), ['sub', 'aa.txt', 'zz.txt'], 'dirs first, then name; dotfile hidden');
  assert.strictEqual(p.pathInput.value, '/home/u');
  assert.deepStrictEqual(p.crumbs.map((c) => c.textContent), ['/', 'home', 'u']);

  // The footer is the view's own: Cancel, then the primary — no tl-dialog
  // built these, and in the window host nothing else would.
  assert.deepStrictEqual(p.buttons.map((b) => b.textContent), ['Cancel', 'Open'],
    'the view renders its own Cancel + primary, in that order');
  const openBtn = p.buttons.find((b) => b.textContent === 'Open');
  assert.strictEqual(openBtn.disabled, true, 'Open starts disabled');
  assert.strictEqual(openBtn.getAttribute('aria-disabled'), 'true');

  p.rows[0].fire('click'); // the directory
  assert.strictEqual(openBtn.disabled, true, 'a selected DIRECTORY does not enable Open');
  p.rows[1].fire('click'); // aa.txt
  assert.strictEqual(openBtn.disabled, false, 'a selected FILE enables Open');
  assert.strictEqual(openBtn.getAttribute('aria-disabled'), 'false');

  // And it answers through onResolve — the primary button is wired, not decor.
  openBtn.fire('click');
  assert.strictEqual(m.answers.length, 1, 'exactly one answer');
  assert.strictEqual(m.answers[0].path, '/home/u/aa.txt');
});

await checkAsync('hidden toggle and filter re-render through the model', async () => {
  const entries = [
    { name: '.bashrc', is_dir: false, size: 1, modified: null },
    { name: 'notes.txt', is_dir: false, size: 1, modified: null },
    { name: 'nope.md', is_dir: false, size: 1, modified: null },
  ];
  const h = makeHarness({ listLocal: () => Promise.resolve(entries) });
  const m = mountView(h);
  await settle();

  let p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['nope.md', 'notes.txt']);

  p.hiddenBox.checked = true;
  p.hiddenBox.fire('change');
  p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['.bashrc', 'nope.md', 'notes.txt'], 'hidden toggle reveals dotfiles');

  p.filterInput.value = 'notes';
  p.filterInput.fire('input');
  p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['notes.txt']);
});

// ---------------------------------------------------------------------------
// 2b. Whose window label the sidebar is filtered by
// ---------------------------------------------------------------------------
//
// `sftp_list_dir` resolves its handle with (window_label, pane_id), so the
// sidebar may only offer sessions belonging to the window that will USE the
// pick — the parent. A chooser rendered in a window of its own has a label of
// its own, and asking for it (the old `getCurrentWindowLabel` call) would
// filter every one of the parent's hosts out of the sidebar while quietly
// offering any session that happened to be keyed to the chooser window.
//
// The fixture is discriminating on purpose: one session per label, so the two
// answers are two DIFFERENT sidebars rather than one sidebar and an empty one.

console.log('file dialog: the sidebar is filtered by the PARENT window label');

await checkAsync('sessions are filtered by parentWindowLabel, not by the host window', async () => {
  const h = makeHarness({
    // What `getCurrentWindowLabel` would answer — i.e. what the view would
    // see if it asked for its own label. It is NOT the parent.
    windowLabel: 'window-7',
    sessions: [
      { key: 'main:1', host: 'alpha', user: 'u', port: 22 },
      { key: 'window-7:2', host: 'beta', user: 'u', port: 22 },
    ],
    listLocal: () => Promise.resolve([]),
    listRemote: () => Promise.resolve([]),
  });
  const m = mountView(h, { parentWindowLabel: 'main' });
  await settle();

  assert.deepStrictEqual(m.parts().scopes.map(scopeName), ['This Mac', 'u@alpha'],
    'only the parent window\'s host — u@beta belongs to the window the view is IN');
});

await checkAsync('the same fixture with the other label renders the other sidebar', async () => {
  // The discriminating half: nothing about this fixture is empty or degenerate
  // under either label, so the check above cannot pass by rendering nothing.
  const h = makeHarness({
    windowLabel: 'window-7',
    sessions: [
      { key: 'main:1', host: 'alpha', user: 'u', port: 22 },
      { key: 'window-7:2', host: 'beta', user: 'u', port: 22 },
    ],
    listLocal: () => Promise.resolve([]),
    listRemote: () => Promise.resolve([]),
  });
  const m = mountView(h, { parentWindowLabel: 'window-7' });
  await settle();

  assert.deepStrictEqual(m.parts().scopes.map(scopeName), ['This Mac', 'u@beta']);
});

check('the view never asks for a window label of its own', () => {
  // The behavioural checks above pin the value that is USED; this pins that
  // there is no second, unused path back to the wrong one — a stray
  // getCurrentWindowLabel call inside the view would be dormant under the
  // fixture and live in the chooser window.
  const src = fs.readFileSync(path.join(APP, 'features/editor/file-dialog-view.js'), 'utf8');
  const calls = src.split('\n').filter((line) => /getCurrentWindowLabel\s*\(/.test(line));
  assert.deepStrictEqual(calls, [], 'the view takes deps.parentWindowLabel and asks nothing');
});

// ---------------------------------------------------------------------------
// 3. Enter semantics + the double-enter race
// ---------------------------------------------------------------------------

console.log('file dialog: Enter descends on a directory, opens on a file');
await checkAsync('Enter on a directory descends and lists the child path', async () => {
  const h = makeHarness({
    listLocal: (p) => Promise.resolve(p === '/home/u'
      ? [{ name: 'sub', is_dir: true, size: 0, modified: null }]
      : [{ name: 'deep.txt', is_dir: false, size: 3, modified: null }]),
  });
  const m = mountView(h);
  await settle();

  m.parts().rows[0].fire('click');
  m.parts().list.fire('keydown', { key: 'Enter' });
  await settle();

  assert.deepStrictEqual(h.calls.listLocal, ['/home/u', '/home/u/sub']);
  assert.deepStrictEqual(m.parts().rows.map(rowName), ['deep.txt']);
  assert.strictEqual(m.parts().pathInput.value, '/home/u/sub');
  assert.strictEqual(m.answers.length, 0, 'descending into a directory does not answer');
});

// The exactly-once latch, in the view where it now lives: two Enters on one
// selected file, counted rather than sampled.
await checkAsync('Enter on a file resolves once, even pressed twice', async () => {
  const h = makeHarness({
    listLocal: () => Promise.resolve([{ name: 'a.txt', is_dir: false, size: 5, modified: null }]),
  });
  const m = mountView(h);
  await settle();

  const p = m.parts();
  p.rows[0].fire('click');
  p.list.fire('keydown', { key: 'Enter' });
  p.list.fire('keydown', { key: 'Enter' });
  await settle();

  assert.strictEqual(m.answers.length, 1, 'exactly one resolution');
  assert.strictEqual(m.answers[0].path, '/home/u/a.txt');
  assert.strictEqual(m.answers[0].entry.size, 5);
  assert.strictEqual(m.answers[0].scope.kind, 'local');
});

// The other half of the latch: a pick and then a cancel. The window host will
// tear the view down when it hears the first answer, but the teardown is not
// instant and the user's Escape lands in between — a second onResolve there
// would tell the parent window "cancelled" about a file it has already opened.
await checkAsync('a pick followed by Escape still answers exactly once', async () => {
  const h = makeHarness({
    listLocal: () => Promise.resolve([{ name: 'a.txt', is_dir: false, size: 5, modified: null }]),
  });
  const m = mountView(h);
  await settle();

  const p = m.parts();
  p.rows[0].fire('click');
  p.list.fire('keydown', { key: 'Enter' });
  m.root.fire('keydown', { key: 'Escape' });
  await settle();

  assert.strictEqual(m.answers.length, 1, 'one answer, not two');
  assert.ok(m.answers[0], 'and it is the pick, not the cancel that came after it');
  assert.strictEqual(m.answers[0].path, '/home/u/a.txt');
});

await checkAsync('THE RACE: Enter on a dir then Enter again mid-listing opens nothing', async () => {
  const pending = deferred();
  let nth = 0;
  const h = makeHarness({
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
  const calls = h.calls;
  const m = mountView(h);
  await settle();

  let p = m.parts();
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
  assert.strictEqual(m.answers.length, 0, 'no open fired while the descent was in flight');
  assert.strictEqual(openBtn.disabled, true, 'the selection was dropped synchronously on descent');

  pending.resolve([{ name: 'deep.txt', is_dir: false, size: 1, modified: null }]);
  await settle();
  p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['deep.txt']);
  assert.strictEqual(m.answers.length, 0, 'the landed listing still opened nothing');
});

// The same race on the path-field route, where the selection is the ONLY
// stale state a second Enter could act on — and where acting on it would be a
// real open of a file in a directory the user has already left.
await checkAsync('THE RACE, path-field route: a stale file selection cannot be opened', async () => {
  const pending = deferred();
  const h = makeHarness({
    listLocal: (p) => (p === '/home/u'
      ? Promise.resolve([{ name: 'a.txt', is_dir: false, size: 5, modified: null }])
      : pending.promise),
  });
  const m = mountView(h);
  await settle();

  const p = m.parts();
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

  assert.strictEqual(m.answers.length, 0, 'no file was opened out from under the jump');
  assert.strictEqual(h.calls.openLocal.length, 0);

  pending.resolve([{ name: 'syslog', is_dir: false, size: 1, modified: null }]);
  await settle();
  assert.strictEqual(m.answers.length, 0);
  assert.deepStrictEqual(m.parts().rows.map(rowName), ['syslog']);
});

// ---------------------------------------------------------------------------
// 4. Failure modes: inline error, disconnect mid-browse
// ---------------------------------------------------------------------------

console.log('file dialog: failure modes render inline');
await checkAsync('a rejected listing renders inline, not as a toast, and does not answer', async () => {
  const h = makeHarness({
    listLocal: () => Promise.reject(new Error('Permission denied (os error 13)')),
  });
  const m = mountView(h);
  await settle();

  const p = m.parts();
  assert.strictEqual(p.error.hidden, false, 'the inline error is visible');
  assert.match(p.error.textContent, /Permission denied/);
  assert.strictEqual(p.error.getAttribute('role'), 'alert');
  assert.strictEqual(p.empty.hidden, true, 'the "No matches" placeholder does not double up on the error');
  assert.strictEqual(h.toasts.length, 0, 'no toast was raised');
  assert.strictEqual(m.answers.length, 0, 'and the chooser did not cancel itself');
});

await checkAsync('a host that drops mid-browse errors inline and the scope bar still switches', async () => {
  let remoteUp = true;
  const h = makeHarness({
    sessions: [{ key: 'main:4', host: 'h1', user: 'ubuntu', port: 22 }],
    listLocal: () => Promise.resolve([{ name: 'local.txt', is_dir: false, size: 1, modified: null }]),
    listRemote: () => (remoteUp
      ? Promise.resolve([{ name: 'remote.txt', is_dir: false, size: 2, modified: null }])
      : Promise.reject(new Error('No SSH connection for pane 4'))),
  });
  const m = mountView(h);
  await settle();

  // Switch to the host: realpath('.') resolves ~ once at scope entry.
  m.parts().scopes[1].fire('click');
  await settle();
  assert.deepStrictEqual(h.calls.realpath, [[4, '.']], 'sftp_realpath once, at scope entry');
  assert.deepStrictEqual(m.parts().rows.map(rowName), ['remote.txt']);
  assert.strictEqual(m.parts().error.hidden, true);

  // Host drops; navigate up.
  remoteUp = false;
  m.parts().crumbs[0].fire('click');
  await settle();
  let p = m.parts();
  assert.strictEqual(p.error.hidden, false);
  assert.match(p.error.textContent, /No SSH connection/);
  assert.strictEqual(p.rows.length, 0);
  assert.strictEqual(p.scopes.length, 2, 'the scope bar is still there to switch away with');

  // Switching back to This Mac clears the error and lists again.
  p.scopes[0].fire('click');
  await settle();
  p = m.parts();
  assert.strictEqual(p.error.hidden, true, 'switching scope clears the error');
  assert.deepStrictEqual(p.rows.map(rowName), ['local.txt']);
});

await checkAsync('a scope-entry realpath failure errors inline without answering', async () => {
  const h = makeHarness({
    sessions: [{ key: 'main:9', host: 'h9', user: 'u', port: 22 }],
    listLocal: () => Promise.resolve([]),
    realpath: () => Promise.reject(new Error('sftp channel closed')),
  });
  const m = mountView(h);
  await settle();
  m.parts().scopes[1].fire('click');
  await settle();

  const p = m.parts();
  assert.strictEqual(p.error.hidden, false);
  assert.match(p.error.textContent, /sftp channel closed/);
  assert.strictEqual(m.answers.length, 0);
});

await checkAsync('a failed scope click can be retried by clicking the same scope again', async () => {
  // `enterScope` marks the clicked scope active BEFORE its `resolveScopeStart`
  // settles. If a click handler guard skips re-entering "the scope that is
  // already active" without checking whether that entry actually succeeded,
  // a scope whose first click failed is stuck forever — this is exactly that
  // scenario: the realpath call rejects once, then succeeds.
  let attempt = 0;
  const h = makeHarness({
    sessions: [{ key: 'main:9', host: 'h9', user: 'u', port: 22 }],
    listLocal: () => Promise.resolve([]),
    listRemote: () => Promise.resolve([{ name: 'ok.txt', is_dir: false, size: 3, modified: null }]),
    realpath: () => {
      attempt += 1;
      return attempt === 1
        ? Promise.reject(new Error('sftp channel closed'))
        : Promise.resolve('/home/remote');
    },
  });
  const m = mountView(h);
  await settle();

  m.parts().scopes[1].fire('click');
  await settle();
  let p = m.parts();
  assert.strictEqual(p.error.hidden, false, 'the first attempt failed and shows the error');
  assert.match(p.error.textContent, /sftp channel closed/);
  assert.strictEqual(p.rows.length, 0, 'nothing listed yet');

  // Click the SAME (now-active) scope button again. Before the fix this
  // guard returned immediately because candidate.id === scope.id, and the
  // button never issued a second resolveScopeStart call.
  m.parts().scopes[1].fire('click');
  await settle();

  assert.strictEqual(attempt, 2, 'resolveScopeStart was retried, not skipped');
  assert.deepStrictEqual(h.calls.realpath, [[9, '.'], [9, '.']], 'a second realpath call for the same pane');
  p = m.parts();
  assert.strictEqual(p.error.hidden, true, 'the error clears once the retry succeeds');
  assert.deepStrictEqual(p.rows.map(rowName), ['ok.txt'], 'the listing loads on retry');
});

// ---------------------------------------------------------------------------
// 5. Cancel paths come from tl-dialog (verified, not re-implemented)
// ---------------------------------------------------------------------------

console.log('file dialog: cancel paths');

// The view's OWN Escape. In the window host there is no tl-dialog above it to
// inherit an Escape registration from, so an unhandled Escape would leave the
// user with a chooser window that only the mouse can dismiss.
await checkAsync('Escape on the view root resolves null', async () => {
  const h = makeHarness({ listLocal: () => Promise.resolve([]) });
  const m = mountView(h);
  await settle();

  m.root.fire('keydown', { key: 'Escape' });
  await settle();
  assert.deepStrictEqual(m.answers, [null], 'exactly one answer, and it is a cancel');
});

await checkAsync('Cancel in the view footer resolves null', async () => {
  const h = makeHarness({
    listLocal: () => Promise.resolve([{ name: 'a.txt', is_dir: false, size: 1, modified: null }]),
  });
  const m = mountView(h);
  await settle();

  const p = m.parts();
  p.rows[0].fire('click');                       // even with a file selected
  p.buttons.find((b) => b.textContent === 'Cancel').fire('click');
  await settle();
  assert.deepStrictEqual(m.answers, [null]);
});

// The two checks that used to live here — Escape through tl-dialog's keyboard
// router, and a backdrop mousedown — were about the tl-dialog HOST that
// file-dialog.js no longer has: the chooser is a window, with no overlay and
// no backdrop to click. Their behaviour is covered where it now lives: the
// view's own Escape, two checks up, and `cancel_file_chooser` in
// test_file_dialog_proxy.mjs.

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

// TWO panes on ONE host, deliberately. With a single session the display
// label and the identity label are the same string, so routing the display
// label instead of the identity label would be invisible — the scope bar's
// " (pane N)" suffix only exists when a host is duplicated. This fixture is
// the only thing that can tell `scope.hostLabel` from `scope.label`, and
// getting it wrong means the same remote file lands in a different temp path
// (and so a second editor tab) than the files panel would give it.
await checkAsync('a remote pick routes the CLEAN hostLabel, not the disambiguated button text', async () => {
  const { sandbox, doc, calls } = makeHarness({
    sessions: [
      { key: 'main:1', host: 'h1', user: 'ubuntu', port: 2222 },
      { key: 'main:2', host: 'h1', user: 'ubuntu', port: 2222 },
    ],
    listLocal: () => Promise.resolve([]),
    listRemote: () => Promise.resolve([{ name: 'app.log', is_dir: false, size: 4242, modified: null }]),
    realpath: () => Promise.resolve('/home/ubuntu'),
  });
  const done = sandbox.termlabFileDialog.openForOpen();
  await settle();

  // Precondition: the two buttons are distinguishable, and neither reads as
  // the bare host label — so a routed button caption cannot pass by accident.
  const scopeText = parts(doc).scopes.map(scopeName);
  assert.deepStrictEqual(scopeText,
    ['This Mac', 'ubuntu@h1:2222 (pane 1)', 'ubuntu@h1:2222 (pane 2)']);

  parts(doc).scopes[2].fire('click'); // the SECOND pane on that host
  await settle();
  parts(doc).rows[0].fire('dblclick');
  await done;

  assert.strictEqual(calls.openLocal.length, 0);
  assert.strictEqual(calls.openRemote.length, 1);
  const d = calls.openRemote[0];
  assert.strictEqual(d.paneId, 2, 'the pane id of the scope actually browsed');
  assert.strictEqual(d.remotePath, '/home/ubuntu/app.log');
  assert.strictEqual(d.size, 4242);
  assert.strictEqual(d.hostLabel, 'ubuntu@h1:2222',
    'openRemoteFile gets the identity label — no " (pane N)" suffix');
  assert.ok(!/pane/.test(d.hostLabel), 'the display suffix never reaches editor_temp_path');
});

await checkAsync('a single-session remote pick still routes paneId/hostLabel/size', async () => {
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
  const h = makeHarness({
    listLocal: (p) => (p === '/etc'
      ? Promise.resolve([{ name: 'hosts', is_dir: false, size: 1, modified: null }])
      : Promise.resolve([])),
  });
  const m = mountView(h);
  await settle();
  const p = m.parts();
  p.pathInput.value = '/etc';
  p.pathInput.fire('keydown', { key: 'Enter' });
  await settle();
  assert.ok(h.calls.listLocal.includes('/etc'));
  assert.deepStrictEqual(m.parts().rows.map(rowName), ['hosts']);
});

await checkAsync('a full FILE path in the path field resolves via its parent and opens', async () => {
  const h = makeHarness({
    listLocal: (p) => (p === '/etc'
      ? Promise.resolve([{ name: 'hosts', is_dir: false, size: 7, modified: null }])
      : (p === '/etc/hosts' ? Promise.reject(new Error('Not a directory')) : Promise.resolve([]))),
  });
  const m = mountView(h);
  await settle();
  const p = m.parts();
  p.pathInput.value = '/etc/hosts';
  p.pathInput.fire('keydown', { key: 'Enter' });
  await settle();
  assert.strictEqual(m.answers.length, 1);
  assert.strictEqual(m.answers[0].path, '/etc/hosts');
  assert.strictEqual(m.answers[0].entry.size, 7);
});

// Save mode itself (filename field, New Folder, the existence check and the
// overwrite prompt) is covered by test_editor_save_as.mjs, which drives it
// alongside the rebind it feeds. What matters here is that the entry point is
// live rather than the Task 6 stub, and that it refuses a non-editor pane
// without putting a dialog on screen.
await checkAsync('openForSave is implemented and refuses a non-editor pane', async () => {
  const { sandbox, doc } = makeHarness({});
  assert.strictEqual(
    fs.readFileSync(path.join(APP, 'features/editor/file-dialog.js'), 'utf8').includes('not implemented yet'),
    false,
    'the Task 6 stub is gone',
  );
  const before = doc.body.children.length;
  assert.strictEqual(await sandbox.termlabFileDialog.openForSave(null), null);
  assert.strictEqual(await sandbox.termlabFileDialog.openForSave({ kind: 'terminal' }), null);
  await settle();
  assert.strictEqual(doc.body.children.length, before, 'and opened no dialog for either');
});

check('the public surface is exactly openForOpen + openForSave + cancelForPane (plus test hooks)', () => {
  const { sandbox } = makeHarness({});
  const fd = sandbox.termlabFileDialog;
  assert.strictEqual(typeof fd.openForOpen, 'function');
  assert.strictEqual(typeof fd.openForSave, 'function');
  assert.strictEqual(typeof fd.cancelForPane, 'function');
  // And the view is a module of its own, reachable without file-dialog.js —
  // which is the whole point of the split: the chooser window loads the view
  // and nothing else from this feature.
  assert.strictEqual(typeof sandbox.termlabFileDialogView.build, 'function');
});

check('file-dialog.js builds no chooser DOM of its own', () => {
  // Task 3 replaces the tl-dialog bridge below with a window proxy. Anything
  // that still painted rows here would have to be written a second time.
  const src = fs.readFileSync(path.join(APP, 'features/editor/file-dialog.js'), 'utf8');
  assert.ok(!/tl-filedlg__(row|scope|list|name|newfolder)/.test(src),
    'the chooser\'s DOM belongs to file-dialog-view.js alone');
});

// ---------------------------------------------------------------------------
// 7. One dialog, one QUESTION: the activeChoice short-circuit is per mode
// ---------------------------------------------------------------------------
//
// `activeChoice` hands a second caller the first caller's promise. That is
// right for two callers asking the same thing and catastrophic for two callers
// asking different things: the path a user picks in a Save As chooser is where
// their untitled buffer is going, and handing it to openForOpen as well makes
// one Return rebind the pane AND open a second tab on the same path. Two
// editors on one file, no error, last save silently wins.
//
// ⌘O is a native menu accelerator (AppKit consumes it before the webview), so
// no dialog focus trap prevents the second keystroke reaching here.

console.log('file dialog: cross-mode sharing');

// The file that is sitting in the listing, so an OPEN really has something to
// pick and "no open happened" is a claim with teeth.
const CROSS_MODE_LISTING = () => Promise.resolve([
  { name: 'notes.md', is_dir: false, size: 11, modified: null },
]);

await checkAsync('⌘O while a SAVE chooser is up resolves null instead of sharing its answer', async () => {
  const { sandbox, doc, calls } = makeHarness({ listLocal: CROSS_MODE_LISTING });

  // The untitled buffer's first-save chooser.
  const saving = sandbox.termlabFileDialog._chooseFile({
    mode: 'save', filename: 'Untitled', selectFilename: true,
  });
  await settle();
  const before = doc.body.children.length;
  assert.strictEqual(before, 1, 'precondition: exactly the save chooser is up');

  // ⌘O lands on top of it. Held, not awaited: a shared promise would not
  // settle until the user answers the chooser, so awaiting here would turn the
  // bug into a hang instead of a wrong answer.
  const opening = sandbox.termlabFileDialog.openForOpen();
  await settle();

  assert.strictEqual(doc.body.children.length, before, 'no second modal was stacked');
  assert.deepStrictEqual(calls.openLocal, [], 'and nothing was opened yet');

  // The user answers the dialog they are actually looking at. It still works,
  // and its answer goes only to the save.
  const p = parts(doc);
  const nameInput = p.nameInput;
  nameInput.value = 'draft.md';
  nameInput.fire('input');
  await settle();
  p.buttons.find((b) => b.textContent === 'Save').fire('click');
  const picked = await saving;
  await settle();

  assert.strictEqual(picked.path, '/home/u/draft.md', 'the save got its own answer');
  assert.strictEqual(await opening, null, 'the open was refused, not handed the save\'s answer');
  assert.deepStrictEqual(calls.openLocal, [],
    'and the refused ⌘O never opened a second editor on the saved path');
  assert.strictEqual(doc.body.children.length, 0, 'nothing left on screen');
});

await checkAsync('a second ⌘O while an OPEN chooser is up still shares the one dialog', async () => {
  // The other direction of the same switch, and the reason it is a mode check
  // rather than a blanket refusal: two ⌘O presses are one question asked
  // twice, and both callers want the file the user picks. Sharing keeps a
  // second modal off the screen.
  const { sandbox, doc, calls } = makeHarness({ listLocal: CROSS_MODE_LISTING });

  const first = sandbox.termlabFileDialog.openForOpen();
  await settle();
  assert.strictEqual(doc.body.children.length, 1, 'precondition: the open chooser is up');
  const second = sandbox.termlabFileDialog.openForOpen();
  await settle();
  assert.strictEqual(doc.body.children.length, 1, 'still one dialog — not stacked, not refused');

  const p = parts(doc);
  p.rows[0].fire('click');
  p.buttons.find((b) => b.textContent === 'Open').fire('click');
  const [a, b] = await Promise.all([first, second]);

  assert.ok(a, 'the first caller got the pick');
  assert.ok(b, 'and so did the second — it shared, it was not refused');
  assert.strictEqual(a.path, '/home/u/notes.md');
  assert.strictEqual(b.path, '/home/u/notes.md');
  assert.deepStrictEqual(calls.openLocal, ['/home/u/notes.md', '/home/u/notes.md'],
    'both routed the same pick (the deliberate double-open, closing deferred)');
});

await checkAsync('⌘⇧S while an OPEN chooser is up is refused too, with no second modal', async () => {
  const { sandbox, doc, calls } = makeHarness({ listLocal: CROSS_MODE_LISTING });
  sandbox.termlabEditorService.saveAs = () => {
    throw new Error('saveAs must not be reached for a refused chooser');
  };

  const opening = sandbox.termlabFileDialog.openForOpen();
  await settle();
  assert.strictEqual(doc.body.children.length, 1, 'precondition: the open chooser is up');

  const pane = { kind: 'editor', filePath: null, remote: null, untitledSeq: 1 };
  const saved = await sandbox.termlabFileDialog.openForSave(pane);
  await settle();

  assert.strictEqual(saved, null, 'the save is refused');
  assert.strictEqual(doc.body.children.length, 1, 'and no second modal was stacked');

  // And the open chooser underneath is untouched and still answerable.
  const p = parts(doc);
  p.rows[0].fire('click');
  p.buttons.find((b) => b.textContent === 'Open').fire('click');
  const choice = await opening;
  assert.strictEqual(choice.path, '/home/u/notes.md');
  assert.deepStrictEqual(calls.openLocal, ['/home/u/notes.md']);
});

// ---------------------------------------------------------------------------
// 8. The redesign: places sidebar, detail columns, click-to-sort
// ---------------------------------------------------------------------------
//
// Presentation only — every check below is about WHERE things are rendered and
// in what order. The behaviour checks above (scope click + retry, the race,
// routing, cancel) are unchanged and are what pin the semantics.

console.log('file dialog: sidebar, columns, sorting');

await checkAsync('the sidebar groups scopes under Places and Hosts, and marks the active one', async () => {
  const h = makeHarness({
    sessions: [
      { key: 'main:2', host: 'zeta', user: 'u', port: 22 },
      { key: 'main:1', host: 'alpha', user: 'u', port: 22 },
    ],
    listLocal: () => Promise.resolve([]),
    listRemote: () => Promise.resolve([]),
  });
  const m = mountView(h);
  await settle();

  let p = m.parts();
  assert.deepStrictEqual(p.sectionLabels.map((s) => s.textContent), ['Places', 'Hosts']);
  assert.deepStrictEqual(p.scopes.map(scopeName), ['This Mac', 'u@alpha', 'u@zeta'],
    'one row per scope, still in buildScopes order');
  // Each row sits in the section it belongs to, not merely somewhere in the
  // sidebar: Places holds exactly This Mac, Hosts holds exactly the sessions.
  const rowsOf = (section) => section.querySelectorAll('.tl-filedlg__scope').map(scopeName);
  assert.deepStrictEqual(rowsOf(p.sections[0]), ['This Mac']);
  assert.deepStrictEqual(rowsOf(p.sections[1]), ['u@alpha', 'u@zeta']);

  assert.ok(classesOf(p.scopes[0]).includes('is-active'), 'the local scope starts active');
  assert.strictEqual(p.scopes[0].getAttribute('aria-pressed'), 'true');
  assert.strictEqual(p.scopes[1].getAttribute('aria-pressed'), 'false');
  assert.strictEqual(p.scopes[1].title, 'u@alpha', 'a host row keeps the clean label as its tooltip');

  // The click handler is the same one: switching moves the active marker AND
  // actually enters the scope (the realpath call proves it is not decoration).
  p.scopes[1].fire('click');
  await settle();
  p = m.parts();
  assert.deepStrictEqual(h.calls.realpath, [[1, '.']], 'the sidebar row entered the scope');
  assert.ok(classesOf(p.scopes[1]).includes('is-active'), 'the clicked host row is now active');
  assert.ok(!classesOf(p.scopes[0]).includes('is-active'), 'and This Mac is not');
});

await checkAsync('with no connected sessions the sidebar has Places only', async () => {
  const h = makeHarness({ listLocal: () => Promise.resolve([]) });
  const m = mountView(h);
  await settle();
  const p = m.parts();
  assert.deepStrictEqual(p.sectionLabels.map((s) => s.textContent), ['Places'],
    'no Hosts heading over an empty list');
  assert.deepStrictEqual(p.scopes.map(scopeName), ['This Mac']);
});

// A date far enough in the past that its rendering is the YYYY-MM-DD branch
// whatever year the suite runs in; noon UTC so no time zone can move its year.
const JUNE_2020 = Math.floor(Date.UTC(2020, 5, 15, 12, 0, 0) / 1000);

await checkAsync('rows carry an icon, a size (— for directories) and a modified date', async () => {
  const h = makeHarness({
    listLocal: () => Promise.resolve([
      { name: 'sub', is_dir: true, size: 4096, modified: JUNE_2020 },
      { name: 'big.bin', is_dir: false, size: 1536, modified: JUNE_2020 },
      { name: 'nodate.txt', is_dir: false, size: 0, modified: null },
    ]),
  });
  const m = mountView(h);
  await settle();

  const p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['sub', 'big.bin', 'nodate.txt']);

  // The directory's size is '—' — NOT its 4096-byte inode size, and not the
  // empty string the pre-redesign dialog left there.
  assert.strictEqual(rowSize(p.rows[0]), '—', 'a directory has no meaningful size');
  assert.strictEqual(rowSize(p.rows[1]), '1.5 KB', 'a file is formatted by the model');
  assert.strictEqual(rowSize(p.rows[2]), '0 B');

  assert.match(rowTime(p.rows[1]), /^2020-06-1[45]$/, 'an old date renders YYYY-MM-DD');
  assert.strictEqual(rowTime(p.rows[2]), '—', 'a null modified time renders as —');

  assert.match(rowIcon(p.rows[0]), /folder\.svg$/, 'directories get the folder glyph');
  assert.match(rowIcon(p.rows[1]), /file\.svg$/, 'files get the file glyph');
});

// A DISCRIMINATING fixture: name-asc, size-asc and modified-asc each produce a
// different order, so a header that silently did nothing (or sorted by the
// wrong key) cannot pass any of the three assertions below. The directory is
// here to prove the dirs-first partition survives every key and direction.
const SORT_FIXTURE = () => Promise.resolve([
  { name: 'a.txt', is_dir: false, size: 300, modified: 3000 },
  { name: 'b.txt', is_dir: false, size: 100, modified: 5000 },
  { name: 'c.txt', is_dir: false, size: 200, modified: 1000 },
  { name: 'zdir', is_dir: true, size: 0, modified: 9999 },
]);

await checkAsync('clicking a column header sorts the listing and reflects it in aria-sort', async () => {
  const h = makeHarness({ listLocal: SORT_FIXTURE });
  const m = mountView(h);
  await settle();

  let p = m.parts();
  assert.deepStrictEqual(p.heads.map(colVariant), ['name', 'size', 'time'],
    'three headers: Name, Size, Modified');
  assert.deepStrictEqual(p.rows.map(rowName), ['zdir', 'a.txt', 'b.txt', 'c.txt'],
    'default: name ascending, directories first');
  assert.deepStrictEqual(sortState(p), ['name:ascending', 'size:none', 'time:none']);

  headFor(p, 'size').fire('click');
  p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['zdir', 'b.txt', 'c.txt', 'a.txt'],
    'size ascending — an order neither name nor modified produces');
  assert.deepStrictEqual(sortState(p), ['name:none', 'size:ascending', 'time:none']);

  headFor(p, 'size').fire('click');
  p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['zdir', 'a.txt', 'c.txt', 'b.txt'],
    'a second click on the same header flips the direction');
  assert.deepStrictEqual(sortState(p), ['name:none', 'size:descending', 'time:none']);

  headFor(p, 'time').fire('click');
  p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['zdir', 'c.txt', 'a.txt', 'b.txt'],
    'modified ascending — a third distinct order; a new key starts ascending');
  assert.deepStrictEqual(sortState(p), ['name:none', 'size:none', 'time:ascending']);

  headFor(p, 'name').fire('click');
  p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['zdir', 'a.txt', 'b.txt', 'c.txt']);
  assert.deepStrictEqual(sortState(p), ['name:ascending', 'size:none', 'time:none']);
});

await checkAsync('sort state is per chooser open — a new chooser starts at name ascending', async () => {
  const h = makeHarness({ listLocal: SORT_FIXTURE });
  const first = mountView(h);
  await settle();
  headFor(first.parts(), 'size').fire('click');
  assert.deepStrictEqual(first.parts().rows.map(rowName), ['zdir', 'b.txt', 'c.txt', 'a.txt']);
  first.parts().buttons.find((b) => b.textContent === 'Cancel').fire('click');
  await settle();
  assert.deepStrictEqual(first.answers, [null], 'precondition: the first chooser is done with');

  const m = mountView(h);
  await settle();
  const p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['zdir', 'a.txt', 'b.txt', 'c.txt'],
    'the second open is back to name ascending');
  assert.deepStrictEqual(sortState(p), ['name:ascending', 'size:none', 'time:none']);
});

await checkAsync('re-sorting keeps the selected ENTRY selected, not the selected index', async () => {
  const h = makeHarness({ listLocal: SORT_FIXTURE });
  const m = mountView(h);
  await settle();

  let p = m.parts();
  p.rows[1].fire('click');                       // a.txt, at index 1 under name-asc
  const openBtn = p.buttons.find((b) => b.textContent === 'Open');
  assert.strictEqual(openBtn.disabled, false, 'precondition: a file is selected');

  headFor(p, 'size').fire('click');              // a.txt moves to the last row
  p = m.parts();
  const selected = p.rows.filter((r) => classesOf(r).includes('is-selected'));
  assert.strictEqual(selected.length, 1, 'exactly one row is selected after the re-sort');
  assert.strictEqual(rowName(selected[0]), 'a.txt', 'the same entry, at its new position');
  assert.strictEqual(selected[0].getAttribute('aria-selected'), 'true');
  assert.strictEqual(openBtn.disabled, false, 'and Open is still live for it');
});

// F-1: `list`'s keydown handler is registered on `list` itself, and the
// column header buttons are SIBLINGS of `list` (both children of `.tl-picker
// __box`), not descendants — so a keydown fired while focus sits on a header
// button never reaches that handler. A header click must hand focus back.
await checkAsync('a header click returns focus to the list, not left on the header button (F-1)', async () => {
  const h = makeHarness({ listLocal: SORT_FIXTURE });
  const m = mountView(h);
  await settle();

  let p = m.parts();
  // Focus really is on the header when it is clicked — otherwise this check
  // would pass on whatever focus happened to be there already.
  headFor(p, 'size').focus();
  assert.strictEqual(h.doc.activeElement, headFor(p, 'size'), 'precondition: the header has focus');
  headFor(p, 'size').fire('click');
  p = m.parts();
  assert.strictEqual(h.doc.activeElement, p.list,
    'focus moves to the list so its keydown handler is reachable again');
});

// F-2: `sortBy` preserves the selected ENTRY across a re-sort but never
// scrolled it into view, so a row moved off-screen by the new order looked
// unselected. The fix must NOT go through `select()` (the trap: `select()`
// writes `nameInput.value` in save mode and would clobber a filename the
// user already typed after clicking that row).
await checkAsync('re-sorting scrolls the preserved selection into view (F-2)', async () => {
  const h = makeHarness({ listLocal: SORT_FIXTURE });
  const m = mountView(h);
  await settle();

  let p = m.parts();
  p.rows[1].fire('click');            // a.txt, at index 1 under name-asc
  headFor(p, 'size').fire('click');   // a.txt moves to the last row
  p = m.parts();

  const selected = p.rows.filter((r) => classesOf(r).includes('is-selected'));
  assert.strictEqual(selected.length, 1, 'precondition: still selected after the re-sort');
  assert.strictEqual(rowName(selected[0]), 'a.txt');
  assert.strictEqual(selected[0].scrollIntoViewCalls, 1,
    'the row carrying the preserved selection is scrolled into view exactly once');
});

await checkAsync('re-sorting in save mode does not touch a typed filename — the F-2 trap (F-2)', async () => {
  const h = makeHarness({ listLocal: SORT_FIXTURE });
  const m = mountView(h, { mode: 'save' });
  await settle();

  let p = m.parts();
  p.rows[1].fire('click'); // a.txt: selecting a row in save mode fills the name field
  const nameField = () => p.footerStart.querySelectorAll('.tl-filedlg__name')[0];
  assert.strictEqual(nameField().value, 'a.txt', 'precondition: the click filled the field');

  nameField().value = 'my-typed-name.txt'; // the user then types over it by hand

  headFor(p, 'size').fire('click'); // a re-sort must not call select() and overwrite that
  p = m.parts();
  assert.strictEqual(nameField().value, 'my-typed-name.txt',
    'sortBy must not clobber a filename the user typed after selecting a row');
});

await checkAsync('a re-sort that drops the selected entry leaves nothing selected', async () => {
  const h = makeHarness({ listLocal: SORT_FIXTURE });
  const m = mountView(h);
  await settle();

  let p = m.parts();
  p.rows[1].fire('click');
  const openBtn = p.buttons.find((b) => b.textContent === 'Open');
  p.filterInput.value = 'b.txt';
  p.filterInput.fire('input');
  p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['b.txt']);
  assert.strictEqual(p.rows.filter((r) => classesOf(r).includes('is-selected')).length, 0);
  assert.strictEqual(openBtn.disabled, true, 'Open gates off again with the selection gone');
});

await checkAsync('the hidden toggle lives in the view\'s own footer and still filters', async () => {
  const h = makeHarness({
    listLocal: () => Promise.resolve([
      { name: '.bashrc', is_dir: false, size: 1, modified: null },
      { name: 'notes.txt', is_dir: false, size: 1, modified: null },
    ]),
  });
  const m = mountView(h);
  await settle();

  let p = m.parts();
  assert.ok(p.footer, 'the view renders a footer of its own');
  assert.ok(p.footerStart, 'with a start slot for the options');
  assert.strictEqual(p.footerStart.querySelectorAll('.tl-filedlg__hidden').length, 1,
    'the Hidden toggle sits in the footer, not above the listing');
  p.hiddenBox.checked = true;
  p.hiddenBox.fire('change');
  p = m.parts();
  assert.deepStrictEqual(p.rows.map(rowName), ['.bashrc', 'notes.txt'], 'and still works from there');
});

await checkAsync('save mode puts the name field and New Folder in the footer too', async () => {
  const h = makeHarness({ listLocal: () => Promise.resolve([]) });
  const m = mountView(h, { mode: 'save', filename: 'draft.md' });
  await settle();

  const p = m.parts();
  assert.strictEqual(p.footerStart.querySelectorAll('.tl-filedlg__name').length, 1,
    'the filename field is in the footer');
  assert.strictEqual(p.footerStart.querySelectorAll('.tl-filedlg__newfolder').length, 1,
    'so is New Folder');
  assert.strictEqual(p.footerStart.querySelectorAll('.tl-filedlg__name')[0].value, 'draft.md',
    'still prefilled');
  assert.deepStrictEqual(p.buttons.map((b) => b.textContent), ['Cancel', 'Save'],
    'and the primary button reads Save');
  assert.strictEqual(p.buttons[1].disabled, false,
    'enabled from the prefill, so ⌘⇧S → Return saves under the same name');
});

await checkAsync('open mode has no save controls anywhere in the chooser', async () => {
  const h = makeHarness({ listLocal: () => Promise.resolve([]) });
  const m = mountView(h);
  await settle();
  assert.strictEqual(m.root.querySelectorAll('.tl-filedlg__name').length, 0);
  assert.strictEqual(m.root.querySelectorAll('.tl-filedlg__newfolder').length, 0);
});

// ---------------------------------------------------------------------------
// 9. The chooser WINDOW runtime (app/chooser-window-runtime.js)
// ---------------------------------------------------------------------------
//
// The thin boot module chooser.html loads last. A separate, minimal sandbox
// from makeHarness()'s above — that one plays the PARENT side of the proxy
// (file-dialog.js asking Rust for a chooser and getting `chooser-resolved`
// back); this one plays the CHOOSER side: a bare `#chooser-root`, a stub
// `invoke` that answers `get_chooser_request` / `chooser_ready` /
// `resolve_file_chooser`, and the REAL view/model/data-service the runtime
// actually hosts (stubbing them would test nothing about the wiring).

function makeRuntimeHarness(options) {
  const opts = options || {};
  const doc = makeDocument();
  const sandbox = { console, document: doc, setTimeout, clearTimeout, Promise };
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const root = doc.createElement('div');
  doc.getElementById = (id) => (id === 'chooser-root' ? root : null);

  sandbox.termlabKeyboardRouter = { register: () => () => {} };
  sandbox.toast = { error() {}, success() {} };
  sandbox.utils = { formatSize: (n) => `${n}B`, formatDate: () => '' };

  const invokeLog = [];
  let viewBuilt = false;
  const closeCalls = { count: 0 };

  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        invokeLog.push({ command, args, viewBuiltAtCallTime: viewBuilt });
        if (command === 'get_chooser_request') {
          return opts.requestRejects
            ? Promise.reject(new Error('no pending chooser'))
            : Promise.resolve(opts.request);
        }
        if (command === 'chooser_ready') return Promise.resolve(null);
        if (command === 'resolve_file_chooser') return Promise.resolve(null);
        return Promise.reject(new Error(`chooser window runtime: unexpected invoke ${command}`));
      },
    },
  };

  // window.__TAURI__.window.getCurrentWindow().close() — the fallback used
  // when no close_current_window command exists (grepped commands.rs at
  // HEAD: it does not).
  sandbox.__TAURI__ = {
    window: {
      getCurrentWindow: () => ({ close: () => { closeCalls.count++; } }),
    },
  };

  load(sandbox, 'features/files/data-service.js');
  sandbox.termlabFilesFeatureDataService = Object.assign({}, sandbox.termlabFilesFeatureDataService, {
    getHomeDir: () => Promise.resolve('/home/u'),
    getSessions: () => Promise.resolve([]),
    listLocalDir: () => Promise.resolve([]),
  });
  load(sandbox, 'ui/tl-dialog.js');
  load(sandbox, 'ui/tl-icon.js');
  load(sandbox, 'features/editor/file-dialog-model.js');
  load(sandbox, 'features/editor/file-dialog-view.js');

  // Spy on build() without replacing the module the runtime actually calls
  // into — every check below reads buildCalls, but the REAL build still
  // runs so the view is genuinely on screen for the resolution check.
  const buildCalls = [];
  const realBuild = sandbox.termlabFileDialogView.build;
  sandbox.termlabFileDialogView = Object.assign({}, sandbox.termlabFileDialogView, {
    build(mountRoot, deps) {
      buildCalls.push(deps);
      viewBuilt = true;
      return realBuild(mountRoot, deps);
    },
  });

  load(sandbox, 'chooser-window-runtime.js');

  return { sandbox, doc, root, invokeLog, buildCalls, closeCalls };
}

console.log('file dialog: chooser window runtime');

// Deliberately NOT 'main' (makeHarness's own default parentWindowLabel) and
// not a `chooser-...`-shaped label either — a runtime bug that fell back to
// some ambient default, or that mistakenly forwarded ITS OWN window label
// instead of the request's, would still have to fail this exact-equality
// check rather than accidentally matching a common fixture value.
const RUNTIME_TEST_PARENT_LABEL = 'editor-9k2';

await checkAsync('build() receives the request\'s parentLabel, never any window\'s own label', async () => {
  const request = {
    reqId: 77, mode: 'save', filename: 'notes.txt', selectFilename: true,
    parentLabel: RUNTIME_TEST_PARENT_LABEL,
  };
  const h = makeRuntimeHarness({ request });
  await settle();

  assert.strictEqual(h.buildCalls.length, 1, 'build() is called exactly once');
  assert.strictEqual(h.buildCalls[0].parentWindowLabel, RUNTIME_TEST_PARENT_LABEL,
    'the PARENT label from the request, byte for byte');
});

await checkAsync('chooser_ready is invoked only after the view has been built', async () => {
  const request = {
    reqId: 5, mode: 'open', filename: null, selectFilename: false,
    parentLabel: RUNTIME_TEST_PARENT_LABEL,
  };
  const h = makeRuntimeHarness({ request });
  await settle();

  const readyCall = h.invokeLog.find((entry) => entry.command === 'chooser_ready');
  assert.ok(readyCall, 'chooser_ready was invoked');
  assert.strictEqual(readyCall.viewBuiltAtCallTime, true,
    'the view already existed when chooser_ready was called');
});

await checkAsync('a view resolution forwards {reqId, choice} to resolve_file_chooser', async () => {
  const request = {
    reqId: 42, mode: 'open', filename: null, selectFilename: false,
    parentLabel: RUNTIME_TEST_PARENT_LABEL,
  };
  const h = makeRuntimeHarness({ request });
  await settle();

  // The view's own Escape handler (file-dialog-view.js) is registered on the
  // root element it was handed — exactly `h.root` here, since
  // document.getElementById('chooser-root') is stubbed to return it.
  h.root.fire('keydown', { key: 'Escape' });
  await settle();

  const resolveCall = h.invokeLog.find((entry) => entry.command === 'resolve_file_chooser');
  assert.ok(resolveCall, 'resolve_file_chooser was invoked');
  // Not assert.deepStrictEqual against a plain object literal: resolveCall.args
  // was built inside the vm sandbox, a different JS realm with its own
  // Object.prototype, and deepStrictEqual's strict mode treats that as
  // unequal even when every own property matches. Compare the two fields.
  assert.strictEqual(resolveCall.args.reqId, 42);
  assert.strictEqual(resolveCall.args.choice, null);
});

await checkAsync('a rejected get_chooser_request closes the window and never builds a view', async () => {
  const h = makeRuntimeHarness({ requestRejects: true });
  await settle();

  assert.strictEqual(h.buildCalls.length, 0, 'the view never builds with no request to build it from');
  assert.strictEqual(h.closeCalls.count, 1, 'the window closes itself (no close_current_window command exists)');
  const commands = h.invokeLog.map((entry) => entry.command);
  assert.ok(commands.includes('get_chooser_request'));
  assert.ok(!commands.includes('chooser_ready'), 'never shown');
  assert.ok(!commands.includes('resolve_file_chooser'), 'nothing to resolve');
});

// ---------------------------------------------------------------------------

if (failures) {
  console.error(`file dialog: ${failures} of ${ran} check(s) FAILED`);
  process.exit(1);
}
console.log(`file dialog: all ${ran} checks passed`);
