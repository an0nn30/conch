// Run: node scripts/tests/test_editor_chooser_teardown.mjs
//
// A Save As chooser outliving the pane it is asking about.
//
// The Unsaved Changes prompt STACKS on top of the chooser rather than replacing
// it (tl-dialog nests), so ⌘W while an untitled buffer's first-save chooser is
// on screen puts two modals up at once. "Don't Save" answers the top one, and
// tab-manager.js then destroys the pane underneath — leaving a modal asking
// where to put a buffer that no longer exists, and, because it is still
// file-dialog.js's `activeChoice`, blocking the chooser every OTHER pane would
// open until somebody answers it.
//
// Everything below the DOM is real: ui/tl-dialog.js builds both dialogs,
// core/dialog-service.js wires the prompt's three buttons, the REAL
// features/editor/file-dialog.js builds the chooser, features/editor/
// editor-service.js decides what the answers mean, and the REAL tab-manager.js
// closeTab / pane-manager.js closePane do the teardown. That last part is the
// point: a hand-rolled teardown in a probe can null the pane's view or forget
// to, and the two differ in whether a ghost file is written. Only the shipped
// teardown settles it.
//
// Stubs, and exactly what each returns:
//   * document — a minimal element factory (no jsdom in this repo), with the
//     class-chain querySelectorAll test_file_dialog.mjs uses.
//   * invoke('editor_write_file') — resolves null, as editor_fs.rs's
//     `Result<(), String>` serialises to. Every write is recorded, because
//     "no file was written" is the assertion with the most at stake here.
//   * the CodeMirror view — `dirty` is a plain boolean and the update listener
//     stops firing once it is set, exactly as editor-pane.js implements it.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const MODULES = [
  'app/ui/tl-dialog.js',
  'app/core/dialog-service.js',
  'app/features/editor/tab-label.js',
  'app/features/files/data-service.js',
  'app/features/editor/file-dialog-model.js',
  'app/features/editor/file-dialog.js',
  'app/features/editor/editor-service.js',
  'app/layout/split-tree.js',
  'app/tab-manager.js',
  'app/pane-manager.js',
];

let failures = 0;
let ran = 0;
async function checkAsync(name, fn) {
  ran++;
  try {
    await fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}

const tick = () => new Promise((r) => setImmediate(r));
const settle = async (n = 14) => { for (let i = 0; i < n; i++) await tick(); };

// An orphaned chooser leaves the save that opened it pending FOREVER — nobody
// can answer a dialog that is not there to be answered, and savePane is parked
// on it. Awaiting such a promise directly would hang the run instead of failing
// it, so every check that waits on a save waits with a deadline.
const TIMEOUT = Symbol('timeout');
async function settledWithin(promise, what) {
  // Deliberately NOT unref'd: an unref'd timer lets node exit the moment the
  // only other pending work is a promise that will never settle, which reports
  // as "unsettled top-level await" rather than as this check failing. Cleared
  // on the happy path so a green run costs nothing.
  let timer = null;
  const raced = await Promise.race([
    promise.then((v) => ({ value: v })),
    new Promise((r) => { timer = setTimeout(() => r(TIMEOUT), 1000); }),
  ]);
  clearTimeout(timer);
  assert.notStrictEqual(raced, TIMEOUT, `${what} never settled`);
  return raced.value;
}

// ---------------------------------------------------------------------------
// Minimal DOM — the file-dialog stub's class-chain querySelectorAll, plus the
// dataset / insertBefore / classList.toggle / remove() that tab-manager and
// pane-manager use.
// ---------------------------------------------------------------------------

function classesOf(el) {
  return String(el.className || '').split(' ').filter(Boolean);
}

function matchesSimple(el, token) {
  if (token.charAt(0) === '.') return classesOf(el).includes(token.slice(1));
  return String(el.tagName || '').toLowerCase() === token.toLowerCase();
}

function makeElement(tag, doc) {
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    dataset: {},
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
    removed: false,
    get lastChild() { return this.children.length ? this.children[this.children.length - 1] : null; },
    appendChild(child) {
      this.children.push(child);
      child.parentNode = this;
      child.isConnected = this.isConnected;
      return child;
    },
    insertBefore(child, ref) {
      const idx = ref ? this.children.indexOf(ref) : -1;
      if (idx < 0) this.children.push(child);
      else this.children.splice(idx, 0, child);
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
    remove() { this.removed = true; if (this.parentNode) this.parentNode.removeChild(this); },
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
    click() { el.fire('click'); },
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
    selectCount: 0,
    select() { el.selectCount += 1; if (doc) doc.activeElement = el; },
    contains(node) { let n = node; while (n) { if (n === el) return true; n = n.parentNode; } return false; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 100, bottom: 30, width: 100, height: 30 }; },
  };
  const classes = new Set();
  el.classList = {
    add(c) { classes.add(c); },
    remove(c) { classes.delete(c); },
    toggle(c, on) { if (on) classes.add(c); else classes.delete(c); },
    contains(c) { return classes.has(c) || classesOf(el).includes(c); },
  };
  return el;
}

// ---------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------

function makeApp() {
  const document = {
    activeElement: null,
    addEventListener() {},
    removeEventListener() {},
    getElementById: () => null,
  };
  document.createElement = (tag) => makeElement(tag, document);
  document.body = makeElement('body', document);
  document.body.isConnected = true;

  const intervals = [];
  const sandbox = {
    console, document, setTimeout, clearTimeout, setImmediate, Promise, Map, Set, WeakMap,
    setInterval: (fn, ms) => { const t = setInterval(fn, ms); t.unref(); intervals.push(t); return t; },
    clearInterval: (t) => clearInterval(t),
  };
  sandbox.window = sandbox;
  sandbox.navigator = { platform: 'MacIntel' };
  sandbox.requestAnimationFrame = (fn) => fn();
  sandbox.dispatchEvent = () => true;
  sandbox.CustomEvent = class { constructor(t, i) { this.type = t; Object.assign(this, i); } };
  sandbox.CM6 = {};
  sandbox.utils = { formatSize: (n) => `${n}B`, formatDate: () => '', esc: (s) => String(s) };
  sandbox.termlabKeyboardRouter = { register: () => () => {} };

  const toasts = [];
  sandbox.toast = {
    error: (title, body) => toasts.push({ kind: 'error', title, body }),
    success: (title, body) => toasts.push({ kind: 'success', title, body }),
    warn: () => {}, info: () => {},
  };

  const writes = [];
  const invoke = (command, args) => {
    if (command === 'editor_write_file') {
      writes.push({ path: args.path, contents: args.contents });
      return Promise.resolve(null);
    }
    return Promise.resolve(null);
  };
  sandbox.termlabServices = { tauriClient: { invoke, listen: () => Promise.resolve(() => {}) } };

  // editor-pane.js's contract, minus CodeMirror: `dirty` is a plain boolean and
  // the update listener stops firing once it is set.
  const destroyedViews = [];
  sandbox.termlabEditorPane = {
    createEditorView(hostEl, options) {
      let text = typeof options.doc === 'string' ? options.doc : '';
      let dirty = false;
      return {
        get state() { return { doc: { toString: () => text } }; },
        focus() {},
        destroyed: false,
        termlabResetDirty() { dirty = false; options.onDirtyChange(false); },
        type(t) { text += t; if (dirty) return; dirty = true; options.onDirtyChange(true); },
      };
    },
    destroyEditorView(view) { destroyedViews.push(view); view.destroyed = true; },
    setFontSize() {},
    setLanguage() {},
  };

  vm.createContext(sandbox);
  for (const rel of MODULES) {
    vm.runInContext(fs.readFileSync(path.join(ROOT, rel), 'utf8'), sandbox, { filename: rel });
  }

  // The chooser's IO. Only the invoke-backed calls are stubbed; the real
  // data-service (and so the real host-label formula) stays in place.
  const realData = sandbox.termlabFilesFeatureDataService;
  sandbox.termlabFilesFeatureDataService = Object.assign({}, realData, {
    getHomeDir: () => Promise.resolve('/home/u'),
    getCurrentWindowLabel: () => Promise.resolve('main'),
    getSessions: () => Promise.resolve([]),
    listLocalDir: () => Promise.resolve([]),
    listRemoteDir: () => Promise.resolve([]),
    getRemoteRealPath: () => Promise.resolve('/home/remote'),
    statLocal: () => Promise.reject('No such file or directory'),
    statRemote: () => Promise.reject('No such file'),
    localMkdir: () => Promise.resolve(null),
    remoteMkdir: () => Promise.resolve(null),
  });

  const tabs = new Map();
  const panes = new Map();
  const statuses = [];
  let activeTabId = null;
  let focusedPaneId = null;
  let nextId = 1;

  const tabManager = sandbox.termlabTabManager.create({
    getTabs: () => tabs,
    getPanes: () => panes,
    getActiveTabId: () => activeTabId,
    setActiveTabId: (id) => { activeTabId = id; },
    getFocusedPaneId: () => focusedPaneId,
    setFocusedPaneId: (id) => { focusedPaneId = id; },
    setNextTabLabel: () => {},
    appEl: makeElement('div', document),
    getTermFontSize: () => 13,
    getEditorVimMode: () => false,
    setFocusedPane: (id) => { focusedPaneId = id; },
    fitAndResizeTab: () => {},
    onTabChanged: () => {},
    allPanesInTab: (tabId) => Array.from(panes.values()).filter((p) => p.tabId === tabId).map((p) => p.paneId),
    rememberPluginViewSize: () => {},
    unregisterPaneDnd: () => {},
    notifyTerminalClosed: () => {},
    notifyPluginViewClosed: () => {},
    deletePluginViewPane: () => {},
    showStatus: (m) => statuses.push(m),
    destroyCurrentWindow: async () => {},
    allocateTabId: () => nextId,
    allocatePaneId: () => nextId,
    allocateTabLabel: () => 'Terminal',
    tabBarEl: makeElement('div', document),
    terminalHostEl: makeElement('div', document),
    setWindowTitle: async () => {},
    getLocalPaneCwd: async () => null,
    getLocalPaneProcess: async () => null,
    getHostIdentity: async () => null,
    getWorkspaceDir: async () => null,
    refreshSshSessions: () => {},
    getCurrentWindowLabel: () => 'main',
    normalizeTabTitle: (t) => t,
    makeLeaf: (id) => sandbox.splitTree.makeLeaf(id),
    setupDividerDrag: () => {},
    initTerminal: () => ({ term: null, fitAddon: { proposeDimensions: () => null } }),
    setupTmuxRightClickBridge: () => () => {},
    createPaneResizeObserver: () => null,
    fitAndResizePane: () => {},
    onTerminalData: () => {},
    spawnShell: async () => {},
    spawnDefaultShell: async () => {},
    onSshData: () => {},
    connectSsh: async () => {},
    ensureVaultUnlocked: async () => {},
  });

  const paneManager = sandbox.termlabPaneManager.create({
    getPanes: () => panes,
    getTabs: () => tabs,
    getFocusedPaneId: () => focusedPaneId,
    setFocusedPaneId: (id) => { focusedPaneId = id; },
    getPaneRatio: () => null,
    setPluginViewSize: () => {},
    rebuildTreeDOM: () => {},
    onTerminalFocused: () => {},
    unregisterPaneDnd: () => {},
    notifyTerminalClosed: () => {},
    refreshSshSessions: () => {},
    notifyPluginViewClosed: () => {},
    deletePluginViewPane: () => {},
    closeTab: (id) => tabManager.closeTab(id),
    initTerminal: () => ({ term: null, fitAddon: null }),
    setupTmuxRightClickBridge: () => () => {},
    createPaneResizeObserver: () => null,
    fitAndResizePane: () => {},
    onLocalTerminalData: () => {},
    spawnShell: async () => {},
    allocatePaneId: () => nextId++,
    splitLeaf: (root, src, next, dir) => sandbox.splitTree.splitLeaf(root, src, next, dir),
    openSshChannel: async () => {},
    onSplitPaneData: () => {},
    toastError: (m) => statuses.push(m),
  });

  // The escape hatches manager-compose-runtime.js publishes.
  // allocateTabId and allocatePaneId both read `nextId`, so an editor tab and
  // its pane share an id (as they do in test_editor_unsaved_end_to_end.mjs).
  // Bumping AFTER the call is what keeps a later split from minting a pane id
  // that collides with one already in the tree.
  sandbox.__termlabCreateEditorTab = (options) => {
    const created = tabManager.createEditorTab(options);
    nextId += 1;
    return created;
  };
  sandbox.__termlabPaneAccess = {
    currentPane: () => (focusedPaneId == null ? null : panes.get(focusedPaneId) || null),
    allPanes: () => panes,
    setFocusedPane: (id) => { focusedPaneId = id; },
    activateTab: (id) => tabManager.activateTab(id),
    setTabLabel: (tabId, label, tooltip) => {
      const tab = tabs.get(tabId);
      if (!tab) return false;
      tabManager.setTabLabel(tab.button, label);
      tab.button.title = tooltip;
      tab.label = label;
      return true;
    },
  };

  // cmd+d beside a focused editor: splitPane has no `kind` guard, so a terminal
  // pane joins the editor's tab and the tab now has two leaves.
  function splitBesideEditor(tabId) {
    const tab = tabs.get(tabId);
    const editorPaneId = tab.focusedPaneId;
    const termPaneId = nextId++;
    tab.treeRoot = sandbox.splitTree.splitLeaf(tab.treeRoot, editorPaneId, termPaneId, 'vertical');
    panes.set(termPaneId, {
      paneId: termPaneId, tabId, kind: 'terminal', type: 'local',
      spawned: false, root: makeElement('div', document),
    });
    return termPaneId;
  }

  // The topmost dialog on screen.
  function top() {
    const overlay = document.body.children[document.body.children.length - 1] || null;
    if (!overlay) return null;
    return {
      overlay,
      title: (overlay.querySelectorAll('.tl-dialog__title')[0] || {}).textContent,
      nameInput: overlay.querySelectorAll('.tl-filedlg__name')[0] || null,
      button: (label) => overlay.querySelectorAll('.tl-dialog__footer .tl-btn')
        .find((b) => b.textContent === label) || null,
    };
  }

  return {
    sandbox, document, tabs, panes, toasts, writes, statuses, destroyedViews,
    tabManager, paneManager, splitBesideEditor, top,
    service: sandbox.termlabEditorService,
    dialogCount: () => document.body.children.length,
    focus: (paneId) => { focusedPaneId = paneId; },
    errors: () => toasts.filter((t) => t.kind === 'error'),
    // The one untitled pane the checks below start from, dirty and with its
    // first-save chooser already on screen. Returns the pane and the pending
    // saveActiveEditor promise.
    async untitledWithChooserUp(text) {
      this.service.openUntitled();
      await settle();
      const pane = [...panes.values()].find((p) => p.kind === 'editor' && p.untitledSeq);
      if (text) pane.view.type(text);
      focusedPaneId = pane.paneId;
      const saving = this.service.saveActiveEditor();
      await settle();
      return { pane, saving };
    },
  };
}

// ---------------------------------------------------------------------------

console.log('editor chooser teardown: ⌘W over an open chooser');

await checkAsync('Don\'t Save closes the chooser with the pane — no dialog left, no file written', async () => {
  const app = makeApp();
  const { pane, saving } = await app.untitledWithChooserUp('my draft');
  assert.strictEqual(app.top().title, 'Save File As', 'precondition: the chooser is up');

  // ⌘W. tab-manager's closeTab asks about the dirty editor, and the prompt
  // stacks on top of the chooser rather than replacing it.
  const closing = app.tabManager.closeTab(pane.tabId);
  await settle();
  assert.strictEqual(app.dialogCount(), 2, 'precondition: the prompt STACKED over the chooser');
  assert.strictEqual(app.top().title, 'Unsaved Changes');

  app.top().button("Don't Save").click();
  await closing;
  await settle();

  assert.strictEqual(app.panes.has(pane.paneId), false, 'the pane is gone');
  assert.strictEqual(app.dialogCount(), 0,
    'and its chooser went with it — no modal left asking where to put a destroyed buffer');
  assert.deepStrictEqual(app.writes, [], 'nothing was written');
  assert.strictEqual(pane.filePath, null, 'and the dead pane was not rebound');

  // The save the user started settles — it does not sit pending on a dialog
  // that no longer exists — and reports nothing: they answered "don't save".
  await settledWithin(saving, 'the abandoned save');
  assert.deepStrictEqual(app.errors(), []);
});

await checkAsync('and the NEXT pane can open its own chooser — activeChoice was released', async () => {
  // The sharpest consequence of leaving the orphan up: it stays
  // file-dialog.js's `activeChoice`, and openForSave refuses outright while one
  // is set. Every later ⌘S would do nothing at all, silently.
  const app = makeApp();
  const { pane, saving } = await app.untitledWithChooserUp('my draft');

  const closing = app.tabManager.closeTab(pane.tabId);
  await settle();
  app.top().button("Don't Save").click();
  await closing;
  await settledWithin(saving, 'the abandoned save');
  await settle();

  // A fresh buffer, a fresh ⌘S.
  app.service.openUntitled();
  await settle();
  const next = [...app.panes.values()].find((p) => p.kind === 'editor');
  next.view.type('second buffer');
  app.focus(next.paneId);
  const savingNext = app.service.saveActiveEditor();
  await settle();

  assert.strictEqual(app.dialogCount(), 1, 'the second buffer gets a chooser of its own');
  assert.strictEqual(app.top().title, 'Save File As');
  const d = app.top();
  d.nameInput.value = 'second.md';
  d.nameInput.fire('input');
  await settle();
  d.button('Save').click();
  await settle();
  await settledWithin(savingNext, "the second buffer's save");

  assert.deepStrictEqual(app.writes.map((w) => w.path), ['/home/u/second.md'],
    'and it really saves — one write, for the pane that is still alive');
  assert.strictEqual(next.filePath, '/home/u/second.md');
  assert.strictEqual(next.dirty, false);
  assert.deepStrictEqual(app.errors(), []);
});

await checkAsync('Save at the stacked prompt still joins the chooser and completes the close', async () => {
  // The happy variant, and the reason the cancel is bound to the TEARDOWN
  // rather than to the prompt appearing: here the pane is not destroyed until
  // the save has finished, and the chooser must survive to be answered.
  const app = makeApp();
  const { pane, saving } = await app.untitledWithChooserUp('my draft');

  const closing = app.tabManager.closeTab(pane.tabId);
  await settle();
  assert.strictEqual(app.dialogCount(), 2, 'prompt stacked over the chooser');

  app.top().button('Save').click();
  await settle();
  assert.strictEqual(app.top().title, 'Save File As',
    'the prompt closed and the chooser underneath is reachable again');

  const d = app.top();
  d.nameInput.value = 'kept.md';
  d.nameInput.fire('input');
  await settle();
  d.button('Save').click();

  const raced = await Promise.race([
    closing.then(() => 'closed'),
    new Promise((r) => setTimeout(() => r('TIMEOUT'), 1500)),
  ]);
  await settledWithin(saving, 'the joined save');
  await settle();

  assert.strictEqual(raced, 'closed', 'the close must not hang on the chooser it is waiting for');
  assert.deepStrictEqual(app.writes.map((w) => w.path), ['/home/u/kept.md'], 'exactly one write');
  assert.strictEqual(app.writes[0].contents, 'my draft');
  assert.strictEqual(pane.filePath, '/home/u/kept.md', 'the pane was rebound before it was torn down');
  assert.strictEqual(app.dialogCount(), 0, 'nothing left on screen');
  assert.strictEqual(app.panes.has(pane.paneId), false, 'and the tab closed');
  assert.deepStrictEqual(app.errors(), []);
});

console.log('editor chooser teardown: cmd+shift+w over an open chooser (the split path)');

await checkAsync('closing a SPLIT editor pane cancels its chooser too', async () => {
  // pane-manager.js's own teardown, reachable on default bindings: cmd+d beside
  // a focused editor, then cmd+shift+w. It never delegates to closeTab, so
  // nothing else on that path would ever close the chooser.
  const app = makeApp();
  const { pane, saving } = await app.untitledWithChooserUp('my draft');
  app.splitBesideEditor(pane.tabId);
  assert.strictEqual(app.top().title, 'Save File As', 'precondition: the chooser is up');

  const closing = app.paneManager.closePane(pane.paneId);
  await settle();
  assert.strictEqual(app.dialogCount(), 2, 'the prompt stacked over the chooser here too');
  app.top().button("Don't Save").click();
  await closing;
  await settle();
  await settledWithin(saving, 'the abandoned save');

  assert.strictEqual(app.panes.has(pane.paneId), false, 'the pane is gone');
  assert.strictEqual(app.dialogCount(), 0, 'and so is its chooser');
  assert.deepStrictEqual(app.writes, [], 'nothing was written');
  assert.deepStrictEqual(app.errors(), []);
});

console.log('editor chooser teardown: the second half of the defence');

await checkAsync('saveAs writes nothing for a pane whose view the teardown nulled', async () => {
  // Why an orphaned chooser does not produce a ghost FILE, only an orphaned
  // modal: both teardown paths set `pane.view = null` (tab-manager.js:445,
  // pane-manager.js:279) and saveAs's first line refuses a pane without one.
  //
  // Pinned here because the two halves are easy to break independently: this is
  // the guard that decides whether the orphan can still write, and the cancel
  // above is what stops the orphan existing. A probe that tears a pane down by
  // hand without nulling the view sees a ghost file written; the shipped
  // teardown does not, and this is the line that makes the difference.
  const app = makeApp();
  app.service.openUntitled();
  await settle();
  const pane = [...app.panes.values()].find((p) => p.kind === 'editor');
  pane.view.type('my draft');

  const view = pane.view;
  pane.view = null;                       // exactly what both teardowns do

  await app.service.saveAs(pane, { scope: 'local', path: '/home/u/ghost.md' });

  assert.deepStrictEqual(app.writes, [], 'no ghost file');
  assert.strictEqual(pane.filePath, null, 'and the dead pane was not rebound');
  assert.deepStrictEqual(app.errors(), [], 'and nothing was reported — there is no user here');

  // The same call on the live pane really does write, so the check above is
  // about the missing view and not about a target the harness cannot write.
  pane.view = view;
  await app.service.saveAs(pane, { scope: 'local', path: '/home/u/real.md' });
  assert.deepStrictEqual(app.writes.map((w) => w.path), ['/home/u/real.md']);
});

console.log('editor chooser teardown: the cancel is per pane');

await checkAsync('tearing down one pane leaves ANOTHER pane\'s chooser alone', async () => {
  // cancelForPane keys on the chooser's subject, not on "a chooser exists".
  // A blanket cancel would close the dialog a different, living pane is waiting
  // on every time any editor tab closed.
  const app = makeApp();
  const { pane: a, saving: savingA } = await app.untitledWithChooserUp('buffer a');

  // A second, clean editor tab — nothing to prompt about, so its close is a
  // straight teardown.
  app.service.openUntitled();
  await settle();
  const b = [...app.panes.values()].find((p) => p.kind === 'editor' && p !== a);
  assert.ok(b, 'a second editor pane exists');

  const closing = app.tabManager.closeTab(b.tabId);
  await closing;
  await settle();

  assert.strictEqual(app.panes.has(b.paneId), false, 'the clean tab closed');
  assert.strictEqual(app.dialogCount(), 1, "and pane A's chooser is untouched");
  assert.strictEqual(app.top().title, 'Save File As');

  const d = app.top();
  d.nameInput.value = 'a.md';
  d.nameInput.fire('input');
  await settle();
  d.button('Save').click();
  await settle();
  await settledWithin(savingA, "pane A's save");

  assert.deepStrictEqual(app.writes.map((w) => w.path), ['/home/u/a.md'], 'and it still saves');
  assert.strictEqual(a.filePath, '/home/u/a.md');
  assert.deepStrictEqual(app.errors(), []);
});

// ---------------------------------------------------------------------------

if (failures) {
  console.error(`editor chooser teardown: ${failures} of ${ran} check(s) FAILED`);
  process.exit(1);
}
console.log(`editor chooser teardown: all ${ran} checks passed`);
