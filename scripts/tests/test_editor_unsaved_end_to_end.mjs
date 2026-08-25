// Run: node scripts/tests/test_editor_unsaved_end_to_end.mjs
//
// The five unsaved-changes scenarios from the light-editor task-7 brief,
// each as its own check, driven through every real module at once in a single
// window: app/ui/tl-dialog.js builds the prompt, core/dialog-service.js wires
// its three buttons, features/editor/editor-service.js decides what an answer
// means, tab-manager.js's closeTab() acts on it, and event-wiring-runtime.js
// answers Rust's close/quit requests. The test clicks the actual <button>
// elements; no part of the prompt or the decision is stubbed.
//
// Stubs, and exactly what each returns:
//   * document — a minimal element factory (no jsdom in this repo). Assigning
//     textContent escapes into innerHTML, as a real element does.
//   * invoke('editor_write_file') — resolves `null` on success, which is what
//     editor_fs.rs's `Result<(), String>` serialises to; on the forced-failure
//     check it rejects with the error string editor_fs.rs actually produces,
//     verbatim from a real chmod-500 run:
//       "Could not write <path>: Permission denied (os error 13)"
//   * invoke(close-guard commands) — resolves undefined, as the three void
//     #[tauri::command]s in close_guard.rs do.
//   * listenOnCurrentWindow — resolves an unlisten function, as
//     @tauri-apps/api's Window.listen does.
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
  'app/features/editor/lsp-state.js',
  'app/features/editor/lsp-bridge.js',
  'app/features/editor/editor-service.js',
  'app/layout/split-tree.js',
  'app/tab-manager.js',
  'app/pane-manager.js',
  'app/event-wiring-runtime.js',
].map((rel) => path.join(ROOT, rel));

// The real message editor_fs.rs returns for a save into a 0500 directory.
const PERMISSION_DENIED = (p) => `Could not write ${p}: Permission denied (os error 13)`;

function makeElement(tag, doc) {
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    dataset: {},
    style: {},
    title: '',
    innerHTML: '',
    disabled: false,
    tabIndex: 0,
    isConnected: false,
    removed: false,
    appendChild(c) { this.children.push(c); c.parentNode = this; c.isConnected = this.isConnected; return c; },
    insertBefore(c, before) {
      const index = this.children.indexOf(before);
      if (index < 0) return this.appendChild(c);
      this.children.splice(index, 0, c); c.parentNode = this; c.isConnected = this.isConnected; return c;
    },
    removeChild(c) {
      const i = this.children.indexOf(c);
      if (i >= 0) this.children.splice(i, 1);
      c.parentNode = null; c.isConnected = false; return c;
    },
    remove() { this.removed = true; if (this.parentNode) this.parentNode.removeChild(this); },
    setAttribute(n, v) { attrs.set(n, String(v)); },
    getAttribute(n) { return attrs.has(n) ? attrs.get(n) : null; },
    removeAttribute(n) { attrs.delete(n); },
    hasAttribute(n) { return attrs.has(n); },
    addEventListener(n, fn) { if (!listeners.has(n)) listeners.set(n, []); listeners.get(n).push(fn); },
    removeEventListener(n, fn) {
      const a = listeners.get(n) || [];
      const i = a.indexOf(fn);
      if (i >= 0) a.splice(i, 1);
    },
    click() { for (const fn of (listeners.get('click') || []).slice()) fn({ target: this }); },
    querySelectorAll() { return []; },
    querySelector() { return null; },
    contains(node) { let n = node; while (n) { if (n === this) return true; n = n.parentNode; } return false; },
    focus() { doc.activeElement = this; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 100, bottom: 30, width: 100, height: 30 }; },
    classList: {
      _set: new Set(),
      add(c) { this._set.add(c); },
      remove(c) { this._set.delete(c); },
      toggle(c, on) { if (on) this._set.add(c); else this._set.delete(c); },
      contains(c) { return this._set.has(c); },
    },
  };
  Object.defineProperty(el, 'className', {
    get() { return Array.from(el.classList._set).join(' '); },
    set(v) { el.classList._set = new Set(String(v).split(' ').filter(Boolean)); },
  });
  let text = '';
  Object.defineProperty(el, 'textContent', {
    get() { return text; },
    set(v) {
      text = String(v == null ? '' : v);
      el.innerHTML = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    },
  });
  return el;
}

function makeRouterStub() {
  let nextId = 1;
  const handlers = new Map();
  return {
    register(options) {
      const id = nextId++;
      handlers.set(id, {
        order: id,
        priority: options && typeof options.priority === 'number' ? options.priority : 0,
        isActive: options && typeof options.isActive === 'function' ? options.isActive : null,
        onKeyDown: options && typeof options.onKeyDown === 'function' ? options.onKeyDown : null,
      });
      return () => handlers.delete(id);
    },
    dispatchEscape() {
      const sorted = Array.from(handlers.values()).sort((a, b) => (
        a.priority !== b.priority ? b.priority - a.priority : a.order - b.order
      ));
      for (const e of sorted) {
        if (e.isActive && !e.isActive()) continue;
        if (e.onKeyDown && e.onKeyDown({ key: 'Escape' }) === true) return true;
      }
      return false;
    },
  };
}

// editor-pane.js:43-48, 84-87 — a plain boolean flag, and no further callbacks
// once it is set.
function makeView(onDirtyChange) {
  let doc = '';
  let dirty = false;
  return {
    get state() { return { doc: { toString: () => doc } }; },
    focus() {},
    termlabResetDirty() { dirty = false; onDirtyChange(false); },
    isDirty: () => dirty,
    type(text) {
      doc += text;
      if (dirty) return;
      dirty = true;
      onDirtyChange(true);
    },
  };
}

function makeApp() {
  const timers = [];
  const sandbox = {
    console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap,
    setInterval: (fn, ms) => { const t = setInterval(fn, ms); t.unref(); timers.push(t); return t; },
    clearInterval: (t) => clearInterval(t),
  };
  sandbox.window = sandbox;
  const document = {
    activeElement: null,
    createElement: (tag) => makeElement(tag, document),
    getElementById: () => null,
    addEventListener() {},
    removeEventListener() {},
  };
  document.body = makeElement('body', document);
  document.body.isConnected = true;
  sandbox.document = document;
  sandbox.navigator = { platform: 'MacIntel' };
  sandbox.requestAnimationFrame = (fn) => fn();
  sandbox.dispatchEvent = () => true;
  sandbox.CustomEvent = class { constructor(t, i) { this.type = t; Object.assign(this, i); } };
  sandbox.termlabKeyboardRouter = makeRouterStub();
  sandbox.CM6 = {};

  const toasts = [];
  sandbox.toast = {
    error: (t, b) => toasts.push([t, b]),
    warn: () => {}, info: () => {}, success: () => {},
  };

  const destroyedViews = [];
  sandbox.termlabEditorPane = {
    createEditorView(_host, options = {}) {
      const doc = { value: options.doc || '', toString() { return this.value; } };
      let paneView = null;
      paneView = {
        state: { doc },
        focus() {},
        termlabResetDirty() { if (options.onDirtyChange) options.onDirtyChange(false); },
        termlabSetReadOnly(value) { paneView.readOnly = value; },
        type(text) {
          const from = doc.value.length;
          doc.value += text;
          if (options.onDirtyChange) options.onDirtyChange(true);
          if (options.onDocumentTransaction) {
            options.onDocumentTransaction({
              docChanged: true,
              changes: { iterChanges(fn) { fn(from, from, from, doc.value.length, { toString: () => text }); } },
            });
          }
        },
      };
      return paneView;
    },
    destroyEditorView: (v) => destroyedViews.push(v),
    setFontSize() {},
    setLanguage() {},
  };

  // The "file system": path -> contents, as the disk would look.
  const disk = new Map();
  const invocations = [];
  let denyWrites = false;
  let denyDocumentClose = false;
  let rejectDocumentCloseId = null;
  let delayDocumentOpen = false;
  let resolveDocumentOpen = null;
  const invoke = (command, args) => {
    invocations.push({ command, args });
    if (command === 'editor_write_file') {
      if (denyWrites) return Promise.reject(PERMISSION_DENIED(args.path));
      disk.set(args.path, args.contents);
      return Promise.resolve(null);
    }
    if (
      command === 'lsp_close_document'
      && (denyDocumentClose || args.documentId === rejectDocumentCloseId)
    ) {
      return Promise.reject(new Error('manager outbox overloaded'));
    }
    if (
      command === 'lsp_close_documents'
      && (denyDocumentClose || (args.documentIds || []).includes(rejectDocumentCloseId))
    ) {
      return Promise.reject(new Error('manager outbox overloaded'));
    }
    if (command === 'editor_reserve_document') {
      return Promise.resolve({ kind: 'reserved', reservationId: 'reservation', canonicalPath: args.path });
    }
    if (command === 'editor_read_file') return Promise.resolve(disk.get(args.path) || '');
    if (command === 'lsp_open_document') {
      const opened = {
        documentId: 'doc-real-tab', version: 1, projectCandidates: [],
        status: {
          revision: 1, documentId: 'doc-real-tab', sessionId: null, adapterId: 'typescript',
          projectRootUri: null, state: 'choosingProject', message: null, capabilities: {},
          errorCount: 0, warningCount: 0,
        },
      };
      if (delayDocumentOpen) {
        return new Promise((resolve, reject) => {
          resolveDocumentOpen = (error) => (error ? reject(error) : resolve(opened));
        });
      }
      return Promise.resolve(opened);
    }
    if (command === 'lsp_resync_document') {
      return Promise.resolve({ documentId: args.documentId, version: args.version, status: null });
    }
    return Promise.resolve(undefined);
  };
  sandbox.termlabServices = { tauriClient: { invoke } };

  for (const file of MODULES) {
    if (!vm.isContext(sandbox)) vm.createContext(sandbox);
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }

  const tabs = new Map();
  const panes = new Map();
  const statuses = [];
  let activeTabId = null;
  let nextId = 1;
  let destroyedWindow = 0;

  const tabManager = sandbox.termlabTabManager.create({
    getTabs: () => tabs,
    getPanes: () => panes,
    getActiveTabId: () => activeTabId,
    setActiveTabId: (id) => { activeTabId = id; },
    getFocusedPaneId: () => null,
    setFocusedPaneId: () => {},
    setNextTabLabel: () => {},
    appEl: makeElement('div', document),
    getTermFontSize: () => 13,
    setFocusedPane: () => {},
    fitAndResizeTab: () => {},
    onTabChanged: () => {},
    allPanesInTab: (tabId) => Array.from(panes.values()).filter((p) => p.tabId === tabId).map((p) => p.paneId),
    rememberPluginViewSize: () => {},
    unregisterPaneDnd: () => {},
    notifyTerminalClosed: () => {},
    notifyPluginViewClosed: () => {},
    deletePluginViewPane: () => {},
    showStatus: (m) => statuses.push(m),
    destroyCurrentWindow: async () => { destroyedWindow += 1; },
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
    makeLeaf: (paneId) => sandbox.splitTree.makeLeaf(paneId),
    setupDividerDrag: () => {},
    normalizeTabTitle: (value) => value,
  });

  let focusedPaneId = null;
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

  // What cmd+d does to a focused editor: splitPane has no `kind` guard, so a
  // terminal pane is added beside the editor and the tab now has two leaves.
  function splitBesideEditor(editorTabId) {
    const tab = tabs.get(editorTabId);
    const editorPaneId = tab.focusedPaneId;
    const termPaneId = nextId++;
    tab.treeRoot = sandbox.splitTree.splitLeaf(tab.treeRoot, editorPaneId, termPaneId, 'vertical');
    panes.set(termPaneId, {
      paneId: termPaneId, tabId: editorTabId, kind: 'terminal', type: 'local',
      spawned: false, root: makeElement('div', document),
    });
    return termPaneId;
  }

  // The escape hatch manager-compose-runtime.js publishes for exactly this.
  sandbox.__termlabPaneAccess = {
    currentPane: () => panes.values().next().value || null,
    allPanes: () => panes,
    setFocusedPane: () => {},
    activateTab: (id) => tabManager.activateTab(id),
  };
  sandbox.__termlabCreateEditorTab = (options) => tabManager.createEditorTab(options);
  sandbox.termlabLspBridge.configure({
    windowLabel: 'main',
    paneAccess: sandbox.__termlabPaneAccess,
    onReservationFailed: (canonicalPath) => sandbox.termlabEditorService.openLocalFile(canonicalPath),
  });

  // Open a titled file and type into it, as the user would.
  function openTitledFileAndType(fileName, text) {
    const id = nextId++;
    const filePath = `/home/u/docs/${fileName}`;
    disk.set(filePath, '');
    const button = makeElement('button', document);
    const containerEl = makeElement('div', document);
    tabs.set(id, {
      id, label: fileName, button, containerEl, focusedPaneId: id,
      treeRoot: sandbox.splitTree.makeLeaf(id),
    });
    const pane = {
      paneId: id, tabId: id, kind: 'editor', filePath, dirty: false, remote: null,
      root: makeElement('div', document),
    };
    pane.view = makeView((d) => { pane.dirty = d; });
    panes.set(id, pane);
    if (activeTabId === null) activeTabId = id;
    if (text) pane.view.type(text);
    return { tabId: id, pane, filePath, button, containerEl };
  }

  const listeners = new Map();
  const wiring = sandbox.termlabEventWiringRuntime.create({
    invoke,
    listen: () => Promise.resolve(() => {}),
    listenOnCurrentWindow: (name, handler) => { listeners.set(name, handler); return Promise.resolve(() => {}); },
    currentWindowLabel: 'main',
    terminalHostEl: null, tabBarEl: null, tabs, panes,
    getActiveTabId: () => activeTabId,
    getFocusedPaneId: () => null,
    getCurrentPane: () => null,
    getCurrentTab: () => null,
    closeTab: (id) => tabManager.closeTab(id),
    createTab: () => {}, closePane: () => {}, splitPane: () => {},
    renameActiveTab: () => {}, setFocusedPane: () => {}, startTabRename: () => {},
    fitAndResizeTab: () => {}, debouncedSaveLayout: () => {},
    showStatus: (m) => statuses.push(m),
    isTextInputTarget: () => false, writeTextToCurrentPane: () => {}, pasteIntoCurrentPane: () => {},
    openCommandPalette: () => {}, closeCommandPalette: () => {}, isCommandPaletteOpen: () => false,
    refocusActiveTerminal: () => {}, terminalRuntime: {}, shortcutDebugEnabled: false,
    getZoom: () => 1, setZoom: () => {}, getThemeState: () => ({}), setThemeState: () => {},
    getTermConfigState: () => ({}), setTermConfigState: () => {}, fontFallbacks: [],
    activateTab: () => {},
  });

  function findButton(label) {
    let found = null;
    const walk = (node) => {
      for (const c of node.children) {
        if (!found && c.tagName === 'BUTTON' && c.textContent === label) found = c;
        walk(c);
      }
    };
    walk(document.body);
    return found;
  }

  return {
    sandbox, tabs, panes, disk, toasts, statuses, destroyedViews, invocations, listeners,
    tabManager, paneManager, wiring, openTitledFileAndType, splitBesideEditor, findButton,
    focusPane: (id) => paneManager.setFocusedPane(id),
    dialogCount: () => sandbox.tlDialog.count(),
    denyWrites: (on) => { denyWrites = on; },
    denyDocumentClose: (on) => { denyDocumentClose = on; },
    rejectDocumentClose: (documentId) => { rejectDocumentCloseId = documentId; },
    delayDocumentOpen: (on) => { delayDocumentOpen = on; },
    resolveDocumentOpen: (error) => resolveDocumentOpen && resolveDocumentOpen(error),
    destroyedWindowCount: () => destroyedWindow,
    appClosed: () => invocations.some((i) => i.command === 'quit_vote' && i.args.allow === true),
  };
}

const tick = () => new Promise((r) => setTimeout(r, 0));
const results = [];
const check = (name, fn) => results.push({ name, fn });

// === Step 7 check 1: close the tab ========================================
check('1a. tab close prompts, and Cancel leaves the tab open and still dirty', async () => {
  const app = makeApp();
  const { tabId, pane, filePath, button, containerEl } = app.openTitledFileAndType('file-1.txt', 'hello');

  const closing = app.tabManager.closeTab(tabId);
  await tick();
  assert.strictEqual(app.dialogCount(), 1, 'the prompt appeared');

  app.findButton('Cancel').click();
  await closing;

  assert.strictEqual(app.tabs.has(tabId), true, 'the tab stayed open');
  assert.strictEqual(app.panes.has(pane.paneId), true, 'and its pane still exists');
  assert.strictEqual(pane.dirty, true, 'and it is still dirty');
  assert.ok(pane.view, 'and its editor view was not destroyed');
  assert.strictEqual(app.destroyedViews.length, 0);
  assert.strictEqual(button.removed, false);
  assert.strictEqual(containerEl.removed, false);
  assert.strictEqual(app.disk.get(filePath), '', 'nothing was written');
});

check("1b. Don't Save closes the tab and leaves the file unchanged on disk", async () => {
  const app = makeApp();
  const { tabId, filePath } = app.openTitledFileAndType('file-1.txt', 'hello');

  const closing = app.tabManager.closeTab(tabId);
  await tick();
  app.findButton("Don't Save").click();
  await closing;

  assert.strictEqual(app.tabs.has(tabId), false, 'the tab closed');
  assert.strictEqual(app.disk.get(filePath), '', 'the file on disk is unchanged');
  assert.strictEqual(app.destroyedViews.length, 1, 'the editor view was disposed');
});

check('1c. Save closes the tab and the file has the text', async () => {
  const app = makeApp();
  const { tabId, filePath } = app.openTitledFileAndType('file-1.txt', 'hello');

  const closing = app.tabManager.closeTab(tabId);
  await tick();
  app.findButton('Save').click();
  await closing;

  assert.strictEqual(app.tabs.has(tabId), false, 'the tab closed');
  assert.strictEqual(app.disk.get(filePath), 'hello', 'and the text reached the file');
  assert.strictEqual(app.toasts.length, 0, 'with no error');
});

// === Step 7 check 2: close the window =====================================
check('2. two dirty files prompt in sequence on window close; Cancel on the second keeps the window', async () => {
  const app = makeApp();
  const a = app.openTitledFileAndType('file-1.txt', 'first');
  const b = app.openTitledFileAndType('file-2.txt', 'second');
  await app.wiring.init();

  // Rust prevented the close and asked.
  app.listeners.get('window-close-requested')({});
  await tick();
  assert.strictEqual(app.dialogCount(), 1, 'first prompt');
  app.findButton("Don't Save").click();
  await tick();
  assert.strictEqual(app.dialogCount(), 1, 'second prompt, in sequence');
  app.findButton('Cancel').click();
  await tick();

  const answer = app.invocations.filter((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answer.length, 1, 'exactly one answer went back to Rust');
  assert.strictEqual(answer[0].args.allow, false, 'and it says: do not close');
  assert.strictEqual(app.tabs.size, 2, 'both tabs are still open');
  assert.strictEqual(b.pane.dirty, true, 'and the cancelled file keeps its text');
  assert.strictEqual(a.pane.dirty, true);
  assert.strictEqual(app.destroyedWindowCount(), 0);
});

check('2b. answering every prompt lets the window close', async () => {
  const app = makeApp();
  app.openTitledFileAndType('file-1.txt', 'first');
  const b = app.openTitledFileAndType('file-2.txt', 'second');
  await app.wiring.init();

  app.listeners.get('window-close-requested')({});
  await tick();
  app.findButton("Don't Save").click();
  await tick();
  app.findButton('Save').click();
  await tick();

  const answer = app.invocations.filter((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answer.length, 1);
  assert.strictEqual(answer[0].args.allow, true, 'the close is allowed');
  assert.strictEqual(app.disk.get(b.filePath), 'second', 'and the saved one was written');
});

// === Step 7 check 3: quit =================================================
check('3. quit prompts, and Cancel keeps the app running', async () => {
  const app = makeApp();
  const { pane } = app.openTitledFileAndType('file-1.txt', 'hello');
  await app.wiring.init();

  app.listeners.get('app-quit-requested')({});
  await tick();
  assert.strictEqual(app.dialogCount(), 1, 'the prompt appeared');
  app.findButton('Cancel').click();
  await tick();

  const votes = app.invocations.filter((i) => i.command === 'quit_vote');
  assert.strictEqual(votes.length, 1, 'one vote was cast');
  assert.strictEqual(votes[0].args.allow, false, 'against quitting');
  assert.strictEqual(app.appClosed(), false, 'so the app keeps running');
  assert.strictEqual(pane.dirty, true, 'with the work still there');
});

check('3b. quit proceeds once the prompt is answered', async () => {
  const app = makeApp();
  const { filePath } = app.openTitledFileAndType('file-1.txt', 'hello');
  await app.wiring.init();

  app.listeners.get('app-quit-requested')({});
  await tick();
  app.findButton('Save').click();
  await tick();

  const votes = app.invocations.filter((i) => i.command === 'quit_vote');
  assert.strictEqual(votes[0].args.allow, true);
  assert.strictEqual(app.disk.get(filePath), 'hello');
});

// === Step 7 check 4: the save itself fails ================================
check('4. a save that fails raises a toast and does NOT close the tab', async () => {
  const app = makeApp();
  const { tabId, pane, filePath } = app.openTitledFileAndType('file-1.txt', 'hello');
  app.denyWrites(true);   // stands in for chmod 500 on the containing directory

  const closing = app.tabManager.closeTab(tabId);
  await tick();
  app.findButton('Save').click();
  await closing;

  assert.strictEqual(app.tabs.has(tabId), true, 'the tab did not close');
  assert.strictEqual(app.panes.has(pane.paneId), true, 'the pane survived');
  assert.strictEqual(pane.dirty, true, 'and is still dirty, so it can be retried');
  assert.strictEqual(app.destroyedViews.length, 0, 'the editor view was not destroyed');
  assert.strictEqual(app.disk.get(filePath), '', 'the file is untouched');
  assert.strictEqual(app.toasts.length, 1, 'an error toast appeared');
  assert.strictEqual(app.toasts[0][0], 'Save Failed');
  assert.match(app.toasts[0][1], /Permission denied \(os error 13\)/);
});

check('4b. the same failure aborts a window close', async () => {
  const app = makeApp();
  app.openTitledFileAndType('file-1.txt', 'hello');
  await app.wiring.init();
  app.denyWrites(true);

  app.listeners.get('window-close-requested')({});
  await tick();
  app.findButton('Save').click();
  await tick();

  const answer = app.invocations.filter((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answer[0].args.allow, false, 'a failed save is not consent to close');
  assert.strictEqual(app.toasts[0][0], 'Save Failed');
});

// === Step 7 check 5: nothing dirty ========================================
check('5. a tab with no dirty editor closes with no prompt at all', async () => {
  const app = makeApp();
  const { tabId } = app.openTitledFileAndType('file-1.txt', '');   // opened, never typed in
  await app.tabManager.closeTab(tabId);

  assert.strictEqual(app.dialogCount(), 0, 'no dialog was ever opened');
  assert.strictEqual(app.tabs.has(tabId), false, 'and the tab closed straight away');
});

check('5b. a saved-then-untouched editor closes with no prompt', async () => {
  const app = makeApp();
  const { tabId, pane } = app.openTitledFileAndType('file-1.txt', 'hello');
  await app.sandbox.termlabEditorService.savePane(pane);
  assert.strictEqual(pane.dirty, false, 'precondition: saving cleaned the pane');

  await app.tabManager.closeTab(tabId);
  assert.strictEqual(app.dialogCount(), 0, 'so closing asks nothing');
  assert.strictEqual(app.tabs.has(tabId), false);
});

check('5c. a window with nothing dirty closes without a prompt', async () => {
  const app = makeApp();
  app.openTitledFileAndType('file-1.txt', '');
  await app.wiring.init();

  app.listeners.get('window-close-requested')({});
  await tick();
  assert.strictEqual(app.dialogCount(), 0);
  const answer = app.invocations.filter((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answer[0].args.allow, true);
});

check('5d. a rejected manager close preserves the real tab, view, and ownership for retry', async () => {
  const app = makeApp();
  const { tabId, pane } = app.openTitledFileAndType('owned.ts', '');
  app.sandbox.termlabLspState.attach(pane, {
    documentId: 'doc-owned', version: 1, projectCandidates: [],
    status: {
      revision: 1, documentId: 'doc-owned', sessionId: 'session', adapterId: 'typescript',
      projectRootUri: 'file:///repo', state: 'ready', message: null, capabilities: {},
      errorCount: 0, warningCount: 0,
    },
  });
  app.denyDocumentClose(true);
  await app.tabManager.closeTab(tabId);
  assert.strictEqual(app.tabs.has(tabId), true, 'uncertain ownership keeps the real tab alive');
  assert.strictEqual(app.panes.has(pane.paneId), true);
  assert.strictEqual(app.destroyedViews.length, 0);
  assert.strictEqual(app.sandbox.termlabLspState.get(pane).documentId, 'doc-owned');

  app.denyDocumentClose(false);
  await app.tabManager.closeTab(tabId);
  assert.strictEqual(app.tabs.has(tabId), false, 'a confirmed later close releases and destroys');
  assert.strictEqual(app.destroyedViews.length, 1);
  assert.strictEqual(app.sandbox.termlabLspState.get(pane), null);
});

check('5d2. a later pane close rejection preserves the entire real tab and every owner', async () => {
  const app = makeApp();
  const { tabId, pane: first } = app.openTitledFileAndType('atomic-a.ts', '');
  const secondId = 9001;
  const tab = app.tabs.get(tabId);
  tab.treeRoot = app.sandbox.splitTree.splitLeaf(tab.treeRoot, first.paneId, secondId, 'vertical');
  const second = {
    paneId: secondId,
    tabId,
    kind: 'editor',
    filePath: '/home/u/docs/atomic-b.ts',
    dirty: false,
    remote: null,
    root: makeElement('div', app.sandbox.document),
  };
  second.view = app.sandbox.termlabEditorPane.createEditorView(second.root, {
    doc: '',
    onDirtyChange(value) { second.dirty = value; },
  });
  app.panes.set(secondId, second);
  for (const [pane, documentId] of [[first, 'doc-atomic-a'], [second, 'doc-atomic-b']]) {
    app.sandbox.termlabLspState.attach(pane, {
      documentId,
      version: 1,
      projectCandidates: [],
      status: {
        revision: 1, documentId, sessionId: 'session', adapterId: 'typescript',
        projectRootUri: 'file:///repo', state: 'ready', message: null, capabilities: {},
        errorCount: 0, warningCount: 0,
      },
    });
  }
  app.rejectDocumentClose('doc-atomic-b');
  await app.tabManager.closeTab(tabId);
  assert.strictEqual(app.tabs.has(tabId), true);
  assert.strictEqual(app.panes.has(first.paneId), true);
  assert.strictEqual(app.panes.has(second.paneId), true);
  assert.strictEqual(app.destroyedViews.length, 0, 'no view is destroyed before the group outcome');
  assert.strictEqual(app.sandbox.termlabLspState.get(first).documentId, 'doc-atomic-a');
  assert.strictEqual(app.sandbox.termlabLspState.get(second).documentId, 'doc-atomic-b');

  app.rejectDocumentClose(null);
  await app.tabManager.closeTab(tabId);
  assert.strictEqual(app.tabs.has(tabId), false);
  assert.strictEqual(app.destroyedViews.length, 2);
});

check('5e. the real tab-manager open path preserves typing while manager attachment is in flight', async () => {
  const app = makeApp();
  const filePath = '/home/u/docs/in-flight.ts';
  app.disk.set(filePath, 'const value = 1;');
  app.delayDocumentOpen(true);
  const opening = app.sandbox.termlabEditorService.openLocalFile(filePath);
  await tick();
  const pane = Array.from(app.panes.values()).find((entry) => entry.kind === 'editor');
  assert.ok(pane, 'the real tab manager constructed the editor pane');
  pane.view.type('\nconst light = "💡";');
  app.resolveDocumentOpen();
  await opening;
  const resync = app.invocations.find((entry) => entry.command === 'lsp_resync_document');
  assert.equal(resync.args.contents, 'const value = 1;\nconst light = "💡";');
  assert.equal(app.sandbox.termlabLspState.get(pane).version, 2);
});

check('5f. real tab close waits for a pending committed open, releases it, then destroys', async () => {
  const app = makeApp();
  const filePath = '/home/u/docs/close-pending-open.ts';
  app.disk.set(filePath, 'const pending = true;');
  app.delayDocumentOpen(true);
  const opening = app.sandbox.termlabEditorService.openLocalFile(filePath);
  await tick();
  const pane = Array.from(app.panes.values()).find((entry) => entry.kind === 'editor');
  const closing = app.tabManager.closeTab(pane.tabId);
  await tick();
  assert.equal(app.tabs.has(pane.tabId), true, 'the view remains until manager open is terminal');
  assert.equal(app.destroyedViews.length, 0);
  app.resolveDocumentOpen();
  await Promise.all([opening, closing]);
  assert.equal(app.tabs.has(pane.tabId), false);
  assert.equal(app.destroyedViews.length, 1);
  assert.equal(app.sandbox.termlabLspState.get(pane), null);
});

check('5g. real tab close waits for failed open reservation cleanup before destruction', async () => {
  const app = makeApp();
  const filePath = '/home/u/docs/close-failed-open.ts';
  app.disk.set(filePath, 'const pending = false;');
  app.delayDocumentOpen(true);
  const opening = app.sandbox.termlabEditorService.openLocalFile(filePath);
  await tick();
  const pane = Array.from(app.panes.values()).find((entry) => entry.kind === 'editor');
  const closing = app.tabManager.closeTab(pane.tabId);
  await tick();
  assert.equal(app.destroyedViews.length, 0);
  app.resolveDocumentOpen(new Error('manager open rejected'));
  await Promise.all([opening, closing]);
  assert.equal(app.tabs.has(pane.tabId), false);
  assert.equal(app.destroyedViews.length, 1);
  const commands = app.invocations.map((entry) => entry.command);
  assert.ok(commands.includes('editor_release_document'));
  assert.equal(commands.includes('lsp_close_document'), false);
});

// === closePane: the split-pane route to the same destruction ==============
// Reachable on default bindings: open a file, cmd+d (splitPane has no kind
// guard, so a terminal appears beside the editor), click back into the
// editor, type, cmd+shift+w. That lands in closePane's split branch, which
// destroys the CodeMirror view directly instead of delegating to the guarded
// closeTab — so without a guard of its own the buffer goes silently.
check('C1. closing a split editor pane prompts; Cancel keeps the pane and its text', async () => {
  const app = makeApp();
  const { tabId, pane, filePath } = app.openTitledFileAndType('file-1.txt', 'hello');
  app.splitBesideEditor(tabId);
  app.focusPane(pane.paneId);
  assert.strictEqual(app.sandbox.splitTree.leafCount(app.tabs.get(tabId).treeRoot), 2);

  const closing = app.paneManager.closePane(pane.paneId);
  await tick();
  assert.strictEqual(app.dialogCount(), 1, 'the prompt appeared');

  app.findButton('Cancel').click();
  await closing;

  assert.strictEqual(app.panes.get(pane.paneId), pane, 'the pane survived');
  assert.ok(pane.view, 'and still holds its editor view');
  assert.strictEqual(pane.dirty, true, 'and is still dirty');
  assert.strictEqual(app.destroyedViews.length, 0, 'the view was never destroyed');
  assert.strictEqual(
    app.sandbox.splitTree.leafCount(app.tabs.get(tabId).treeRoot), 2,
    'and the layout is untouched',
  );
  assert.strictEqual(app.disk.get(filePath), '', 'nothing was written');
});

check('C1b. Save on a split editor pane writes the text, then closes the pane', async () => {
  const app = makeApp();
  const { tabId, pane, filePath } = app.openTitledFileAndType('file-1.txt', 'hello');
  app.splitBesideEditor(tabId);
  app.focusPane(pane.paneId);

  const closing = app.paneManager.closePane(pane.paneId);
  await tick();
  app.findButton('Save').click();
  await closing;

  assert.strictEqual(app.disk.get(filePath), 'hello', 'the text reached the file');
  assert.strictEqual(app.panes.has(pane.paneId), false, 'and the pane closed');
  assert.strictEqual(app.destroyedViews.length, 1, 'its view was disposed');
});

check("C1c. Don't Save on a split editor pane closes it without writing", async () => {
  const app = makeApp();
  const { tabId, pane, filePath } = app.openTitledFileAndType('file-1.txt', 'hello');
  app.splitBesideEditor(tabId);
  app.focusPane(pane.paneId);

  const closing = app.paneManager.closePane(pane.paneId);
  await tick();
  app.findButton("Don't Save").click();
  await closing;

  assert.strictEqual(app.panes.has(pane.paneId), false, 'the pane closed');
  assert.strictEqual(app.disk.get(filePath), '', 'the file is unchanged');
});

check('C1d. a failed save keeps the split editor pane', async () => {
  const app = makeApp();
  const { tabId, pane } = app.openTitledFileAndType('file-1.txt', 'hello');
  app.splitBesideEditor(tabId);
  app.focusPane(pane.paneId);
  app.denyWrites(true);

  const closing = app.paneManager.closePane(pane.paneId);
  await tick();
  app.findButton('Save').click();
  await closing;

  assert.strictEqual(app.panes.get(pane.paneId), pane, 'the pane survived');
  assert.strictEqual(pane.dirty, true);
  assert.strictEqual(app.destroyedViews.length, 0);
  assert.strictEqual(app.toasts[0][0], 'Save Failed');
});

check('C1e. a clean split editor pane closes with no prompt', async () => {
  const app = makeApp();
  const { tabId, pane } = app.openTitledFileAndType('file-1.txt', '');
  app.splitBesideEditor(tabId);
  app.focusPane(pane.paneId);

  await app.paneManager.closePane(pane.paneId);
  assert.strictEqual(app.dialogCount(), 0, 'nothing was asked');
  assert.strictEqual(app.panes.has(pane.paneId), false, 'and the pane closed');
});

check('C1f. the last-leaf case still asks exactly once, via closeTab', async () => {
  // closePane delegates to closeTab here; asking in both places would prompt
  // twice for one keystroke.
  const app = makeApp();
  const { tabId, pane } = app.openTitledFileAndType('file-1.txt', 'hello');
  app.focusPane(pane.paneId);
  assert.strictEqual(app.sandbox.splitTree.leafCount(app.tabs.get(tabId).treeRoot), 1);

  const closing = app.paneManager.closePane(pane.paneId);
  await tick();
  assert.strictEqual(app.dialogCount(), 1, 'one prompt');
  app.findButton("Don't Save").click();
  await closing;
  await tick();

  assert.strictEqual(app.dialogCount(), 0, 'and no second one');
  assert.strictEqual(app.tabs.has(tabId), false, 'the tab closed');
});

let failed = 0;
for (const { name, fn } of results) {
  try {
    await fn();
    console.log(`  ok   ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${error && error.message}`);
  }
}
if (failed) {
  console.log(`unsaved-changes end to end: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`unsaved-changes end to end: all ${results.length} checks passed`);
}
