// Run: node scripts/tests/test_tab_close_dirty_guard.mjs
//
// closeTab() destroys CodeMirror views and deletes panes; once it has run
// there is nothing left to save. So the unsaved-changes question has to be
// settled *before* any of that, and a "no" has to stop the whole function —
// not just skip a step. This drives the real tab-manager.js closeTab() and
// asserts on what actually survives: the tab entry, the pane entry, the pane's
// view object, and whether destroyEditorView was called.
//
// The prompt itself is stubbed here (it is driven for real, through tl-dialog
// and its actual buttons, in test_editor_close_guards.mjs). The stub returns
// exactly what the real termlabEditorService.confirmDirtyPanes returns: a
// Promise<boolean>, true meaning "safe to proceed".
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const TAB_MANAGER = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/tab-manager.js',
);

function makeElement(tag) {
  return {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    dataset: {},
    style: {},
    title: '',
    textContent: '',
    removed: false,
    appendChild(c) { this.children.push(c); return c; },
    remove() { this.removed = true; },
    addEventListener() {},
    removeEventListener() {},
    classList: { add() {}, remove() {}, toggle() {}, contains() { return false; } },
  };
}

function makeHarness() {
  // tab-manager.js starts a poll timer at create(); it never fires within a
  // test, but it has to exist.
  const timers = [];
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    setInterval: (fn, ms) => { const t = setInterval(fn, ms); t.unref(); timers.push(t); return t; },
    clearInterval: (t) => clearInterval(t),
    Promise,
    Map,
    Set,
  };
  sandbox.window = sandbox;
  sandbox.document = {
    createElement: (tag) => makeElement(tag),
    getElementById: () => null,
    addEventListener() {},
  };
  sandbox.dispatchEvent = () => true;
  sandbox.CustomEvent = class { constructor(type, init) { this.type = type; Object.assign(this, init); } };

  const destroyed = [];
  sandbox.termlabEditorPane = {
    destroyEditorView: (view) => { destroyed.push(view); },
  };

  // Stubbed prompt. Returns a Promise<boolean> — the same contract as the
  // real confirmDirtyPanes, whose true/false the caller treats as
  // "safe to proceed" / "do not close".
  const asked = [];
  let answer = true;
  sandbox.termlabEditorService = {
    confirmDirtyPanes: (panes) => {
      asked.push(panes.slice());
      return Promise.resolve(answer);
    },
  };

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(TAB_MANAGER, 'utf8'), sandbox, { filename: TAB_MANAGER });

  const tabs = new Map();
  const panes = new Map();
  const statuses = [];
  let destroyedWindow = 0;
  let activeTabId = 1;

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
    allocateTabId: () => 99,
    allocatePaneId: () => 99,
    allocateTabLabel: () => 'Terminal',
    tabBarEl: makeElement('div'),
    terminalHostEl: makeElement('div'),
    setWindowTitle: async () => {},
    getLocalPaneCwd: async () => null,
    getLocalPaneProcess: async () => null,
    getHostIdentity: async () => null,
    getWorkspaceDir: async () => null,
    refreshSshSessions: () => {},
    getCurrentWindowLabel: () => 'main',
  });

  function addEditorTab(tabId, filePath, dirty) {
    const button = makeElement('button');
    const containerEl = makeElement('div');
    tabs.set(tabId, { id: tabId, label: filePath, button, containerEl, focusedPaneId: tabId * 10 });
    const view = { id: `view-${tabId}` };
    panes.set(tabId * 10, {
      paneId: tabId * 10,
      tabId,
      kind: 'editor',
      filePath,
      dirty: !!dirty,
      view,
    });
    return { button, containerEl, view };
  }

  return {
    sandbox,
    manager,
    tabs,
    panes,
    statuses,
    destroyed,
    asked,
    addEditorTab,
    setAnswer: (v) => { answer = v; },
    destroyedWindowCount: () => destroyedWindow,
  };
}

const results = [];
const check = (name, fn) => results.push({ name, fn });

check('a dirty editor tab asks before closing', async () => {
  const h = makeHarness();
  h.addEditorTab(1, '/s/a.txt', true);
  h.setAnswer(true);
  await h.manager.closeTab(1);
  assert.strictEqual(h.asked.length, 1, 'the guard asked once');
  assert.strictEqual(h.asked[0][0].filePath, '/s/a.txt', 'about the right pane');
  assert.strictEqual(h.tabs.size, 0, 'and the tab closed');
});

check('a refused close leaves the tab, the pane and the view intact', async () => {
  const h = makeHarness();
  const { view, button, containerEl } = h.addEditorTab(1, '/s/a.txt', true);
  h.setAnswer(false);
  await h.manager.closeTab(1);

  assert.strictEqual(h.tabs.size, 1, 'the tab is still open');
  assert.strictEqual(h.panes.size, 1, 'the pane still exists');
  assert.strictEqual(h.panes.get(10).view, view, 'and still holds its editor view');
  assert.strictEqual(h.panes.get(10).dirty, true, 'still dirty, so it can still be saved');
  assert.deepStrictEqual(h.destroyed, [], 'the view was never destroyed');
  assert.strictEqual(button.removed, false, 'the tab button is still in the bar');
  assert.strictEqual(containerEl.removed, false, 'and its container is still mounted');
  assert.strictEqual(h.destroyedWindowCount(), 0, 'the window was not closed either');
});

check('a clean editor tab closes with no prompt at all', async () => {
  const h = makeHarness();
  h.addEditorTab(1, '/s/a.txt', false);
  await h.manager.closeTab(1);
  assert.strictEqual(h.asked.length, 0, 'nothing was asked');
  assert.strictEqual(h.tabs.size, 0, 'and the tab closed');
  assert.strictEqual(h.destroyed.length, 1, 'its editor view was disposed');
});

check('skipDirtyCheck suppresses the prompt for a caller that already asked', async () => {
  const h = makeHarness();
  h.addEditorTab(1, '/s/a.txt', true);
  await h.manager.closeTab(1, { skipDirtyCheck: true });
  assert.strictEqual(h.asked.length, 0, 'no second prompt');
  assert.strictEqual(h.tabs.size, 0);
});

check('only the closing tab is asked about', async () => {
  const h = makeHarness();
  h.addEditorTab(1, '/s/a.txt', true);
  h.addEditorTab(2, '/s/b.txt', true);
  h.setAnswer(true);
  await h.manager.closeTab(1);
  assert.strictEqual(h.asked.length, 1);
  assert.strictEqual(h.asked[0].length, 1, 'the other tab is not dragged in');
  assert.strictEqual(h.asked[0][0].filePath, '/s/a.txt');
  assert.strictEqual(h.tabs.size, 1, 'the other tab is untouched');
});

check('a dirty editor with no editor service keeps the tab', async () => {
  const h = makeHarness();
  h.addEditorTab(1, '/s/a.txt', true);
  delete h.sandbox.termlabEditorService;
  await h.manager.closeTab(1);
  assert.strictEqual(h.tabs.size, 1, 'the tab survives rather than closing unasked');
  assert.strictEqual(h.statuses.length, 1, 'and the user is told');
  assert.match(h.statuses[0], /unsaved/i);
});

check('a tab closed underneath the prompt is not closed twice', async () => {
  const h = makeHarness();
  h.addEditorTab(1, '/s/a.txt', true);
  // Answer "yes", but delete the tab while the prompt is still open — the
  // window-close path can close tabs while a per-tab prompt is pending.
  h.sandbox.termlabEditorService = {
    confirmDirtyPanes: async () => {
      h.tabs.delete(1);
      return true;
    },
  };
  await h.manager.closeTab(1);
  assert.strictEqual(h.destroyed.length, 0, 'no second teardown of an already-gone tab');
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
  console.log(`tab close dirty guard: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`tab close dirty guard: all ${results.length} checks passed`);
}
