// Run: node scripts/tests/test_editor_vim_glue.mjs
//
// The seam between @replit/codemirror-vim and this app: what `:w` and `:q`
// actually do, and where vim's keymap sits in the editor's extension list.
//
// Two things are stubbed and nothing else. CM6, because the real bundle needs
// a DOM (there is no jsdom in this repo) — the stub mirrors the shapes the
// real package documents in its .d.ts:
//
//     vim(options?): Extension
//     Vim.defineEx(name: string, prefix: string|undefined, func: ExFn): void
//
// and the app's own save/close entry points, because `deps` is exactly the
// injection point vim-mode.js exists to have. The modules under test —
// features/editor/vim-mode.js and features/editor/editor-pane.js — are the
// real files.
//
// What this pins:
//   - vim's extension is present only when the setting is on;
//   - :w / :write, :q / :quit and :wq are all defined, with the short forms
//     registered as vim's own prefixes;
//   - :w saves the FOCUSED pane through editor-service's savePane, and :q
//     closes through the GUARDED closeTab (never a raw view destroy), so a
//     dirty editor still gets its Save/Don't Save/Cancel prompt;
//   - :wq saves before it closes and, when the save fails, does not close;
//   - none of them do anything when the focused pane is not an editor;
//   - the vim compartment is the FIRST extension in editor-pane's list, ahead
//     of the default keymap — the ordering the whole feature hangs on.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const VIM_MODE = path.join(ROOT, 'app/features/editor/vim-mode.js');
const EDITOR_PANE = path.join(ROOT, 'app/features/editor/editor-pane.js');
const LANGUAGE_MAP = path.join(ROOT, 'app/features/editor/language-map.js');
const COMPOSE = path.join(ROOT, 'app/manager-compose-runtime.js');
const NODE_MODULES = path.join(ROOT, 'node_modules');
const VIM_CORE_KEYMAP = path.join(NODE_MODULES, '@replit/codemirror-vim-core/vim.js');

// `gd` is matched by the REAL vim engine, because the whole point of the
// binding is that vim's own command table owns those two keys in normal mode.
const REAL_VIM = await import(
  pathToFileURL(path.join(NODE_MODULES, '@replit/codemirror-vim/dist/index.js')).href
);

// The smallest CodeMirror adapter the engine's normal-mode path touches when
// it runs an action: somewhere to keep vim state, a cursor, and an operation
// that gives the engine a `curOp` to tag. `cm6` is where the adapter puts the
// EditorView, which is the one field the action reads.
function vimAdapter(view) {
  // The cursor is real enough to move: the engine skips recording a jump that
  // did not go anywhere, so `G` only reaches the jumplist if the position
  // actually changes.
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
    // vim's own jumplist stores bookmarks; the real adapter makes them from
    // the document, and this is enough for the engine to run its half.
    setBookmark: (pos) => ({ find: () => pos, clear() {} }),
  };
}

// Type `keys` at the real engine and return whatever the last key resolved to.
function typeKeys(adapter, keys) {
  let result;
  for (const key of keys) result = REAL_VIM.Vim.findKey(adapter, key, 'test');
  return result;
}

// --- CM6 stand-in ----------------------------------------------------------
//
// Every extension factory returns a tagged plain object so the extension list
// can be identified positionally without importing CodeMirror. Compartment
// mirrors the real one closely enough for editor-pane.js: `of()` wraps the
// initial contents, `reconfigure()` produces the effect a dispatch carries.
function makeCM6(sandbox) {
  let nextCompartmentId = 1;
  class Compartment {
    constructor() { this.id = nextCompartmentId++; }
    of(ext) { return { compartment: this.id, contents: ext }; }
    reconfigure(ext) { return { reconfigure: this.id, contents: ext }; }
  }

  const views = [];
  function EditorView(config) {
    this.config = config;
    this.state = config.state;
    this.effects = [];
    this.dispatch = (tr) => {
      const effects = tr && tr.effects;
      if (Array.isArray(effects)) this.effects.push(...effects);
      else if (effects) this.effects.push(effects);
    };
    this.destroy = () => { this.destroyed = true; };
    views.push(this);
  }
  EditorView.updateListener = { of: (fn) => ({ ext: 'updateListener', fn }) };
  EditorView.theme = (spec) => ({ ext: 'theme', spec });

  const tagged = (name) => () => ({ ext: name });

  const CM = {
    Compartment,
    EditorView,
    EditorState: {
      create: (spec) => ({ spec, doc: { toString: () => spec.doc } }),
      readOnly: { of: (value) => ({ ext: 'readOnly', value }) },
    },
    lineNumbers: tagged('lineNumbers'),
    highlightActiveLineGutter: tagged('highlightActiveLineGutter'),
    highlightSpecialChars: tagged('highlightSpecialChars'),
    history: tagged('history'),
    foldGutter: tagged('foldGutter'),
    drawSelection: tagged('drawSelection'),
    rectangularSelection: tagged('rectangularSelection'),
    indentOnInput: tagged('indentOnInput'),
    bracketMatching: tagged('bracketMatching'),
    highlightActiveLine: tagged('highlightActiveLine'),
    highlightSelectionMatches: tagged('highlightSelectionMatches'),
    keymap: { of: (bindings) => ({ ext: 'keymap', bindings }) },
    defaultKeymap: ['defaultKeymap'],
    historyKeymap: ['historyKeymap'],
    searchKeymap: ['searchKeymap'],
    foldKeymap: ['foldKeymap'],
    indentWithTab: 'indentWithTab',
    StreamLanguage: { define: (parser) => ({ ext: 'streamLanguage', parser }) },
    // The two names vim-mode.js reaches for. `vim()` returns an extension;
    // `Vim.defineEx` records (name, prefix, handler) the way the real engine
    // does when it fills exCommands[name] and commandMap_[prefix].
    vim: (options) => ({ ext: 'vim', options: options || null }),
    Vim: {
      defined: [],
      defineEx(name, prefix, func) {
        if (prefix && name.indexOf(prefix) !== 0) {
          throw new Error(`(Vim.defineEx) "${prefix}" is not a prefix of "${name}"`);
        }
        this.defined.push({ name, prefix: prefix || name, func });
      },
    },
    getCM: () => null,
  };
  sandbox.CM6 = CM;
  return { CM, views };
}

function loadModules(files, extra) {
  const sandbox = { console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap, Array, Object };
  sandbox.window = sandbox;
  sandbox.document = {
    createElement: () => ({ appendChild() {}, style: {}, classList: { add() {}, remove() {} } }),
  };
  const cm = makeCM6(sandbox);
  Object.assign(sandbox, extra || {});
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return { sandbox, cm };
}

// A harness for the ex commands: real vim-mode.js, recorded deps.
function makeExHarness(options = {}) {
  const { sandbox, cm } = loadModules([VIM_MODE]);
  const saves = [];
  const closes = [];
  let saveRejection = options.saveRejection || null;
  const toasts = [];
  sandbox.toast = {
    error: (title, body) => { toasts.push([title, body]); },
    warn() {}, info() {}, success() {},
  };

  let focused = options.pane === undefined
    ? { paneId: 7, tabId: 3, kind: 'editor', view: {}, filePath: '/s/a.txt', dirty: true }
    : options.pane;

  const deps = {
    savePane: (pane) => {
      saves.push(pane);
      if (saveRejection) return Promise.reject(saveRejection);
      return Promise.resolve();
    },
    closeTab: (tabId) => { closes.push(tabId); return Promise.resolve(); },
    currentPane: () => focused,
  };
  const registered = sandbox.termlabVimMode.registerExCommands(deps);

  const byName = (name) => cm.CM.Vim.defined.find((entry) => entry.name === name);
  return {
    sandbox,
    cm,
    registered,
    saves,
    closes,
    toasts,
    deps,
    byName,
    focusedPane: () => focused,
    setFocused: (pane) => { focused = pane; },
    // The real engine calls the handler as func(cm, params).
    run: (name) => byName(name).func({ /* cm adapter */ }, { commandName: name, input: name }),
  };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

const results = [];
function check(name, fn) { results.push({ name, fn }); }

// --- vimExtensions ---------------------------------------------------------
check('vimExtensions(false) contributes nothing', () => {
  const { sandbox } = loadModules([VIM_MODE]);
  const ext = sandbox.termlabVimMode.vimExtensions(false);
  assert.ok(Array.isArray(ext), 'an array, so it can be spread into the list');
  assert.strictEqual(ext.length, 0);
});

check('vimExtensions(true) contributes vim()', () => {
  const { sandbox } = loadModules([VIM_MODE]);
  const ext = sandbox.termlabVimMode.vimExtensions(true);
  assert.strictEqual(ext.length, 1);
  // Cross-realm: compare the field, never the object identity or prototype.
  assert.strictEqual(ext[0].ext, 'vim');
});

check('vimExtensions(true) is empty when the bundle has no vim export', () => {
  const { sandbox } = loadModules([VIM_MODE]);
  delete sandbox.CM6.vim;
  assert.strictEqual(sandbox.termlabVimMode.vimExtensions(true).length, 0);
});

check('enabled-with-no-engine is LOUD, once — a stale bundle must not look like vim-off', () => {
  // A checkout can carry a stale generated bundle whose CM6 predates the vim
  // export; that shipped once and read as "the Settings toggle is broken".
  const { sandbox } = loadModules([VIM_MODE]);
  delete sandbox.CM6.vim;
  const errors = [];
  const toasts = [];
  sandbox.console = { ...console, error: (...a) => errors.push(a.join(' ')) };
  sandbox.toast = { error: (title, body) => toasts.push({ title, body }) };

  sandbox.termlabVimMode.vimExtensions(true);
  assert.strictEqual(errors.length, 1, 'console.error fired');
  assert.ok(/build:vendor/.test(errors[0]), 'the message names the fix');
  assert.strictEqual(toasts.length, 1, 'the user is told, not just the console');
  assert.ok(/stale/i.test(toasts[0].body), 'the toast says the bundle is stale');

  sandbox.termlabVimMode.vimExtensions(true);
  assert.strictEqual(errors.length, 1, 'warned once, not per reconfigure');
  assert.strictEqual(toasts.length, 1);

  // vim-off stays silent — [] is a real answer there, not a failure.
  errors.length = 0;
  sandbox.termlabVimMode.vimExtensions(false);
  assert.strictEqual(errors.length, 0, 'disabled never warns');
});

// --- registerExCommands ----------------------------------------------------
check('registers write, quit and wq with vim\'s own short prefixes', () => {
  const h = makeExHarness();
  assert.strictEqual(h.registered, true);
  const names = h.cm.CM.Vim.defined.map((e) => e.name);
  assert.deepStrictEqual(names.slice().sort(), ['quit', 'wq', 'write']);
  assert.strictEqual(h.byName('write').prefix, 'w', ':w must reach it');
  assert.strictEqual(h.byName('quit').prefix, 'q', ':q must reach it');
  assert.strictEqual(h.byName('wq').prefix, 'wq');
  for (const entry of h.cm.CM.Vim.defined) {
    assert.strictEqual(typeof entry.func, 'function');
  }
});

check(':w saves the focused pane through savePane', async () => {
  const h = makeExHarness();
  h.run('write');
  await tick();
  assert.strictEqual(h.saves.length, 1);
  assert.strictEqual(h.saves[0], h.focusedPane(), 'the pane currentPane() reported');
  assert.strictEqual(h.closes.length, 0, ':w never closes anything');
});

check(':w toasts and does not throw when the save fails', async () => {
  const h = makeExHarness({ saveRejection: 'Permission denied (os error 13)' });
  h.run('write');
  await tick();
  assert.strictEqual(h.saves.length, 1);
  assert.strictEqual(h.toasts.length, 1);
  assert.strictEqual(h.toasts[0][0], 'Save Failed');
  assert.match(h.toasts[0][1], /Permission denied/);
});

check(':q closes the tab through the guarded closeTab, not a view destroy', async () => {
  const h = makeExHarness();
  h.run('quit');
  await tick();
  assert.deepStrictEqual(h.closes, [3], "the focused pane's tabId");
  assert.strictEqual(h.saves.length, 0, ':q must not write anything');
  assert.strictEqual(
    h.focusedPane().view.destroyed,
    undefined,
    'nothing bypassed the guard by destroying the view directly',
  );
});

check(':wq saves first, then closes', async () => {
  const order = [];
  const { sandbox, cm } = loadModules([VIM_MODE]);
  const pane = { paneId: 1, tabId: 11, kind: 'editor', view: {} };
  sandbox.termlabVimMode.registerExCommands({
    savePane: (p) => { order.push(['save', p.tabId]); return Promise.resolve(); },
    closeTab: (id) => { order.push(['close', id]); return Promise.resolve(); },
    currentPane: () => pane,
  });
  cm.CM.Vim.defined.find((e) => e.name === 'wq').func({}, {});
  await tick();
  assert.deepStrictEqual(order, [['save', 11], ['close', 11]]);
});

check(':wq does not close when the save fails', async () => {
  const h = makeExHarness({ saveRejection: 'disk full' });
  h.run('wq');
  await tick();
  assert.strictEqual(h.saves.length, 1);
  assert.deepStrictEqual(h.closes, [], 'a failed save is not consent to lose the file');
  assert.strictEqual(h.toasts[0][0], 'Save Failed');
});

check('every handler is a no-op when no pane is focused', async () => {
  const h = makeExHarness({ pane: null });
  h.run('write');
  h.run('quit');
  h.run('wq');
  await tick();
  assert.deepStrictEqual(h.saves, []);
  assert.deepStrictEqual(h.closes, []);
});

check('every handler is a no-op when the focused pane is a terminal', async () => {
  const h = makeExHarness({ pane: { paneId: 2, tabId: 5, kind: 'terminal', term: {} } });
  h.run('write');
  h.run('quit');
  h.run('wq');
  await tick();
  assert.deepStrictEqual(h.saves, []);
  assert.deepStrictEqual(h.closes, [], 'a stray :q must never close a terminal tab');
});

check('a pane with no view is not saved or closed', async () => {
  const h = makeExHarness({ pane: { paneId: 3, tabId: 6, kind: 'editor', view: null } });
  h.run('write');
  h.run('quit');
  await tick();
  assert.deepStrictEqual(h.saves, []);
  assert.deepStrictEqual(h.closes, []);
});

check('registerExCommands reports false when the bundle has no Vim engine', () => {
  const { sandbox } = loadModules([VIM_MODE]);
  delete sandbox.CM6.Vim;
  assert.strictEqual(
    sandbox.termlabVimMode.registerExCommands({ savePane() {}, closeTab() {}, currentPane: () => null }),
    false,
  );
});

// --- editor-pane wiring ----------------------------------------------------
//
// The ordering the feature hangs on: CodeMirror resolves keymaps in extension
// order, so vim's keymap has to precede CM.keymap.of([...defaultKeymap]).
// Put it after, and `i` inserts an "i" instead of entering insert mode.
function makePaneHarness(paneOptions) {
  const { sandbox, cm } = loadModules([VIM_MODE, EDITOR_PANE]);
  const host = sandbox.document.createElement('div');
  const view = sandbox.termlabEditorPane.createEditorView(host, paneOptions || {});
  const extensions = view.state.spec.extensions;
  return { sandbox, cm, view, extensions };
}

function indexOfKeymap(extensions) {
  return extensions.findIndex((e) => e && e.ext === 'keymap');
}

check('the vim compartment is the first extension, ahead of the default keymap', () => {
  const h = makePaneHarness({ doc: 'x', vimMode: true });
  // Located by content, not by assuming index 0 — the point of the check is
  // WHERE it landed, so the position has to be found, not asserted into being.
  const vimAt = h.extensions.findIndex(
    (e) => e && Array.isArray(e.contents) && e.contents.some((c) => c && c.ext === 'vim'),
  );
  assert.ok(vimAt >= 0, 'the vim extension made it into the list at all');
  assert.strictEqual(vimAt, 0, 'and it is first');
  assert.ok(
    typeof h.extensions[vimAt].compartment === 'number',
    'carried by a compartment, so it can be reconfigured live',
  );
  const keymapAt = indexOfKeymap(h.extensions);
  assert.ok(keymapAt >= 0, 'the default keymap is in the list');
  assert.ok(
    vimAt < keymapAt,
    `vim must precede CM.keymap.of([...defaultKeymap]) (vim at ${vimAt}, keymap at ${keymapAt})`,
  );
});

check('with vim off the first extension is still the compartment, but empty', () => {
  const h = makePaneHarness({ doc: 'x', vimMode: false });
  const first = h.extensions[0];
  assert.ok(first && typeof first.compartment === 'number');
  assert.deepStrictEqual(first.contents.length, 0);
});

check('setVimMode(view, true) reconfigures that same compartment', () => {
  const h = makePaneHarness({ doc: 'x', vimMode: false });
  const compartmentId = h.extensions[0].compartment;
  h.sandbox.termlabEditorPane.setVimMode(h.view, true);
  assert.strictEqual(h.view.effects.length, 1, 'exactly one dispatch');
  const effect = h.view.effects[0];
  assert.strictEqual(effect.reconfigure, compartmentId, 'the vim compartment, not the font one');
  assert.strictEqual(effect.contents.length, 1);
  assert.strictEqual(effect.contents[0].ext, 'vim');
});

check('setVimMode(view, false) empties the compartment', () => {
  const h = makePaneHarness({ doc: 'x', vimMode: true });
  h.sandbox.termlabEditorPane.setVimMode(h.view, false);
  assert.strictEqual(h.view.effects.length, 1);
  assert.strictEqual(h.view.effects[0].reconfigure, h.extensions[0].compartment);
  assert.strictEqual(h.view.effects[0].contents.length, 0);
});

check('setVimMode on a view it does not know is a no-op', () => {
  const h = makePaneHarness({ doc: 'x' });
  const effects = [];
  const stranger = { dispatch: (tr) => effects.push(tr) };
  h.sandbox.termlabEditorPane.setVimMode(stranger, true);
  assert.strictEqual(effects.length, 0);
});

check('setReadOnly gates a live editor through its own compartment during ownership close', () => {
  const h = makePaneHarness({ doc: 'x' });
  h.sandbox.termlabEditorPane.setReadOnly(h.view, true);
  assert.strictEqual(h.view.effects.length, 1);
  assert.strictEqual(h.view.effects[0].contents.ext, 'readOnly');
  assert.strictEqual(h.view.effects[0].contents.value, true);
  h.view.termlabSetReadOnly(false);
  assert.strictEqual(h.view.effects[1].reconfigure, h.view.effects[0].reconfigure);
  assert.strictEqual(h.view.effects[1].contents.value, false);
});

check('the font, theme and language compartments still work alongside it', () => {
  const h = makePaneHarness({ doc: 'x', vimMode: true });
  const ids = h.extensions.filter((e) => e && typeof e.compartment === 'number').map((e) => e.compartment);
  assert.strictEqual(
    new Set(ids).size,
    5,
    'vim, language, read-only, theme and font are five distinct compartments',
  );
  h.sandbox.termlabEditorPane.setFontSize(h.view, 15);
  assert.strictEqual(h.view.effects.length, 1);
  assert.notStrictEqual(
    h.view.effects[0].reconfigure,
    h.extensions[0].compartment,
    'setFontSize must not touch the vim compartment',
  );
});

// --- the language compartment (Save As renames a live pane) ----------------
//
// The language used to be fixed at creation. Save As changes a pane's name
// while it is open, so it has to be reconfigurable — through a compartment
// rather than a fresh EditorState, which would throw away the document, the
// selection and the undo history.
// Builds the pane with the app's REAL filename -> language table already
// loaded and a language export present on CM6, so the compartment's INITIAL
// contents are the ones the app would really compute. (makePaneHarness cannot
// do this: it loads the map after the view exists, by which time the initial
// derivation has already run against no map at all.)
function makeLanguagePaneHarness(filename) {
  const { sandbox, cm } = loadModules([VIM_MODE, LANGUAGE_MAP, EDITOR_PANE]);
  // languageKeyFor('deploy.py') -> 'python', which editor-pane resolves as an
  // export name on window.CM6 — the @codemirror/lang-* FUNCTION shape.
  cm.CM.python = () => ({ ext: 'python' });
  const host = sandbox.document.createElement('div');
  const view = sandbox.termlabEditorPane.createEditorView(host, { doc: 'x', filename });
  return { sandbox, cm, view, extensions: view.state.spec.extensions };
}

// Located structurally: the compartment that follows the default keymap is the
// language one (vim is ahead of the keymap; theme and font follow it).
function languageCompartmentAt(extensions) {
  const keymapAt = indexOfKeymap(extensions);
  const at = extensions.findIndex(
    (e, i) => i > keymapAt && e && typeof e.compartment === 'number',
  );
  assert.ok(at > keymapAt, 'the language extension is carried by a compartment');
  return at;
}

// Moving the language into a compartment created a regression that costs
// nothing to make and is invisible everywhere else: `languageComp.of([])`
// instead of `languageComp.of(languageExtension(opts.filename))`. Every file
// would then open with NO highlighting while every suite stayed green — the
// compartment slot is still there, setLanguage still works, and only a file
// opened and never renamed shows the damage. So the compartment's INITIAL
// contents are pinned, not just its existence.
check('a pane opens with the language its filename derives, in the compartment', () => {
  const h = makeLanguagePaneHarness('deploy.py');
  const at = languageCompartmentAt(h.extensions);
  const initial = h.extensions[at].contents;
  assert.ok(Array.isArray(initial), 'the compartment holds an extension array');
  assert.strictEqual(
    initial.length,
    1,
    'a .py file opens WITH highlighting — an empty compartment here means every '
    + 'freshly opened file loses its language until it is renamed',
  );
  assert.strictEqual(initial[0].ext, 'python', 'and it is the one languageKeyFor names');
});

check('a pane whose filename has no known language opens with an empty compartment', () => {
  const h = makeLanguagePaneHarness('notes.unknownext');
  const at = languageCompartmentAt(h.extensions);
  assert.strictEqual(h.extensions[at].contents.length, 0, 'nothing, rather than a wrong mode');
  // The converse of the check above: it is the FILENAME that decides, so the
  // non-empty case cannot be passing for some reason other than the derivation.
  const py = makeLanguagePaneHarness('deploy.py');
  assert.strictEqual(py.extensions[languageCompartmentAt(py.extensions)].contents.length, 1);
});

check('setLanguage re-derives highlighting on a live view, via its own compartment', () => {
  const h = makeLanguagePaneHarness('notes.txt');
  const languageAt = languageCompartmentAt(h.extensions);
  const languageId = h.extensions[languageAt].compartment;
  assert.notStrictEqual(languageId, h.extensions[0].compartment, 'not the vim one');

  h.sandbox.termlabEditorPane.setLanguage(h.view, 'deploy.py');
  assert.strictEqual(h.view.effects.length, 1, 'exactly one dispatch');
  assert.strictEqual(h.view.effects[0].reconfigure, languageId, 'the language compartment');
  assert.strictEqual(h.view.effects[0].contents.length, 1);
  assert.strictEqual(h.view.effects[0].contents[0].ext, 'python', 'derived from the NEW name');

  // A name with no known language empties it — how a .py saved as .txt
  // correctly loses its highlighting.
  h.sandbox.termlabEditorPane.setLanguage(h.view, 'plain.unknownext');
  assert.strictEqual(h.view.effects[1].reconfigure, languageId);
  assert.strictEqual(h.view.effects[1].contents.length, 0);
});

check('setLanguage on a view it does not know is a no-op', () => {
  const h = makePaneHarness({ doc: 'x' });
  const effects = [];
  h.sandbox.termlabEditorPane.setLanguage({ dispatch: (tr) => effects.push(tr) }, 'a.py');
  assert.strictEqual(effects.length, 0);
});

// --- gd / gD: Go to Definition ---------------------------------------------
//
// Driven through the REAL engine: `Vim.findKey` is what CodeMirror calls for
// every key in normal mode, so matching `g` then `d` here is the same match the
// app performs. A DOM handler could not do this — vim's ViewPlugin consumes
// normal-mode keys, and the LSP surfaces' Prec.highest handlers deliberately
// fall through when nothing of theirs is open.

// A sandbox whose CM6 carries the real Vim engine, plus the real vim-mode.js.
function makeNavHarness(options = {}) {
  const { sandbox, cm } = loadModules([VIM_MODE]);
  cm.CM.Vim = REAL_VIM.Vim;
  const jumps = [];
  const calls = {
    back: 0, forward: 0, hovers: [], nextProblem: 0, previousProblem: 0, recorded: [],
  };
  const deps = {
    goToDefinition: (view) => { jumps.push(view); return Promise.resolve('navigated'); },
    navigateBack: () => { calls.back += 1; return Promise.resolve('navigated'); },
    navigateForward: () => { calls.forward += 1; return Promise.resolve('navigated'); },
    showHover: (view) => { calls.hovers.push(view); return Promise.resolve(true); },
    nextDiagnostic: () => { calls.nextProblem += 1; },
    previousDiagnostic: () => { calls.previousProblem += 1; },
    recordJump: (view, position) => { calls.recorded.push({ view, position }); return true; },
  };
  for (const omitted of options.omit || []) delete deps[omitted];
  const registered = sandbox.termlabVimMode.registerNavigationCommands(deps);
  return {
    sandbox, cm, registered, jumps, calls,
  };
}

check('the shipped vim keymap binds neither gd nor gD, so nothing is taken away', () => {
  const source = fs.readFileSync(VIM_CORE_KEYMAP, 'utf8');
  const entries = source.match(/\{ *keys: *'[^']+'/g) || [];
  assert.ok(entries.length > 100, 'the default keymap was found at all');
  for (const spelling of ["keys: 'gd'", "keys: 'gD'"]) {
    assert.ok(
      !source.includes(spelling),
      `the package now binds ${spelling} itself — check what it does before overriding it`,
    );
  }
});

check('gd in normal mode asks for the definition at the cursor', async () => {
  const h = makeNavHarness();
  assert.strictEqual(h.registered, true);
  const view = { id: 'the-view' };
  const command = typeKeys(vimAdapter(view), ['g', 'd']);
  assert.strictEqual(typeof command, 'function', 'the engine matched gd to a command');
  command();
  await tick();
  assert.strictEqual(h.jumps.length, 1, 'exactly one definition request');
  assert.strictEqual(h.jumps[0], view, 'for the view the keystroke was typed in');
});

check('gD does the same, since declaration folds into definition here', async () => {
  const h = makeNavHarness();
  const view = { id: 'other-view' };
  const command = typeKeys(vimAdapter(view), ['g', 'D']);
  assert.strictEqual(typeof command, 'function');
  command();
  await tick();
  assert.deepStrictEqual(h.jumps, [view]);
});

check('Ctrl-] jumps too, vim\'s own tag idiom', async () => {
  const h = makeNavHarness();
  const view = { id: 'tag-view' };
  const command = typeKeys(vimAdapter(view), ['<C-]>']);
  assert.strictEqual(typeof command, 'function', 'the engine matched <C-]>');
  command();
  await tick();
  assert.deepStrictEqual(h.jumps, [view]);
});

check('the real CodeMirror 6 adapter hands an action its view as cm6', () => {
  const types = fs.readFileSync(
    path.join(NODE_MODULES, '@replit/codemirror-vim/dist/index.d.ts'), 'utf8',
  );
  assert.ok(
    /cm6: EditorView/.test(types),
    'the action reads cm.cm6 — if the package renames it, gd silently stops working',
  );
});

check('the jump is deferred, so it never runs inside vim\'s own operation', async () => {
  const h = makeNavHarness();
  const command = typeKeys(vimAdapter({ id: 'deferred' }), ['g', 'd']);
  command();
  assert.strictEqual(h.jumps.length, 0, 'not synchronously inside the operation');
  await tick();
  assert.strictEqual(h.jumps.length, 1);
});

// --- Ctrl-O / Ctrl-I: one history, vim semantics ----------------------------
//
// vim's own <C-o>/<C-i> walk a jumplist whose entries are per-DOCUMENT
// bookmarks, fed only by vim motions — it can neither see a gd into another
// file nor switch tabs, which is exactly why the owner pressed Ctrl-O after a
// gd and nothing happened. These keys are remapped onto the window's own
// cross-file history instead.

check('Ctrl-O and Ctrl-I walk the window history, not vim\'s per-document one', async () => {
  const h = makeNavHarness();
  const adapter = vimAdapter({ id: 'view' });
  typeKeys(adapter, ['<C-o>'])();
  typeKeys(adapter, ['<C-i>'])();
  await tick();
  assert.strictEqual(h.calls.back, 1, 'Ctrl-O went back');
  assert.strictEqual(h.calls.forward, 1, 'Ctrl-I went forward');
});

check('a vim jump motion is absorbed into that same history', async () => {
  const h = makeNavHarness();
  const view = { id: 'jump-view' };
  const adapter = vimAdapter(view);
  // The engine's own hook, called by every `toJumplist` motion — G, gg, {, },
  // /search, n/N, marks, %, H/M/L.
  REAL_VIM.Vim.getVimGlobalState_().jumpList.add(
    adapter, { line: 3, ch: 2 }, { line: 41, ch: 0 },
  );
  assert.strictEqual(h.calls.recorded.length, 1, 'the pre-motion position was recorded');
  assert.strictEqual(h.calls.recorded[0].view, view);
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.calls.recorded[0].position)), { line: 3, character: 2 },
    'vim reports ch; the history speaks LSP characters',
  );
});

check('absorbing does not break vim\'s own jumplist bookkeeping', () => {
  makeNavHarness();
  const adapter = vimAdapter({ id: 'native' });
  const jumpList = REAL_VIM.Vim.getVimGlobalState_().jumpList;
  jumpList.add(adapter, { line: 1, ch: 0 }, { line: 9, ch: 0 });
  const mark = jumpList.find(adapter, -1);
  assert.ok(mark, 'the engine still recorded its own entry underneath');
});

check('a real G motion feeds the history through the engine', async () => {
  const h = makeNavHarness();
  const view = { id: 'motion-view' };
  const adapter = vimAdapter(view);
  const command = typeKeys(adapter, ['G']);
  assert.strictEqual(typeof command, 'function', 'the engine matched G');
  command();
  assert.ok(h.calls.recorded.length >= 1, 'G is a jump, and jumps are remembered');
  assert.strictEqual(h.calls.recorded[0].view, view);
});

// --- the parity keys --------------------------------------------------------

check('K asks for hover at the cursor', async () => {
  const h = makeNavHarness();
  const view = { id: 'hover-view' };
  typeKeys(vimAdapter(view), ['K'])();
  await tick();
  assert.deepStrictEqual(h.calls.hovers, [view]);
});

check(']d and [d step through diagnostics', async () => {
  const h = makeNavHarness();
  const adapter = vimAdapter({ id: 'diag-view' });
  const next = typeKeys(adapter, [']', 'd']);
  assert.strictEqual(typeof next, 'function', 'our ]d wins over the package\'s ]<character>');
  next();
  typeKeys(adapter, ['[', 'd'])();
  await tick();
  assert.strictEqual(h.calls.nextProblem, 1);
  assert.strictEqual(h.calls.previousProblem, 1);
});

check('a key whose feature was not wired stays out of this window', async () => {
  const h = makeNavHarness({ omit: ['showHover'] });
  typeKeys(vimAdapter({ id: 'no-hover' }), ['K'])();
  await tick();
  assert.deepStrictEqual(h.calls.hovers, [], 'no hover was requested from this registration');
});

check('Ctrl-I is not Tab in a webview, so insert-mode Tab is untouched', () => {
  const key = (event) => REAL_VIM.Vim.vimKeyFromEvent(event, {});
  assert.strictEqual(key({ key: 'i', ctrlKey: true }), '<C-i>');
  assert.strictEqual(key({ key: 'Tab' }), '<Tab>', 'a Tab keydown is its own key, not ^I');
  const source = fs.readFileSync(VIM_MODE, 'utf8');
  assert.ok(!/'<Tab>'/.test(source), 'nothing here claims Tab');
  assert.ok(
    !/context: 'insert'/.test(source),
    'every mapping is normal-mode, so insert-mode completion and indent are untouched',
  );
});

check('registerNavigationCommands reports false with no engine and with no dependency', () => {
  const withoutEngine = loadModules([VIM_MODE]);
  withoutEngine.cm.CM.Vim = undefined;
  assert.strictEqual(
    withoutEngine.sandbox.termlabVimMode.registerNavigationCommands({ goToDefinition() {} }),
    false,
  );
  const withoutDep = loadModules([VIM_MODE]);
  withoutDep.cm.CM.Vim = REAL_VIM.Vim;
  assert.strictEqual(withoutDep.sandbox.termlabVimMode.registerNavigationCommands({}), false);
});

check('the compose runtime hands vim every feature it maps, and no view or pane map', () => {
  const source = fs.readFileSync(COMPOSE, 'utf8');
  assert.ok(/registerNavigationCommands/.test(source), 'the binding is wired at composition');
  const at = source.indexOf('registerNavigationCommands');
  const block = source.slice(at, at + 2600);
  for (const dep of [
    'goToDefinition', 'navigateBack', 'navigateForward', 'recordJump',
    'showHover', 'nextDiagnostic', 'previousDiagnostic',
  ]) {
    assert.ok(new RegExp(`${dep}:`).test(block), `${dep} is not wired`);
  }
  assert.ok(/termlabLspNavigation/.test(block), 'navigation comes from the navigator');
  assert.ok(/termlabLspTooltips/.test(block), 'K goes to the hover controller');
  assert.ok(
    /termlab:editor-next-problem/.test(block) && /termlab:editor-previous-problem/.test(block),
    ']d and [d ride the same events F8 does, so both reach problems-navigation',
  );
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
  console.log(`editor vim glue: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`editor vim glue: all ${results.length} checks passed`);
}
