// Run: node scripts/tests/test_editor_window_fallback.mjs
//
// A window opened by `termlab <file>` boots with only an editor tab (no
// terminal) and carries window.__termlabEditorWindow. Closing its last tab
// must open a default terminal tab instead of closing the window — once:
// the flag is consumed, so closing THAT terminal tab closes the window like
// any other. The window-teardown path (closeWindowWhenLast: false) must
// never trigger the fallback. Drives the real tab-manager.js closeTab().
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');
const TAB_MANAGER = path.join(APP, 'tab-manager.js');

function makeElement(tag) {
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    dataset: {},
    className: '',
    textContent: '',
    title: '',
    classList: {
      _set: new Set(),
      add(...c) { c.forEach((x) => this._set.add(x)); },
      remove(...c) { c.forEach((x) => this._set.delete(x)); },
      toggle(c, force) {
        if (force === undefined) { this._set.has(c) ? this._set.delete(c) : this._set.add(c); }
        else if (force) this._set.add(c); else this._set.delete(c);
      },
      contains(c) { return this._set.has(c); },
    },
    appendChild(child) { this.children.push(child); return child; },
    removeChild(child) {
      const i = this.children.indexOf(child);
      if (i >= 0) this.children.splice(i, 1);
      return child;
    },
    remove() {},
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener() {},
    removeEventListener() {},
    setAttribute() {},
    getAttribute() { return null; },
  };
  return el;
}

function load() {
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    setInterval: () => 0,
    clearInterval: () => {},
    requestAnimationFrame: (cb) => cb(),
  };
  sandbox.window = sandbox;
  sandbox.document = {
    createElement: (tag) => makeElement(tag),
    getElementById: () => null,
    addEventListener() {},
  };
  sandbox.dispatchEvent = () => true;
  sandbox.CustomEvent = class { constructor(type, init) { this.type = type; Object.assign(this, init); } };
  sandbox.utils = { esc: (s) => String(s), attr: (s) => String(s) };
  sandbox.splitTree = { makeLeaf: (paneId) => ({ type: 'leaf', paneId }) };
  sandbox.splitPane = {
    setupDividerDrag: () => {},
    createPaneResizeObserver: () => ({ disconnect() {} }),
  };

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(TAB_MANAGER, 'utf8'), sandbox, { filename: TAB_MANAGER });

  const tabs = new Map();
  const panes = new Map();
  const statuses = [];
  let destroyedWindow = 0;
  let spawnedShells = 0;
  let activeTabId = 1;
  let nextId = 50;

  const fakeTerm = () => ({
    onTitleChange: () => {},
    onData: () => {},
    dispose: () => {},
    focus: () => {},
    options: {},
  });

  const manager = sandbox.termlabTabManager.create({
    getTabs: () => tabs,
    getPanes: () => panes,
    getActiveTabId: () => activeTabId,
    setActiveTabId: (id) => { activeTabId = id; },
    getFocusedPaneId: () => null,
    setFocusedPaneId: () => {},
    setNextTabLabel: () => {},
    appEl: makeElement('div'),
    getTermFontSize: () => 13,
    setFocusedPane: () => {},
    fitAndResizeTab: () => {},
    fitAndResizePane: () => {},
    onTabChanged: () => {},
    allPanesInTab: (tabId) => Array.from(panes.values())
      .filter((p) => p.tabId === tabId)
      .map((p) => p.paneId),
    rememberPluginViewSize: () => {},
    unregisterPaneDnd: () => {},
    notifyTerminalClosed: () => {},
    notifyPluginViewClosed: () => {},
    deletePluginViewPane: () => {},
    showStatus: (m) => statuses.push(m),
    destroyCurrentWindow: async () => { destroyedWindow += 1; },
    allocateTabId: () => nextId++,
    allocatePaneId: () => nextId++,
    allocateTabLabel: () => 'Terminal',
    tabBarEl: makeElement('div'),
    terminalHostEl: makeElement('div'),
    setWindowTitle: async () => {},
    getLocalPaneCwd: async () => null,
    getLocalPaneProcess: async () => null,
    getHostIdentity: async () => null,
    getWorkspaceDir: async () => null,
    refreshSshSessions: () => {},
    getCurrentWindowLabel: () => 'window-1',
    initTerminal: () => ({
      term: fakeTerm(),
      fitAddon: { proposeDimensions: () => ({ cols: 80, rows: 24 }), fit: () => {} },
    }),
    setupTmuxRightClickBridge: () => () => {},
    createPaneResizeObserver: () => ({ disconnect() {} }),
    makeLeaf: (paneId) => ({ type: 'leaf', paneId }),
    setupDividerDrag: () => {},
    normalizeTabTitle: (raw, fallback) => raw || fallback,
    spawnShell: async () => { spawnedShells += 1; },
    spawnDefaultShell: async () => { spawnedShells += 1; },
    onTerminalData: () => {},
  });

  function addEditorTab(tabId) {
    const button = makeElement('button');
    const containerEl = makeElement('div');
    tabs.set(tabId, { id: tabId, label: 'notes.md', button, containerEl, focusedPaneId: tabId * 10 });
    panes.set(tabId * 10, {
      paneId: tabId * 10,
      tabId,
      kind: 'editor',
      filePath: '/tmp/notes.md',
      dirty: false,
      view: null,
    });
  }

  return {
    sandbox, manager, tabs, panes, addEditorTab,
    counts: () => ({ destroyedWindow, spawnedShells }),
  };
}

// --- editor-window flag: closing the last tab opens a terminal, once ---------
{
  const h = load();
  h.sandbox.__termlabEditorWindow = true;
  h.addEditorTab(1);
  await h.manager.closeTab(1);
  assert.strictEqual(h.counts().destroyedWindow, 0, 'window survives the last editor tab closing');
  assert.strictEqual(h.tabs.size, 1, 'a fallback terminal tab exists');
  assert.strictEqual(h.counts().spawnedShells, 1, 'the fallback tab spawned a shell');
  assert.notStrictEqual(h.sandbox.__termlabEditorWindow, true, 'flag is consumed');

  const fallbackTabId = h.tabs.keys().next().value;
  await h.manager.closeTab(fallbackTabId);
  assert.strictEqual(h.counts().destroyedWindow, 1, 'closing the fallback tab closes the window normally');
}

// --- no flag: closing the last tab closes the window as today ----------------
{
  const h = load();
  h.addEditorTab(1);
  await h.manager.closeTab(1);
  assert.strictEqual(h.counts().destroyedWindow, 1);
  assert.strictEqual(h.tabs.size, 0);
}

// --- window teardown (closeWindowWhenLast: false) never triggers the fallback -
{
  const h = load();
  h.sandbox.__termlabEditorWindow = true;
  h.addEditorTab(1);
  await h.manager.closeTab(1, { closeWindowWhenLast: false });
  assert.strictEqual(h.counts().destroyedWindow, 0);
  assert.strictEqual(h.tabs.size, 0, 'no fallback tab during teardown');
  assert.strictEqual(h.sandbox.__termlabEditorWindow, true, 'flag untouched by teardown');
}

console.log('test_editor_window_fallback: all assertions passed');
