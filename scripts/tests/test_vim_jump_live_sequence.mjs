// Run: node scripts/tests/test_vim_jump_live_sequence.mjs
//
// The owner's live sequence, end to end, with as little stubbed as a headless
// process allows:
//
//   open a project window with ZERO tabs
//     -> single-click a file in the project tree (editor-service.openLocalFile)
//     -> put the caret on a symbol
//     -> vim `gd`      (the SHIPPED vendor bundle's engine, our mapped action)
//     -> vim `<C-o>`   (same engine, same seam)
//     -> vim `<C-i>`
//
// and asserts that Ctrl-O lands back on the file the jump started from.
//
// What is REAL here: vendor/codemirror/codemirror.js (the artifact the app
// loads, vim engine included), vim-mode.js, vim-jump-trace.js,
// lsp-navigation.js + lsp-navigation-history.js, editor-service.js,
// lsp-state.js, lsp-uri.js, lsp-position.js, and app/pane-manager.js — so the
// focus path that feeds the trail (setFocusedPane -> onFocusedPaneChanged ->
// noteFocusedPaneChanged) is the shipped one, not a re-implementation.
//
// What is stubbed: the Tauri client (editor_read_file returns fixture text,
// the LSP commands answer with a definition), the DOM (there is no jsdom in
// this repo), and tab-manager's createEditorTab — reduced to the three things
// it does that matter here, in the order it does them: build the pane with a
// real EditorState, activateTab, setFocusedPane.
//
// Every variant below (the trail after a tree open, the same-file definition,
// the click-out-and-back that "fixes" it in the built app, a terminal tool
// window holding focus in the bottom zone) drives that one harness.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const EDITOR = path.join(APP, 'features/editor');
const BUNDLE = path.join(ROOT, 'vendor/codemirror/codemirror.js');

assert.ok(
  fs.existsSync(BUNDLE),
  `vendor bundle missing: run "npm run build:vendor" in ${ROOT} (it is generated and git-ignored)`,
);

// index.html's order, restricted to what this sequence touches.
const MODULES = [
  path.join(APP, 'features/diagnostics/diag-log.js'),
  path.join(EDITOR, 'vim-jump-trace.js'),
  path.join(EDITOR, 'vim-mode.js'),
  path.join(EDITOR, 'lsp-uri.js'),
  path.join(EDITOR, 'lsp-position.js'),
  path.join(EDITOR, 'lsp-state.js'),
  path.join(EDITOR, 'language-map.js'),
  path.join(EDITOR, 'lsp-bridge.js'),
  path.join(EDITOR, 'lsp-navigation-history.js'),
  path.join(EDITOR, 'lsp-navigation-chooser.js'),
  path.join(EDITOR, 'lsp-navigation.js'),
  path.join(EDITOR, 'tab-label.js'),
  path.join(EDITOR, 'editor-service.js'),
  path.join(APP, 'pane-manager.js'),
];

const FILE_A = '/repo/src/main.rs';
const FILE_B = '/repo/src/lib.rs';
const TEXT_A = [
  'mod lib;',
  '',
  'fn main() {',
  '    let value = lib::helper();',
  '    println!("{value}");',
  '}',
  '',
].join('\n');
const TEXT_B = [
  'pub fn helper() -> u32 {',
  '    41 + 1',
  '}',
  '',
].join('\n');

const CAPABILITIES = { definition: true, references: true, hover: true };

const tick = () => new Promise((resolve) => { setTimeout(resolve, 0); });

// --- the harness --------------------------------------------------------------

// A DOM stub with only what the modules under test reach for. The chooser is
// the one that builds elements, and it only does so when several definitions
// come back; the single-definition path here never calls it.
function makeDocument() {
  function element(tag) {
    const el = {
      tagName: String(tag).toUpperCase(),
      className: '',
      textContent: '',
      children: [],
      style: {},
      dataset: {},
      hidden: false,
      classList: { add() {}, remove() {}, toggle() {}, contains: () => false },
      setAttribute() {},
      removeAttribute() {},
      appendChild(child) { this.children.push(child); return child; },
      addEventListener() {},
      removeEventListener() {},
    };
    return el;
  }
  return {
    createElement: element,
    createTextNode: (value) => ({ nodeValue: String(value) }),
    body: element('body'),
    // The vendor bundle probes this at load time for browser feature
    // detection (`document.documentElement.style`).
    documentElement: element('html'),
    activeElement: null,
    addEventListener() {},
    removeEventListener() {},
  };
}

// The CodeMirror 5 adapter vim's normal-mode path drives, over a REAL
// EditorState. `cm6` is the field our actions read the view off; the cursor is
// kept in sync with the view's selection so `gd` asks about the character the
// caret is actually on.
function vimAdapter(CM, pane) {
  const lineOf = (offset) => pane.view.state.doc.lineAt(offset);
  return {
    cm6: pane.view,
    state: {},
    curOp: null,
    getCursor() {
      const head = pane.view.state.selection.main.head;
      const line = lineOf(head);
      return { line: line.number - 1, ch: head - line.from };
    },
    listSelections() {
      const cursor = this.getCursor();
      return [{ anchor: cursor, head: cursor }];
    },
    setCursor(line, ch) {
      const target = typeof line === 'object' && line !== null
        ? line
        : { line, ch: ch || 0 };
      const doc = pane.view.state.doc;
      const number = Math.min(Math.max(target.line + 1, 1), doc.lines);
      const at = doc.line(number).from + (target.ch || 0);
      pane.view.dispatch({ selection: { anchor: at, head: at } });
    },
    getOption: () => undefined,
    setOption() {},
    operation(fn) {
      this.curOp = this.curOp || {};
      try { return fn(); } finally { this.curOp = null; }
    },
    getLine: (n) => {
      const doc = pane.view.state.doc;
      const number = Math.min(Math.max(n + 1, 1), doc.lines);
      return doc.line(number).text;
    },
    lineCount: () => pane.view.state.doc.lines,
    firstLine: () => 0,
    lastLine: () => pane.view.state.doc.lines - 1,
    getRange: () => '',
    getSelection: () => '',
    replaceRange() {},
    focus() { pane.view.focus(); },
    scrollIntoView() {},
    on() {},
    off() {},
    setBookmark: (pos) => ({ find: () => pos, clear() {} }),
    // CM6 is what our own actions read; the rest of the surface above is what
    // the engine reads.
    getMode: () => ({ name: 'null' }),
  };
}

function harness(options = {}) {
  const opts = options || {};
  const sandbox = {
    console: { log() {}, warn() {}, error() {}, debug() {} },
    setTimeout,
    clearTimeout,
    Promise,
    JSON,
    self: {},
    globalThis: {},
    performance: { now: () => Date.now() },
  };
  sandbox.window = sandbox;
  sandbox.document = makeDocument();
  sandbox.CustomEvent = class CustomEvent {
    constructor(type, init) { this.type = type; this.detail = (init || {}).detail; }
  };
  const toasts = [];
  sandbox.toast = {
    info: (title, body) => { toasts.push(['info', title, body]); },
    error: (title, body) => { toasts.push(['error', title, body]); },
    warn: (title, body) => { toasts.push(['warn', title, body]); },
    success() {},
  };
  sandbox.addEventListener = () => {};
  sandbox.removeEventListener = () => {};
  sandbox.dispatchEvent = () => true;

  // Everything the frontend sent to Rust, so a test can assert on the on-disk
  // diagnostic log's records as well as on the navigation itself.
  const invoked = [];
  const contents = { [FILE_A]: TEXT_A, [FILE_B]: TEXT_B };
  const definition = opts.definition || {
    uri: `file://${FILE_B}`,
    range: { start: { line: 0, character: 7 }, end: { line: 0, character: 13 } },
  };
  let nextDocument = 1;
  const invoke = async (command, args = {}) => {
    invoked.push({ command, args });
    if (command === 'editor_reserve_document') {
      return { kind: 'reserved', reservationId: `r-${invoked.length}`, canonicalPath: args.path };
    }
    if (command === 'editor_read_file') {
      const text = contents[args.path];
      if (typeof text !== 'string') throw new Error(`no fixture for ${args.path}`);
      return text;
    }
    if (command === 'lsp_open_document') {
      const documentId = `doc-${nextDocument++}`;
      return {
        documentId,
        version: 1,
        projectCandidates: [],
        status: {
          revision: 1,
          documentId,
          state: 'ready',
          capabilities: { ...CAPABILITIES },
          errorCount: 0,
          warningCount: 0,
        },
      };
    }
    if (command === 'lsp_definition') {
      return { documentId: args.documentId, locations: [definition] };
    }
    if (command === 'lsp_flush_document' || command === 'lsp_change_document') return null;
    if (command === 'app_diag_log' || command === 'app_diag_log_path') return null;
    return null;
  };
  sandbox.termlabServices = {
    tauriClient: {
      invoke,
      listen: () => Promise.resolve(() => {}),
      listenOnCurrentWindow: () => Promise.resolve(() => {}),
      currentWindow: { setFocus: async () => {} },
    },
  };

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(BUNDLE, 'utf8'), sandbox, { filename: BUNDLE });
  for (const file of MODULES) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  const CM = sandbox.CM6;

  // --- the window's pane/tab state, as manager-compose-runtime composes it ---
  const panes = new Map();
  const tabs = new Map();
  let focusedPaneId = null;
  let activeTabId = null;
  let nextId = 1;
  const events = [];

  const paneManager = sandbox.termlabPaneManager.create({
    getPanes: () => panes,
    getTabs: () => tabs,
    getFocusedPaneId: () => focusedPaneId,
    setFocusedPaneId: (id) => { focusedPaneId = id; },
    getPaneRatio: () => null,
    setPluginViewSize: () => {},
    rebuildTreeDOM: () => {},
    onFocusedPaneChanged: (previousPane, nextPane) => {
      events.push([
        'focus',
        previousPane ? previousPane.filePath : null,
        nextPane ? nextPane.filePath : null,
      ]);
      const navigation = sandbox.termlabLspNavigation;
      if (navigation && typeof navigation.noteFocusedPaneChanged === 'function') {
        navigation.noteFocusedPaneChanged(previousPane, nextPane);
      }
    },
    onTerminalFocused: () => {},
    unregisterPaneDnd: () => {},
    notifyTerminalClosed: () => {},
    refreshSshSessions: () => {},
    notifyPluginViewClosed: () => {},
    deletePluginViewPane: () => {},
    closeTab: () => {},
    initTerminal: () => ({}),
    setupTmuxRightClickBridge: () => {},
    createPaneResizeObserver: () => ({ disconnect() {} }),
    fitAndResizePane: () => {},
    toastError: () => {},
  });

  function activateTab(tabId) {
    const tab = tabs.get(tabId);
    if (!tab) return;
    activeTabId = tabId;
    if (tab.focusedPaneId != null) paneManager.setFocusedPane(tab.focusedPaneId);
  }

  sandbox.__termlabPaneAccess = {
    currentPane: () => paneManager.currentPane(),
    allPanes: () => panes,
    activateTab,
    setFocusedPane: (id) => paneManager.setFocusedPane(id),
    setTabLabel: () => true,
  };

  // tab-manager's createEditorTab, reduced to what this sequence exercises and
  // in the same order: build the pane and its view, register it, then
  // activateTab + setFocusedPane (tab-manager.js does both, in that order, at
  // the very end).
  sandbox.__termlabCreateEditorTab = (createOptions) => {
    const paneId = nextId++;
    const tabId = paneId + 100;
    const pane = {
      paneId,
      tabId,
      kind: 'editor',
      filePath: createOptions.filePath || null,
      remote: createOptions.remote || null,
      dirty: false,
      view: {
        state: CM.EditorState.create({ doc: createOptions.contents || '' }),
        dispatch(spec) {
          this.state = this.state.update(spec).state;
          // What the vendor's vim ViewPlugin does on every selection change,
          // verified against the shipped bundle in a real browser: a non-empty
          // selection IS visual mode, and an empty one leaves it. Modelled here
          // because there is no DOM in this process to mount the real plugin
          // in, and because it is the whole mechanism of the bug — our jump
          // keys are mapped normal-mode only.
          if (this.cm && this.cm.state.vim) {
            this.cm.state.vim.visualMode = !this.state.selection.main.empty;
          }
        },
        focus() { sandbox.document.activeElement = `view:${paneId}`; },
        hasFocus: true,
        // The field the vim ViewPlugin assigns (`this.view.cm = this.cm`), and
        // the only signal anything has that vim is mounted on a view.
        cm: { state: { vim: { visualMode: false, insertMode: false } } },
        termlabResetDirty() {},
        termlabSetReadOnly() {},
      },
    };
    panes.set(paneId, pane);
    tabs.set(tabId, { id: tabId, focusedPaneId: paneId, label: pane.filePath });
    if (typeof createOptions.onPaneCreated === 'function') createOptions.onPaneCreated(pane);
    activateTab(tabId);
    paneManager.setFocusedPane(paneId);
    return tabId;
  };

  sandbox.termlabEditorPane = { setLanguage() {}, setFontSize() {} };
  sandbox.termlabLspBridge.configure({
    windowLabel: 'project-1',
    paneAccess: sandbox.__termlabPaneAccess,
  });
  sandbox.termlabLspNavigation.configure({
    paneForView: (view) => {
      for (const pane of panes.values()) {
        if (pane && pane.view === view) return pane;
      }
      return null;
    },
    currentPane: () => paneManager.currentPane(),
    allPanes: () => panes,
    windowLabel: 'project-1',
  });

  // manager-compose-runtime's registration, verbatim in the parts that matter.
  const registered = sandbox.termlabVimMode.registerNavigationCommands({
    goToDefinition: (view) => sandbox.termlabLspNavigation.goToDefinition(view),
    findReferences: (view) => sandbox.termlabLspNavigation.findReferences(view),
    navigateBack: () => sandbox.termlabLspNavigation.navigateBack(),
    navigateForward: () => sandbox.termlabLspNavigation.navigateForward(),
    recordJump: (view, position) => sandbox.termlabLspNavigation.recordJump(view, position),
  });

  // One vim key against the focused editor pane, through the engine's own
  // resolution — the same call CM6's vim ViewPlugin makes for a keystroke.
  // One adapter per pane, for the life of the pane. vim keeps its per-editor
  // state (the pending-key buffer a two-key `gd` needs, the mode, the
  // registers) on the adapter, so a fresh one per keystroke would lose the `g`
  // before the `d` arrived — exactly the way the real ViewPlugin keeps one
  // adapter per view.
  const adapters = new Map();
  function adapterFor(pane) {
    let adapter = adapters.get(pane.paneId);
    if (!adapter) {
      adapter = vimAdapter(CM, pane);
      adapters.set(pane.paneId, adapter);
    }
    return adapter;
  }

  async function vimKey(key) {
    const pane = paneManager.currentPane();
    assert.ok(pane && pane.kind === 'editor', `vim key ${key} needs a focused editor pane`);
    const adapter = adapterFor(pane);
    const command = CM.Vim.findKey(adapter, key, 'test');
    if (typeof command !== 'function') return false;
    command();
    // The mapped actions defer to a microtask, and the work they defer to is
    // async (a definition request, an open, a reveal). Two macrotask turns is
    // enough for every await in that chain here.
    await tick();
    await tick();
    await tick();
    return true;
  }

  function caretAt(line, character) {
    const pane = paneManager.currentPane();
    const doc = pane.view.state.doc;
    const at = doc.line(line).from + character;
    pane.view.dispatch({ selection: { anchor: at, head: at } });
  }

  return {
    sandbox,
    CM,
    panes,
    tabs,
    events,
    invoked,
    toasts,
    registered,
    paneManager,
    vimKey,
    caretAt,
    service: sandbox.termlabEditorService,
    navigation: sandbox.termlabLspNavigation,
    history: sandbox.termlabLspNavigationHistory,
    get current() { return paneManager.currentPane(); },
    get focusedPath() {
      const pane = paneManager.currentPane();
      return pane ? pane.filePath : null;
    },
    diagLines: () => invoked
      .filter((entry) => entry.command === 'app_diag_log')
      .map((entry) => `${entry.args.category}: ${entry.args.message}`),
  };
}

// The whole sequence the owner performs, up to but not including the press
// being tested. Returns the harness.
async function openThenDefine(options = {}) {
  const h = harness(options);
  // Zero tabs: this is the state a project window boots into.
  assert.strictEqual(h.panes.size, 0, 'a project window starts with no panes');
  // Single-click in the project tree.
  await h.service.openLocalFile(FILE_A);
  await tick();
  assert.strictEqual(h.focusedPath, FILE_A, 'the tree open focused the file it opened');
  // Caret on `helper` in `lib::helper()`.
  h.caretAt(4, 21);
  // gd
  await h.vimKey('g');
  await h.vimKey('d');
  return h;
}

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

// --- the sequence -------------------------------------------------------------

check('the shipped bundle registered our jump actions', () => {
  const h = harness();
  assert.strictEqual(h.registered, true, 'registerNavigationCommands accepted the bundle');
});

check('gd from the first tree-opened file lands in the definition file', async () => {
  const h = await openThenDefine();
  assert.strictEqual(
    h.focusedPath, FILE_B,
    `gd should have opened the definition file; diag log:\n${h.diagLines().join('\n')}`,
  );
});

check('the jump trail is not empty after that gd', async () => {
  const h = await openThenDefine();
  const state = h.navigation.historyState();
  assert.strictEqual(
    state.back.length, 1,
    `gd must leave one entry to come back to; diag log:\n${h.diagLines().join('\n')}`,
  );
  assert.strictEqual(state.back[0].uri, `file://${FILE_A}`, 'the entry names the file gd left');
});

check('Ctrl-O returns to the first tree-opened file (the owner\'s live sequence)', async () => {
  const h = await openThenDefine();
  await h.vimKey('<C-o>');
  assert.strictEqual(
    h.focusedPath, FILE_A,
    `Ctrl-O must come back to ${FILE_A}; diag log:\n${h.diagLines().join('\n')}`,
  );
  const caret = h.current.view.state.selection.main.head;
  const line = h.current.view.state.doc.lineAt(caret);
  assert.strictEqual(line.number, 4, 'the caret is back on the line gd was pressed from');
});

check('Ctrl-I goes forward again after Ctrl-O', async () => {
  const h = await openThenDefine();
  await h.vimKey('<C-o>');
  await h.vimKey('<C-i>');
  assert.strictEqual(
    h.focusedPath, FILE_B,
    `Ctrl-I must go forward again; diag log:\n${h.diagLines().join('\n')}`,
  );
});

// --- the bug: the reveal's selection put vim in visual mode -------------------
//
// gd revealed its target by SELECTING the symbol's range. CodeMirror's vim
// plugin reads a non-empty selection as visual mode, and our <C-o>/<C-i> are
// mapped `{ context: 'normal' }` — so the very press that follows a jump had
// nothing to resolve to, while every ordinary motion kept working. The trail
// was never the problem; the mode was.

check('gd does not leave the landing editor in visual mode', async () => {
  const h = await openThenDefine();
  assert.strictEqual(
    h.current.view.cm.state.vim.visualMode, false,
    `landing in visual mode makes every normal-mode mapping unreachable; diag log:\n${h.diagLines().join('\n')}`,
  );
  assert.strictEqual(
    h.current.view.state.selection.main.empty, true,
    'the caret lands on the definition rather than selecting it, which is what keeps vim in normal mode',
  );
});

check('Ctrl-O does not leave the editor it returns to in visual mode', async () => {
  const h = await openThenDefine();
  await h.vimKey('<C-o>');
  assert.strictEqual(
    h.current.view.cm.state.vim.visualMode, false,
    'a second Ctrl-O has to be reachable too',
  );
});

check('a reveal into a pane WITHOUT vim still selects the range', async () => {
  const h = harness();
  await h.service.openLocalFile(FILE_A);
  await tick();
  const pane = h.current;
  // A window with vim_mode off: the plugin is not mounted, so the view carries
  // no adapter and the selection affordance is worth keeping.
  delete pane.view.cm;
  const revealed = h.service.revealRange(
    pane,
    { start: { line: 3, character: 8 }, end: { line: 3, character: 13 } },
    { focus: false },
  );
  assert.strictEqual(revealed, true, 'the reveal still reports success');
  assert.strictEqual(
    pane.view.state.selection.main.empty, false,
    'without vim the range is selected, as it always was',
  );
});

// --- variants -----------------------------------------------------------------

check('a SAME-FILE definition still leaves a trail Ctrl-O can walk', async () => {
  const h = harness({
    definition: {
      uri: `file://${FILE_A}`,
      range: { start: { line: 0, character: 4 }, end: { line: 0, character: 7 } },
    },
  });
  await h.service.openLocalFile(FILE_A);
  await tick();
  h.caretAt(4, 21);
  await h.vimKey('g');
  await h.vimKey('d');
  const afterJump = h.current.view.state.selection.main.head;
  assert.strictEqual(
    h.current.view.state.doc.lineAt(afterJump).number, 1,
    'gd moved the caret inside the same file',
  );
  await h.vimKey('<C-o>');
  const back = h.current.view.state.selection.main.head;
  assert.strictEqual(
    h.current.view.state.doc.lineAt(back).number, 4,
    `Ctrl-O must restore the caret in the same file; diag log:\n${h.diagLines().join('\n')}`,
  );
});

check('a manual switch back to the origin file stacks a second trail entry', async () => {
  const h = await openThenDefine();
  // The "fix" the owner found, in the reading that matches what they saw:
  // clicking a file in the tree is itself a jump, so the switch recorder puts
  // an entry on the trail whether or not gd managed to. Ctrl-O then walks THAT
  // entry — back to where the switch left from, which is the definition file.
  await h.service.openLocalFile(FILE_A);
  await tick();
  assert.strictEqual(
    h.navigation.historyState().back.length, 2,
    `gd's entry and the manual switch's; diag log:\n${h.diagLines().join('\n')}`,
  );
  await h.vimKey('<C-o>');
  assert.strictEqual(
    h.focusedPath, FILE_B,
    'Ctrl-O walks the newest entry, which is where the manual switch left from',
  );
  await h.vimKey('<C-o>');
  assert.strictEqual(h.focusedPath, FILE_A, 'a second press reaches gd\'s own entry');
});

check('a terminal pane taking focus between gd and Ctrl-O does not eat the trail', async () => {
  const h = await openThenDefine();
  const terminalPaneId = 900;
  h.panes.set(terminalPaneId, {
    paneId: terminalPaneId,
    tabId: 990,
    kind: 'terminal',
    term: { focus() {} },
  });
  h.tabs.set(990, { id: 990, focusedPaneId: terminalPaneId });
  h.paneManager.setFocusedPane(terminalPaneId);
  // Back to the editor, the way clicking into it does.
  h.paneManager.setFocusedPane(h.panes.get(2).paneId);
  await h.vimKey('<C-o>');
  assert.strictEqual(
    h.focusedPath, FILE_A,
    `a terminal detour must not lose the trail; diag log:\n${h.diagLines().join('\n')}`,
  );
});

check('the walk is reported to the on-disk diagnostic log', async () => {
  const h = await openThenDefine();
  await h.vimKey('<C-o>');
  const lines = h.diagLines();
  assert.ok(
    lines.some((line) => line.startsWith('vim-nav: 2 vim ran the mapping — termlabJumpBack')),
    `stage 2 must reach the log unconditionally, got:\n${lines.join('\n')}`,
  );
  assert.ok(
    lines.some((line) => line.startsWith('vim-nav: 3 navigate back')),
    `stage 3 must reach the log unconditionally, got:\n${lines.join('\n')}`,
  );
  assert.ok(
    lines.some((line) => line.includes('0 record definition — recorded')),
    `the recorder must say what it did, got:\n${lines.join('\n')}`,
  );
});

check('the diagnostic log is written without the toast trace being armed', async () => {
  const h = await openThenDefine();
  assert.strictEqual(
    h.sandbox.termlabVimJumpTrace.isEnabled(), false,
    'the toast trace stays off by default',
  );
  assert.ok(h.diagLines().length > 0, 'the file still gets every record');
  assert.strictEqual(
    h.toasts.filter((entry) => String(entry[1]).startsWith('Vim trace')).length, 0,
    'and no trace toast was raised',
  );
});

// --- run ----------------------------------------------------------------------

for (const { name, fn } of queued) {
  ran += 1;
  try {
    await fn();
    console.log(`ok - ${name}`);
  } catch (error) {
    failures += 1;
    console.error(`not ok - ${name}`);
    console.error(`    ${error && error.message ? error.message : error}`);
  }
}

console.log(`\n${ran - failures}/${ran} checks passed`);
if (failures) process.exit(1);
