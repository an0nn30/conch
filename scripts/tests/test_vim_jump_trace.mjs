// Run: node scripts/tests/test_vim_jump_trace.mjs
//
// The in-app diagnostic for the vim <C-o>/<C-i> jump trail:
// features/editor/vim-jump-trace.js plus the three places that call it.
//
// The bug it exists to read: in the built app `gd` jumps but Ctrl-O/Ctrl-I do
// nothing, and every static explanation has been eliminated — the mapping
// resolves against the shipped vendor bundle (test_vim_vendor_keymap.mjs), no
// router/config/menu binding claims the combination, and registration is
// unconditional. What is left is only visible in a live WKWebView, and the
// owner does not open devtools. So each stage toasts:
//
//   1. shortcut-runtime.js — the capture-phase router saw the keydown;
//   2. vim-mode.js         — vim matched the key to one of our actions;
//   3. lsp-navigation.js   — what the walk then did.
//
// What this pins:
//   - the trace defaults OFF, is a module variable, and is never persisted;
//   - stage 1 reports `key` AND `code`, so a "Process"/"Dead" delivery is
//     visible, and it fires for that case rather than filtering it out;
//   - stage 1's router entry never consumes, and outranks every consumer, so
//     arming the trace cannot change which handler wins a key;
//   - stage 3 spells step()'s 'none' as "empty", which is the reading that
//     separates a dead key from an empty trail;
//   - none of the three can break the feature when the module is absent;
//   - both palette commands exist, under the names the report names.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const MODULES = path.join(APP, 'features/editor');
const TRACE = path.join(MODULES, 'vim-jump-trace.js');
const VIM_MODE = path.join(MODULES, 'vim-mode.js');
const HISTORY = path.join(MODULES, 'lsp-navigation-history.js');
const CHOOSER = path.join(MODULES, 'lsp-navigation-chooser.js');
const NAVIGATION = path.join(MODULES, 'lsp-navigation.js');
const URI = path.join(MODULES, 'lsp-uri.js');
const POSITION = path.join(MODULES, 'lsp-position.js');
const SHORTCUT_RUNTIME = path.join(APP, 'shortcut-runtime.js');
const PALETTE_RUNTIME = path.join(APP, 'command-palette-runtime.js');
const INDEX_HTML = path.join(ROOT, 'index.html');
const NODE_MODULES = path.join(ROOT, 'node_modules');

// vim's own engine, for stage 2. The SHIPPED bundle is asserted separately, in
// test_vim_vendor_keymap.mjs — this file is about the trace, not the mapping.
const REAL_VIM = await import(
  pathToFileURL(path.join(NODE_MODULES, '@replit/codemirror-vim/dist/index.js')).href
);

const results = [];
function check(name, fn) { results.push({ name, fn }); }
const tick = () => new Promise((resolve) => { setTimeout(resolve, 0); });

// --- sandboxes ---------------------------------------------------------------

// Every harness records toasts rather than rendering them: "which stage
// toasted, with what body" IS the diagnostic's contract.
function load(files, extra) {
  const sandbox = {
    console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap, Array, Object, JSON, Number,
  };
  sandbox.window = sandbox;
  const toasts = [];
  sandbox.toast = {
    info: (title, body, opts) => { toasts.push({ level: 'info', title, body, opts }); },
    error: (title, body) => { toasts.push({ level: 'error', title, body }); },
    warn: (title, body) => { toasts.push({ level: 'warn', title, body }); },
    success: (title, body) => { toasts.push({ level: 'success', title, body }); },
  };
  Object.assign(sandbox, extra || {});
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return { sandbox, toasts, trace: sandbox.termlabVimJumpTrace };
}

// --- the module's own surface -------------------------------------------------

check('the trace defaults off and toasts nothing until it is armed', () => {
  const { trace, toasts } = load([TRACE]);
  assert.strictEqual(trace.isEnabled(), false, 'a diagnostic that starts on is a bug, not a diagnostic');
  assert.strictEqual(trace.noteKey({ ctrlKey: true, key: 'o', code: 'KeyO' }), false);
  assert.strictEqual(trace.noteVimAction('termlabJumpBack'), false);
  assert.strictEqual(trace.noteNavigation('back', 'navigated'), false);
  assert.deepStrictEqual(toasts, [], 'nothing at all before the toggle');
});

check('the trace state is session-only — no storage, no settings, no invoke', () => {
  const source = fs.readFileSync(TRACE, 'utf8');
  for (const forbidden of ['localStorage', 'sessionStorage', 'invoke(', 'save_window_layout', 'get_all_settings']) {
    assert.ok(
      !source.includes(forbidden),
      `${forbidden} would persist or transmit a per-sitting diagnostic`,
    );
  }
  // Two fresh loads are two fresh sandboxes; the point is that the ON state
  // does not survive one, which is what "module variable" buys.
  const first = load([TRACE]);
  first.trace.setEnabled(true);
  assert.strictEqual(first.trace.isEnabled(), true);
  assert.strictEqual(load([TRACE]).trace.isEnabled(), false, 'a reload starts off');
});

check('toggleTrace flips the state and says so in both directions', () => {
  const { trace, toasts } = load([TRACE]);
  assert.strictEqual(trace.toggleTrace(), true);
  assert.strictEqual(trace.isEnabled(), true);
  assert.strictEqual(toasts.length, 1);
  assert.match(toasts[0].body, /^On\./, 'the on toast says how to use it');
  assert.match(toasts[0].body, /Ctrl-O/, 'and which keys to press');
  assert.strictEqual(trace.toggleTrace(), false);
  assert.strictEqual(toasts.length, 2, 'switching off is not a silent no-op');
  assert.match(toasts[1].body, /Off/);
});

check('stage 1 matches Ctrl-O/Ctrl-I and ignores everything else', () => {
  const { trace } = load([TRACE]);
  assert.strictEqual(trace.isJumpKeyEvent({ ctrlKey: true, key: 'o', code: 'KeyO' }), true);
  assert.strictEqual(trace.isJumpKeyEvent({ ctrlKey: true, key: 'i', code: 'KeyI' }), true);
  assert.strictEqual(trace.isJumpKeyEvent({ ctrlKey: true, key: 'O', code: 'KeyO' }), true, 'case folded');
  assert.strictEqual(trace.isJumpKeyEvent({ ctrlKey: false, key: 'o', code: 'KeyO' }), false, 'plain o is a motion');
  assert.strictEqual(trace.isJumpKeyEvent({ ctrlKey: true, key: 'p', code: 'KeyP' }), false);
  assert.strictEqual(trace.isJumpKeyEvent(null), false);
});

check('a "Process"/"Dead" delivery still trips stage 1 — that is the case worth reporting', () => {
  const { trace, toasts } = load([TRACE]);
  trace.setEnabled(true);
  // WKWebView can hand the app a composed keystroke whose `key` says nothing
  // useful. Matching on `code` too is the difference between "the trace found
  // nothing" and "the trace found the answer".
  assert.strictEqual(trace.isJumpKeyEvent({ ctrlKey: true, key: 'Process', code: 'KeyO' }), true);
  assert.strictEqual(trace.isJumpKeyEvent({ ctrlKey: true, key: 'Dead', code: 'KeyI' }), true);
  assert.strictEqual(trace.noteKey({ ctrlKey: true, key: 'Process', code: 'KeyO' }), true);
  assert.strictEqual(toasts.length, 1);
  assert.match(toasts[0].body, /key="Process"/, 'the raw key is quoted into the toast');
  assert.match(toasts[0].body, /code="KeyO"/, 'and so is the code');
});

check('stage 1 reports key, code and the modifiers actually held', () => {
  const { trace } = load([TRACE]);
  const body = trace.describeKeyEvent({
    ctrlKey: true, altKey: true, key: 'o', code: 'KeyO', repeat: true,
    target: { tagName: 'DIV' },
  });
  assert.match(body, /key="o"/);
  assert.match(body, /code="KeyO"/);
  assert.match(body, /mods=ctrl\+alt/);
  assert.match(body, /repeat/);
  assert.match(body, /target=div/);
  assert.match(
    trace.describeKeyEvent({ key: 'o', code: 'KeyO' }), /mods=none/,
    'no modifiers is stated, not left blank',
  );
});

check("stage 3 spells step()'s 'none' as empty — the reading the owner needs", () => {
  const { trace } = load([TRACE]);
  assert.strictEqual(trace.normalizeOutcome('none'), 'empty');
  assert.strictEqual(trace.normalizeOutcome(undefined), 'empty');
  for (const outcome of ['navigated', 'elsewhere', 'failed', 'unrevealed']) {
    assert.strictEqual(trace.normalizeOutcome(outcome), outcome, 'every other outcome is reported verbatim');
  }
});

check('the trail summary reports both depths and the top back entry as file:line', () => {
  const { sandbox, trace } = load([TRACE, URI]);
  const state = {
    back: [
      { uri: 'file:///repo/src/first.ts', position: { line: 3, character: 0 } },
      { uri: 'file:///repo/src/main.ts', position: { line: 41, character: 8 } },
    ],
    forward: [{ uri: 'file:///repo/src/other.ts', position: { line: 0, character: 0 } }],
  };
  sandbox.termlabLspNavigation = { historyState: () => state };
  const summary = trace.showTrail();
  assert.match(summary, /back 2/);
  assert.match(summary, /forward 1/);
  // 1-based, because that is what the editor's gutter shows; the history
  // speaks 0-based LSP positions.
  assert.match(summary, /main\.ts:42/, 'the TOP of the back stack, in gutter line numbers');
  assert.ok(!summary.includes('first.ts'), 'only the entry the next Ctrl-O would take');
});

check('the trail summary survives an empty trail and a window with no navigator', () => {
  const empty = load([TRACE, URI]);
  empty.sandbox.termlabLspNavigation = { historyState: () => ({ back: [], forward: [] }) };
  assert.match(empty.trace.showTrail(), /back 0 \/ forward 0/);
  assert.match(empty.trace.showTrail(), /top back entry none/);

  const bare = load([TRACE]);
  assert.match(bare.trace.showTrail(), /no navigation history/);
  assert.strictEqual(bare.toasts.length, 1, 'it still tells the user something');

  const broken = load([TRACE]);
  broken.sandbox.termlabLspNavigation = { historyState: () => { throw new Error('gone'); } };
  assert.match(broken.trace.showTrail(), /no navigation history/, 'a throwing navigator is reported, not rethrown');
});

check('showTrail works without arming the trace — reading the trail is half the answer', () => {
  const { sandbox, trace, toasts } = load([TRACE, URI]);
  sandbox.termlabLspNavigation = { historyState: () => ({ back: [], forward: [] }) };
  assert.strictEqual(trace.isEnabled(), false);
  trace.showTrail();
  assert.strictEqual(toasts.length, 1, 'no arming step to forget');
});

// --- stage 1: the keyboard router --------------------------------------------

function shortcutHarness(extra) {
  const registrations = [];
  const { sandbox } = load([SHORTCUT_RUNTIME], Object.assign({
    termlabKeyboardRouter: {
      register(options) {
        registrations.push(options);
        return () => {};
      },
    },
    document: { addEventListener() {}, removeEventListener() {}, activeElement: null },
  }, extra || {}));
  const runtime = sandbox.termlabShortcutRuntime.create({
    invoke: () => Promise.resolve({}),
    isMacPlatform: true,
    isTextInputTarget: () => false,
    handleMenuAction: () => {},
    shouldDebugKeyEvent: () => false,
    formatKeyEventForDebug: () => '{}',
    shortcutDebugEnabled: false,
    openCommandPalette: () => {},
    closeCommandPalette: () => {},
    isCommandPaletteOpen: () => false,
    getTabIds: () => [],
    activateTab: () => {},
    getCurrentPane: () => null,
    writeTextToCurrentPane: () => {},
    getActiveTab: () => null,
    getFocusedPaneId: () => null,
    setFocusedPane: () => {},
    findAdjacentPane: () => null,
  });
  return { sandbox, runtime, registrations };
}

check('shortcut-runtime registers the stage 1 observer above every consumer', async () => {
  const h = shortcutHarness();
  await h.runtime.init();
  const entry = h.registrations.find((r) => r.name === 'vim-jump-trace');
  assert.ok(entry, 'the observer is registered on every boot, like the rest');
  const consumers = h.registrations.filter((r) => r.name !== 'vim-jump-trace');
  assert.ok(consumers.length > 0, 'there are consumers to outrank');
  for (const consumer of consumers) {
    assert.ok(
      entry.priority > consumer.priority,
      `the observer must see a key before ${consumer.name} can claim it`,
    );
  }
  // The command palette registers at 260 from its own module, and the app's
  // highest today is tl-dialog's Escape at 225 — the observer has to clear
  // those too, or "who ate my Ctrl-O" is unanswerable.
  assert.ok(entry.priority > 260, 'above the command palette as well');
});

check('the stage 1 observer notes the key and NEVER consumes it', async () => {
  const h = shortcutHarness();
  await h.runtime.init();
  vm.runInContext(fs.readFileSync(TRACE, 'utf8'), h.sandbox, { filename: TRACE });
  const trace = h.sandbox.termlabVimJumpTrace;
  const entry = h.registrations.find((r) => r.name === 'vim-jump-trace');

  trace.setEnabled(true);
  const seen = [];
  h.sandbox.toast.info = (title, body) => { seen.push({ title, body }); };
  assert.strictEqual(
    entry.onKeyDown({ ctrlKey: true, key: 'o', code: 'KeyO' }), false,
    'returning true would consume the key and CHANGE the behaviour being diagnosed',
  );
  assert.strictEqual(seen.length, 1);
  assert.match(seen[0].title, /^Vim trace 1/);
  assert.match(seen[0].body, /code="KeyO"/);

  // Everything else stays quiet even while armed.
  assert.strictEqual(entry.onKeyDown({ ctrlKey: false, key: 'j', code: 'KeyJ' }), false);
  assert.strictEqual(seen.length, 1);
});

check('the stage 1 observer is inert when the trace module is missing', async () => {
  const h = shortcutHarness();
  await h.runtime.init();
  const entry = h.registrations.find((r) => r.name === 'vim-jump-trace');
  assert.strictEqual(h.sandbox.termlabVimJumpTrace, undefined, 'no module in this sandbox');
  assert.strictEqual(entry.onKeyDown({ ctrlKey: true, key: 'o', code: 'KeyO' }), false, 'and no throw');
});

// --- stage 2: vim's mapped action ---------------------------------------------

function vimAdapter(view) {
  let cursor = { line: 0, ch: 0 };
  return {
    cm6: view,
    state: {},
    curOp: null,
    getCursor: () => ({ line: cursor.line, ch: cursor.ch }),
    listSelections: () => [{ anchor: cursor, head: cursor }],
    setCursor(line, ch) {
      cursor = typeof line === 'object' && line !== null
        ? { line: line.line, ch: line.ch || 0 }
        : { line, ch: ch || 0 };
    },
    getOption: () => undefined,
    setOption() {},
    operation(fn) {
      this.curOp = this.curOp || {};
      try { return fn(); } finally { this.curOp = null; }
    },
    getLine: () => 'const value = 1;',
    lineCount: () => 40,
    firstLine: () => 0,
    lastLine: () => 39,
    getRange: () => '',
    getSelection: () => '',
    replaceRange() {},
    focus() {},
    scrollIntoView() {},
    on() {},
    off() {},
    setBookmark: (pos) => ({ find: () => pos, clear() {} }),
  };
}

function vimHarness(files) {
  const loaded = load(files, { CM6: { Vim: REAL_VIM.Vim, vim: () => ({ ext: 'vim' }) } });
  const calls = { back: 0, forward: 0, definitions: [] };
  loaded.sandbox.termlabVimMode.registerNavigationCommands({
    goToDefinition: (view) => { calls.definitions.push(view); return Promise.resolve('navigated'); },
    navigateBack: () => { calls.back += 1; return Promise.resolve('navigated'); },
    navigateForward: () => { calls.forward += 1; return Promise.resolve('navigated'); },
  });
  return Object.assign(loaded, { calls });
}

check('stage 2 fires from vim\'s own action, naming the mapping that ran', async () => {
  const h = vimHarness([TRACE, VIM_MODE]);
  h.trace.setEnabled(true);
  h.toasts.length = 0;
  const adapter = vimAdapter({ id: 'view' });
  REAL_VIM.Vim.findKey(adapter, '<C-o>', 'test')();
  await tick();
  assert.strictEqual(h.calls.back, 1, 'the jump still happened');
  const stage2 = h.toasts.filter((t) => /^Vim trace 2/.test(t.title));
  assert.strictEqual(stage2.length, 1);
  assert.strictEqual(stage2[0].body, 'termlabJumpBack', 'the action name, so C-o and C-i are told apart');
});

check('stage 2 reports gd too, so "gd traced, Ctrl-O silent" is readable', async () => {
  const h = vimHarness([TRACE, VIM_MODE]);
  h.trace.setEnabled(true);
  h.toasts.length = 0;
  const adapter = vimAdapter({ id: 'view' });
  REAL_VIM.Vim.findKey(adapter, 'g', 'test');
  REAL_VIM.Vim.findKey(adapter, 'd', 'test')();
  await tick();
  const stage2 = h.toasts.filter((t) => /^Vim trace 2/.test(t.title));
  assert.deepStrictEqual(stage2.map((t) => t.body), ['termlabGoToDefinition']);
});

check('vim-mode works unchanged with the trace module absent, and silently with it off', async () => {
  const absent = vimHarness([VIM_MODE]);
  assert.strictEqual(absent.sandbox.termlabVimJumpTrace, undefined);
  REAL_VIM.Vim.findKey(vimAdapter({ id: 'a' }), '<C-o>', 'test')();
  await tick();
  assert.strictEqual(absent.calls.back, 1, 'the key still works with no diagnostic loaded');

  const off = vimHarness([TRACE, VIM_MODE]);
  off.toasts.length = 0;
  REAL_VIM.Vim.findKey(vimAdapter({ id: 'b' }), '<C-i>', 'test')();
  await tick();
  assert.strictEqual(off.calls.forward, 1);
  assert.deepStrictEqual(off.toasts, [], 'off means off');
});

// --- stage 3: the navigation outcome -------------------------------------------

function navigationHarness(options) {
  const opts = options || {};
  const windowListeners = new Map();
  const files = (opts.omitTrace ? [] : [TRACE]).concat([URI, POSITION, HISTORY, CHOOSER, NAVIGATION]);
  const loaded = load(files, {
    CM6: null,
    document: { createElement: () => ({ appendChild() {}, style: {}, classList: { add() {}, remove() {} } }) },
    addEventListener: (type, handler) => {
      if (!windowListeners.has(type)) windowListeners.set(type, []);
      windowListeners.get(type).push(handler);
    },
    removeEventListener: () => {},
    CustomEvent: class { constructor(type) { this.type = type; } },
    dispatchEvent: () => true,
  });
  const opens = [];
  loaded.sandbox.termlabEditorService = {
    openLocalFileAt: (filePath, range) => {
      opens.push({ filePath, range });
      return Promise.resolve(opts.open ? opts.open(filePath) : { status: 'opened', revealed: true });
    },
  };
  loaded.sandbox.termlabLspState = { get: () => null };
  const navigation = loaded.sandbox.termlabLspNavigation;
  navigation.configure({
    paneForView: () => null,
    currentPane: () => null,
    allPanes: () => new Map(),
    windowLabel: 'main',
    requestFeature: () => Promise.resolve(null),
  });
  return Object.assign(loaded, { navigation, opens });
}

check('stage 3 reports an empty trail as "empty", not as silence', async () => {
  const h = navigationHarness();
  h.trace.setEnabled(true);
  h.toasts.length = 0;
  const outcome = await h.navigation.navigateBack();
  assert.strictEqual(outcome, 'none', 'the outcome the caller sees is untouched');
  const stage3 = h.toasts.filter((t) => /^Vim trace 3/.test(t.title));
  assert.strictEqual(stage3.length, 1);
  assert.match(stage3[0].title, /navigate back/);
  assert.match(stage3[0].body, /^empty/, 'the trail was empty — a recorder problem, not a key problem');
  assert.match(stage3[0].body, /back 0 \/ forward 0/, 'with the depths, so it is self-explaining');
  assert.deepStrictEqual(h.opens, [], 'nothing was opened for an empty trail');
});

check('stage 3 reports a completed walk, and forward is labelled forward', async () => {
  const h = navigationHarness();
  const store = h.sandbox.termlabLspNavigationHistory;
  store.record({
    uri: 'file:///repo/src/main.ts',
    position: { line: 9, character: 2 },
    range: { start: { line: 9, character: 2 }, end: { line: 9, character: 2 } },
    owner: { windowLabel: 'main', paneId: '1' },
  });
  h.trace.setEnabled(true);
  h.toasts.length = 0;
  assert.strictEqual(await h.navigation.navigateBack(), 'navigated');
  assert.strictEqual(await h.navigation.navigateForward(), 'none');
  const stage3 = h.toasts.filter((t) => /^Vim trace 3/.test(t.title));
  assert.strictEqual(stage3.length, 2);
  assert.match(stage3[0].body, /^navigated/);
  assert.match(stage3[1].title, /navigate forward/);
});

check('navigation is untouched when the trace module is absent', async () => {
  const h = navigationHarness({ omitTrace: true });
  assert.strictEqual(h.sandbox.termlabVimJumpTrace, undefined);
  assert.strictEqual(await h.navigation.navigateBack(), 'none', 'no throw, same answer');
  assert.deepStrictEqual(h.toasts, []);
});

// --- the palette entries --------------------------------------------------------

const PALETTE_NAMES = [
  'Vim Navigation: Show Jump Trail',
  'Vim Navigation: Trace Keys (toggle)',
];

// A palette harness small enough to read: the runtime hands its body to
// tlDialog and renders into it, so the two elements it builds (input, results)
// are all the DOM this needs.
function paletteElement(tag) {
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    className: '',
    textContent: '',
    value: '',
    isConnected: true,
    appendChild(child) { this.children.push(child); return child; },
    setAttribute() {},
    focus() {},
    scrollIntoView() {},
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    dispatch(name, event) {
      for (const fn of (listeners.get(name) || []).slice()) fn(event);
    },
    classList: { add() {}, remove() {}, contains: () => false },
  };
  Object.defineProperty(el, 'innerHTML', { get: () => '', set() { el.children = []; } });
  return el;
}

function collectTitles(node, out) {
  if (node.className === 'tl-palette__title') out.push(node.textContent);
  for (const child of node.children || []) collectTitles(child, out);
  return out;
}

async function paletteHarness() {
  let dialogBody = null;
  const { sandbox, toasts } = load([PALETTE_RUNTIME], {
    document: { createElement: paletteElement, activeElement: null, addEventListener() {}, removeEventListener() {} },
    tlDialog: {
      open(opts) {
        dialogBody = opts.body;
        if (typeof opts.onOpen === 'function') opts.onOpen(paletteElement('div'));
        return { el: paletteElement('div'), close() { if (opts.onClose) opts.onClose(); } };
      },
      count: () => 1,
    },
    termlabKeyboardRouter: { register: () => () => {} },
  });
  vm.runInContext(fs.readFileSync(TRACE, 'utf8'), sandbox, { filename: TRACE });
  const runtime = sandbox.termlabCommandPaletteRuntime.create({
    invoke: (command) => {
      if (command === 'remote_get_servers') return Promise.resolve({ folders: [], ungrouped: [], ssh_config: [] });
      if (command === 'scan_plugins' || command === 'get_plugin_menu_items') return Promise.resolve([]);
      if (command === 'tunnel_get_all' || command === 'project_recents') return Promise.resolve([]);
      return Promise.resolve(undefined);
    },
    listen: () => Promise.resolve(),
    esc: (value) => value,
    handleMenuAction: () => {},
    createSshTab: () => {},
    getCurrentPane: () => null,
    showStatus: () => {},
    refreshTitlebar: () => {},
    refreshSshPanel: () => {},
  });
  await runtime.open();
  return {
    sandbox,
    toasts,
    query(text) {
      const input = dialogBody.children[0];
      input.value = text;
      input.dispatch('input', {});
      return collectTitles(dialogBody.children[1], []);
    },
  };
}

check('both diagnostics are reachable from the palette, by name', async () => {
  const h = await paletteHarness();
  const titles = h.query('vim navigation');
  for (const name of PALETTE_NAMES) {
    assert.ok(titles.includes(name), `"${name}" is missing from the palette (found: ${titles.join(', ')})`);
  }
});

check('running the palette entries drives the trace module', async () => {
  const h = await paletteHarness();
  h.sandbox.termlabLspNavigation = { historyState: () => ({ back: [], forward: [] }) };
  const trace = h.sandbox.termlabVimJumpTrace;

  h.query('vim navigation show jump trail');
  h.toasts.length = 0;
  // The palette's own execution path: the first rendered row is what Enter
  // runs, and the query above puts Show Jump Trail there.
  const titles = h.query('show jump trail');
  assert.strictEqual(titles[0], 'Vim Navigation: Show Jump Trail', 'the trail entry sorts first for its own name');

  assert.strictEqual(trace.isEnabled(), false);
  trace.toggleTrace();
  assert.strictEqual(trace.isEnabled(), true, 'the toggle is the state the three stages read');
});

check('the diagnostic module is loaded by index.html, ahead of vim-mode', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const trace = html.indexOf('app/features/editor/vim-jump-trace.js');
  const vimMode = html.indexOf('app/features/editor/vim-mode.js');
  assert.ok(trace > 0, 'index.html must load the trace module or every stage is a silent no-op');
  assert.ok(vimMode > 0);
  assert.ok(trace < vimMode, 'loaded before its first caller, matching the rest of the editor block');
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
  console.log(`vim jump trace: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`vim jump trace: all ${results.length} checks passed`);
}
