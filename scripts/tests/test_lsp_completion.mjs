// Run: node scripts/tests/test_lsp_completion.mjs
//
// LSP-backed CodeMirror completion.
//
// Two halves, deliberately:
//
//   * the REAL half — every test that depends on how CodeMirror or
//     @replit/codemirror-vim actually behaves imports those packages from
//     crates/termlab_tauri/frontend/node_modules and asserts on real
//     behaviour: the document text after an edit lands, the selection a
//     snippet ends up with, the order the view would run keydown handlers in.
//     A stub cannot fail these, which is the point — an earlier round of this
//     file passed with all of those bugs live.
//
//   * the stub half — request gating, staleness and item translation, where
//     there is no CodeMirror behaviour involved and a stub is honest.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const MODULES = path.join(APP, 'features/editor');
// Position conversion lives in its own module now (lsp-position.js); every
// harness that loads a converting module loads it too, the way index.html does.
const POSITION = path.join(MODULES, 'lsp-position.js');
const APPLY = path.join(MODULES, 'lsp-completion-apply.js');
const COMPLETION = path.join(MODULES, 'lsp-completion.js');
const BRIDGE = path.join(MODULES, 'lsp-bridge.js');
const EDITOR_PANE = path.join(MODULES, 'editor-pane.js');
const VIM_MODE = path.join(MODULES, 'vim-mode.js');
const VENDOR_ENTRY = path.join(ROOT, 'vendor-entry.mjs');
const PACKAGE_JSON = path.join(ROOT, 'package.json');
const INDEX_HTML = path.join(ROOT, 'index.html');
const COMPOSE = path.join(APP, 'manager-compose-runtime.js');
const NODE_MODULES = path.join(ROOT, 'node_modules');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

// --- fake DOM (documentation rendering only) -------------------------------
function makeDocument() {
  function element(tag) {
    return {
      tagName: String(tag).toUpperCase(),
      className: '',
      textContent: '',
      children: [],
      style: {},
      classList: { add() {}, remove() {} },
      appendChild(child) { this.children.push(child); return child; },
      // Deliberately absent: innerHTML. An info panel that reaches for it is
      // rendering server text as markup.
    };
  }
  return { createElement: (tag) => element(tag) };
}

// --- the real CodeMirror packages ------------------------------------------
//
// Mirrors vendor-entry.mjs. `completionStatus` is wrapped so a test can say
// "the popup is open" without needing a live EditorView (which needs a DOM
// this repo has no jsdom for); everything else is the shipped code.
async function loadRealCM() {
  const pkg = async (name) => import(
    pathToFileURL(path.join(NODE_MODULES, name, 'dist/index.js')).href
  );
  const state = await pkg('@codemirror/state');
  const view = await pkg('@codemirror/view');
  const autocomplete = await pkg('@codemirror/autocomplete');
  const vim = await pkg('@replit/codemirror-vim');
  return { state, view, autocomplete, vim };
}
const REAL = await loadRealCM();

// The facet every ViewPlugin lands in, obtained from the same realm the
// extensions are built in — this is the list `computeHandlers` walks to
// decide which keydown handler runs first.
const VIEW_PLUGIN_FACET = REAL.view.ViewPlugin.define(() => ({})).extension[0].facet;

function keydownOwners(state) {
  return state.facet(VIEW_PLUGIN_FACET)
    .filter((entry) => entry.plugin.domEventHandlers && entry.plugin.domEventHandlers.keydown)
    .map((entry) => entry.plugin.id);
}

function realCM6(overrides) {
  const { state, view, autocomplete, vim } = REAL;
  return {
    EditorState: state.EditorState,
    Compartment: state.Compartment,
    Prec: state.Prec,
    ChangeSet: state.ChangeSet,
    StateEffect: state.StateEffect,
    EditorView: view.EditorView,
    keymap: view.keymap,
    autocompletion: autocomplete.autocompletion,
    startCompletion: autocomplete.startCompletion,
    closeCompletion: autocomplete.closeCompletion,
    acceptCompletion: autocomplete.acceptCompletion,
    moveCompletionSelection: autocomplete.moveCompletionSelection,
    completionStatus: autocomplete.completionStatus,
    insertCompletionText: autocomplete.insertCompletionText,
    snippet: autocomplete.snippet,
    vim: vim.vim,
    Vim: vim.Vim,
    ...(overrides || {}),
  };
}

// A view with a REAL EditorState behind it. dispatch applies the transaction
// the way an EditorView would, so a test can assert the resulting text.
function realView(doc) {
  const view = {
    state: REAL.state.EditorState.create({ doc }),
    dispatches: 0,
    dispatch(spec) {
      this.dispatches += 1;
      this.state = this.state.update(spec).state;
    },
  };
  return view;
}

function text(view) { return view.state.doc.toString(); }
function selected(view) {
  const { main } = view.state.selection;
  return view.state.doc.sliceString(main.from, main.to);
}

// --- CM6 stand-in (stub half) ----------------------------------------------
function makeStubCM(sandbox) {
  const calls = [];
  let status = null;
  const CM = {
    Prec: { highest: (ext) => ({ ext: 'prec', level: 'highest', contents: ext }) },
    keymap: { of: (bindings) => ({ ext: 'keymap', bindings }) },
    ChangeSet: { of: () => ({ ext: 'changeSet' }) },
    StateEffect: { mapEffects: (effects) => effects },
    Compartment: class Compartment {
      constructor() { this.id = Math.random(); }
      of(ext) { return { compartment: this.id, contents: ext }; }
      reconfigure(ext) { return { reconfigure: this.id, contents: ext }; }
    },
    EditorView: Object.assign(
      function EditorView(config) {
        this.config = config;
        this.state = config && config.state;
        this.dispatch = () => {};
        this.destroy = () => {};
      },
      {
        updateListener: { of: (fn) => ({ ext: 'updateListener', fn }) },
        theme: (spec) => ({ ext: 'theme', spec }),
        domEventHandlers: (handlers) => ({ ext: 'domEventHandlers', handlers }),
      },
    ),
    EditorState: {
      create: (spec) => ({ spec, doc: { toString: () => spec.doc } }),
      readOnly: { of: (value) => ({ ext: 'readOnly', value }) },
    },
    autocompletion: (config) => ({ ext: 'autocompletion', config }),
    startCompletion: (view) => { calls.push(['startCompletion', view]); return true; },
    closeCompletion: (view) => { calls.push(['closeCompletion', view]); return status !== null; },
    acceptCompletion: (view) => { calls.push(['acceptCompletion', view]); return status !== null; },
    completionStatus: () => status,
    moveCompletionSelection: (forward, by) => (view) => {
      calls.push(['moveCompletionSelection', forward, by || 'option', view]);
      return status !== null;
    },
    insertCompletionText: (state, insert, from, to) => ({
      changes: [{ from, to, insert }], userEvent: 'input.complete',
    }),
    snippet: () => () => {},
    lineNumbers: () => ({ ext: 'lineNumbers' }),
    highlightActiveLineGutter: () => ({ ext: 'highlightActiveLineGutter' }),
    highlightSpecialChars: () => ({ ext: 'highlightSpecialChars' }),
    history: () => ({ ext: 'history' }),
    foldGutter: () => ({ ext: 'foldGutter' }),
    drawSelection: () => ({ ext: 'drawSelection' }),
    rectangularSelection: () => ({ ext: 'rectangularSelection' }),
    indentOnInput: () => ({ ext: 'indentOnInput' }),
    bracketMatching: () => ({ ext: 'bracketMatching' }),
    highlightActiveLine: () => ({ ext: 'highlightActiveLine' }),
    highlightSelectionMatches: () => ({ ext: 'highlightSelectionMatches' }),
    StreamLanguage: { define: (parser) => ({ ext: 'streamLanguage', parser }) },
    defaultKeymap: ['defaultKeymap'],
    historyKeymap: ['historyKeymap'],
    searchKeymap: ['searchKeymap'],
    foldKeymap: ['foldKeymap'],
    indentWithTab: 'indentWithTab',
    vim: () => ({ ext: 'vim' }),
    Vim: { defineEx() {} },
  };
  sandbox.CM6 = CM;
  return { CM, calls, setStatus: (value) => { status = value; } };
}

// --- document stand-in (stub half) -----------------------------------------
function makeDoc(value) {
  const lines = String(value).split('\n');
  const starts = [];
  let at = 0;
  for (const line of lines) {
    starts.push(at);
    at += line.length + 1;
  }
  return {
    lines: lines.length,
    length: String(value).length,
    line(number) {
      const index = Math.min(Math.max(number, 1), lines.length) - 1;
      return {
        number: index + 1,
        from: starts[index],
        to: starts[index] + lines[index].length,
        text: lines[index],
      };
    },
    lineAt(offset) {
      let index = 0;
      for (let i = 0; i < lines.length; i += 1) if (offset >= starts[i]) index = i;
      return this.line(index + 1);
    },
    toString() { return String(value); },
  };
}

// --- sandbox ---------------------------------------------------------------
function load(files, extra, cmFactory) {
  const sandbox = {
    console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap, Array, Object,
    JSON, Date, RegExp, String, Number, Boolean, Math, Error,
  };
  sandbox.window = sandbox;
  sandbox.document = makeDocument();
  const windowListeners = new Map();
  sandbox.addEventListener = (type, handler) => {
    if (!windowListeners.has(type)) windowListeners.set(type, []);
    windowListeners.get(type).push(handler);
  };
  sandbox.removeEventListener = (type, handler) => {
    const list = windowListeners.get(type) || [];
    const at = list.indexOf(handler);
    if (at >= 0) list.splice(at, 1);
  };
  sandbox.CustomEvent = class CustomEvent {
    constructor(type, init) { this.type = type; this.detail = init && init.detail; }
  };
  sandbox.dispatchEvent = (event) => {
    for (const handler of (windowListeners.get(event.type) || []).slice()) handler(event);
    return true;
  };
  sandbox.toast = { error() {}, success() {}, info() {}, warn() {} };
  const cm = cmFactory ? cmFactory(sandbox) : makeStubCM(sandbox);
  Object.assign(sandbox, extra || {});
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return { sandbox, cm, windowListeners };
}

const DEFAULT_STATUS = {
  revision: 3,
  documentId: 'doc-1',
  sessionId: 'session-1',
  adapterId: 'typescript',
  projectRootUri: 'file:///repo',
  state: 'ready',
  message: null,
  capabilities: {
    completion: true, hover: true, signatureHelp: true, definition: true, diagnostics: true,
  },
  errorCount: 0,
  warningCount: 0,
  pathname: '/repo/main.ts',
};

// A pane, its lsp-state record and a stubbed request path, wired the way
// manager-compose-runtime wires them in the app. `real: true` swaps the CM6
// stub for the shipped packages and gives the pane a real EditorState.
function harness(options = {}) {
  const opts = options || {};
  const docText = opts.text === undefined ? 'const value = 1;\nva' : opts.text;
  const cmFactory = opts.real
    ? (sandbox) => {
      const calls = [];
      let forcedStatus;
      // Everything here is the shipped package except three functions that
      // cannot run headlessly: `completionStatus` reads a state field only a
      // live EditorView installs, and the three commands act on that field.
      // They are spied so the guards and the key mapping around them can be
      // asserted; the ORDERING test below uses no spies at all.
      const CM = realCM6({
        completionStatus: (state) => (
          forcedStatus === undefined ? REAL.autocomplete.completionStatus(state) : forcedStatus
        ),
        startCompletion: (view) => { calls.push(['startCompletion', view]); return true; },
        closeCompletion: (view) => { calls.push(['closeCompletion', view]); return true; },
        acceptCompletion: (view) => { calls.push(['acceptCompletion', view]); return true; },
      });
      sandbox.CM6 = CM;
      return {
        CM,
        calls,
        setStatus: (value) => { forcedStatus = value === undefined ? undefined : value; },
      };
    }
    : undefined;
  const { sandbox, cm, windowListeners } = load([POSITION, APPLY, COMPLETION], null, cmFactory);

  const view = opts.real
    ? realView(docText)
    : {
      state: { doc: makeDoc(docText) },
      dispatches: 0,
      dispatch(spec) { this.dispatches += 1; this.dispatched.push(spec); },
      dispatched: [],
    };
  if (!opts.real) view.dispatched = view.dispatched || [];

  const pane = {
    paneId: 4,
    tabId: 2,
    kind: 'editor',
    view,
    filePath: '/repo/main.ts',
    remote: opts.remote || null,
  };
  const status = opts.status === undefined
    ? { ...DEFAULT_STATUS, completionTriggerCharacters: opts.triggerCharacters }
    : opts.status;
  const documentState = opts.documentState === undefined
    ? {
      documentId: 'doc-1',
      version: 7,
      projectCandidates: [],
      selectedRoot: '/repo',
      trust: 'trusted',
      // lsp-state.js mirrors the status's capabilities into the record, so the
      // harness has to as well or a "cannot complete" case is not one.
      capabilities: status.capabilities,
      status,
      diagnosticsRevision: 0,
    }
    : opts.documentState;

  sandbox.termlabLspState = {
    get: (candidate) => (candidate === pane ? documentState : null),
  };
  const requests = [];
  sandbox.termlabLspBridge = opts.bridge || {};
  sandbox.termlabEditorService = {
    requestFeature: async (target, kind, position, trigger) => {
      requests.push({ target, kind, position, trigger });
      if (typeof opts.respond === 'function') return opts.respond({ position, trigger });
      return opts.response === undefined ? null : opts.response;
    },
  };

  const completion = sandbox.termlabLspCompletion;
  completion.configure({
    paneForView: (candidate) => (candidate === view ? pane : null),
    currentPane: () => (opts.focused === undefined ? pane : opts.focused),
  });

  function context(overrides) {
    const over = overrides || {};
    const pos = over.pos !== undefined ? over.pos : view.state.doc.length;
    return {
      state: view.state, view, explicit: false, aborted: false, ...over, pos,
    };
  }

  return {
    sandbox, cm, windowListeners, view, pane, documentState, requests, completion, context,
    apply: sandbox.termlabLspCompletionApply,
  };
}

const item = (overrides) => ({
  id: 'item-1',
  label: 'value',
  detail: 'const value: number',
  kind: 'variable',
  documentation: [{ markdown: false, value: 'A number.' }],
  sortText: '0000value',
  filterText: null,
  insertText: null,
  isSnippet: false,
  textEdit: null,
  additionalTextEdits: [],
  commitCharacters: [],
  deprecated: false,
  unsupportedEffects: [],
  ...(overrides || {}),
});

const responseFor = (items, overrides) => ({
  documentId: 'doc-1',
  sourceVersion: 7,
  isIncomplete: false,
  items,
  ...(overrides || {}),
});

const keyEvent = (key, mods) => ({
  key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...(mods || {}),
});

// ===========================================================================
// REAL CodeMirror: keyboard ownership next to vim
// ===========================================================================

// This is the check the previous round got wrong. `Prec.highest(keymap.of(…))`
// looks like it outranks vim and does not: every keymap in a state is served
// by ONE DOM handler that @codemirror/view registers at Prec.default, while
// vim is a ViewPlugin whose own keydown handler sits at default precedence and
// runs first — so Escape left insert mode and the popup never saw it.
check('the completion keydown handler runs BEFORE vim\'s', () => {
  const h = harness({ real: true });
  const { EditorState, Prec } = REAL.state;
  // The module's own extensions, in precedence order. autocompletion()
  // contributes keydown handlers of its own at default precedence; the popup
  // key handler is the one the module raises, so it is the first of them.
  const moduleOwners = keydownOwners(
    EditorState.create({ extensions: h.completion.extensions() }),
  );
  const vimOwners = new Set(
    keydownOwners(EditorState.create({ extensions: [h.cm.CM.vim()] })),
  );
  assert.ok(moduleOwners.length >= 1 && vimOwners.size >= 1, 'both sides register handlers');
  const ourId = moduleOwners[0];
  assert.ok(!vimOwners.has(ourId), 'and they are different plugins');

  const together = keydownOwners(EditorState.create({
    extensions: [h.cm.CM.vim(), ...h.completion.extensions()],
  }));
  const ourAt = together.indexOf(ourId);
  const vimAt = together.findIndex((id) => vimOwners.has(id));
  assert.ok(ourAt >= 0 && vimAt >= 0, 'both are present');
  assert.strictEqual(ourAt, 0, 'the completion handler is the first keydown handler of all');
  assert.ok(
    ourAt < vimAt,
    `it must run before vim (completion at ${ourAt}, vim at ${vimAt}) — `
    + 'the view stops at the first handler that returns true or preventDefaults',
  );

  // And the design that does NOT work, so this test cannot pass by accident:
  // a Prec.highest keymap still loses, because it is not what orders the DOM
  // handler.
  const viaKeymap = keydownOwners(EditorState.create({
    extensions: [
      h.cm.CM.vim(),
      Prec.highest(REAL.view.keymap.of([{ key: 'Escape', run: () => true }])),
    ],
  }));
  assert.ok(
    viaKeymap.findIndex((id) => vimOwners.has(id)) === 0,
    'a Prec.highest keymap leaves vim first — which is why this module does not use one',
  );
});

check('with the popup closed the handler consumes nothing, so vim keeps every key', () => {
  const h = harness({ real: true });
  h.cm.setStatus(null);
  for (const key of ['Escape', 'Enter', 'Tab', 'ArrowDown', 'ArrowUp', 'PageDown', 'PageUp']) {
    assert.strictEqual(
      h.completion.handleKeydown(keyEvent(key), h.view),
      false,
      `${key} must fall through — returning true here is what preventDefaults it away from vim`,
    );
  }
});

check('with the popup open Escape closes it and never reaches vim', () => {
  const h = harness({ real: true });
  h.cm.setStatus('active');
  assert.strictEqual(
    h.completion.handleKeydown(keyEvent('Escape'), h.view),
    true,
    'consumed — so insert mode survives the first press, per the spec',
  );
  h.cm.setStatus(null);
  assert.strictEqual(
    h.completion.handleKeydown(keyEvent('Escape'), h.view),
    false,
    'and the second press is vim\'s',
  );
});

check('a pending popup still owns Escape', () => {
  const h = harness({ real: true });
  h.cm.setStatus('pending');
  assert.strictEqual(h.completion.handleKeydown(keyEvent('Escape'), h.view), true);
});

check('modified and unowned keys are never intercepted', () => {
  const h = harness({ real: true });
  h.cm.setStatus('active');
  const key = h.completion.eventKey;
  assert.strictEqual(key(keyEvent('Escape', { metaKey: true })), null, 'Cmd-Escape is not ours');
  assert.strictEqual(key(keyEvent('Enter', { altKey: true })), null);
  assert.strictEqual(key(keyEvent('Tab', { shiftKey: true })), null, 'Shift-Tab is outdent');
  assert.strictEqual(key(keyEvent('n', { ctrlKey: true })), null, 'vim keeps its Ctrl- bindings');
  assert.strictEqual(key(keyEvent('j')), null, 'and every normal-mode motion');
  assert.strictEqual(key(keyEvent('i')), null);
  assert.strictEqual(key(keyEvent(' ', { ctrlKey: true })), 'Ctrl-Space', 'except the manual trigger');
});

check('the arrows navigate the list only while it is open', () => {
  const moves = [];
  const h = harness({ real: true });
  h.cm.CM.moveCompletionSelection = (forward, by) => () => {
    moves.push([forward, by || 'option']);
    return true;
  };
  h.cm.setStatus(null);
  for (const key of ['ArrowDown', 'ArrowUp', 'PageDown', 'PageUp']) {
    assert.strictEqual(h.completion.handleKeydown(keyEvent(key), h.view), false);
  }
  assert.deepStrictEqual(moves, [], 'a closed popup is not asked to move');
  h.cm.setStatus('active');
  for (const key of ['ArrowDown', 'ArrowUp', 'PageDown', 'PageUp']) {
    assert.strictEqual(h.completion.handleKeydown(keyEvent(key), h.view), true);
  }
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(moves)),
    [[true, 'option'], [false, 'option'], [true, 'page'], [false, 'page']],
  );
});

check('Ctrl-Space is guarded too — a pane with no language service keeps the key', () => {
  const attached = harness({ real: true });
  assert.strictEqual(
    attached.completion.handleKeydown(keyEvent(' ', { ctrlKey: true }), attached.view),
    true,
    'an attached pane opens the popup',
  );
  const remote = harness({ real: true, remote: { hostLabel: 'box', remotePath: '/x.ts' } });
  assert.strictEqual(
    remote.completion.handleKeydown(keyEvent(' ', { ctrlKey: true }), remote.view),
    false,
    'a remote pane has no source to open, so swallowing the key would be for nothing',
  );
  const plain = harness({ real: true, documentState: null });
  assert.strictEqual(
    plain.completion.handleKeydown(keyEvent(' ', { ctrlKey: true }), plain.view),
    false,
  );
});

check('autocompletion is configured with its own keymap off', () => {
  const h = harness({ real: true });
  // Configuration is asserted through the module, not by reading CodeMirror's
  // internals: the source is the module's, and defaultKeymap must be off or
  // two handlers fight over Enter.
  const stub = harness();
  const config = stub.completion.extensions().find((e) => e && e.ext === 'autocompletion').config;
  assert.strictEqual(config.defaultKeymap, false);
  assert.strictEqual(config.activateOnTyping, true);
  assert.strictEqual(typeof config.override[0], 'function', 'the LSP source is the only source');
  assert.ok(h.completion.extensions().length >= 2, 'and the real build produces extensions too');
});

// ===========================================================================
// REAL CodeMirror: applying an item
// ===========================================================================

check('an item applied after the user kept typing does not leave the extra keystrokes', async () => {
  // The popup survives typing (validFor), and autocomplete maps the result's
  // `to` forward while the server's edit range still points at the old caret.
  // Applying the server's end verbatim produced "valuel".
  const h = harness({
    real: true,
    text: 'const value = 1;\nva',
    response: responseFor([item({
      label: 'value',
      textEdit: {
        kind: 'textEdit',
        range: { start: { line: 1, character: 0 }, end: { line: 1, character: 2 } },
        newText: 'value',
      },
    })]),
  });
  const result = await h.completion.completionSource(h.context({ pos: 19 }));
  const option = result.options[0];

  // The user types "l"; the popup stays open and CodeMirror maps `to` to 20.
  h.view.state = h.view.state.update({ changes: { from: 19, insert: 'l' } }).state;
  assert.strictEqual(text(h.view), 'const value = 1;\nval', 'the keystroke landed');

  option.apply(h.view, option, 17, 20);
  assert.strictEqual(
    text(h.view),
    'const value = 1;\nvalue',
    'the completion replaced everything the user had typed, not the request-time range',
  );
  assert.strictEqual(h.view.dispatches, 1, 'in one transaction');
});

check('a plain apply with no drift replaces exactly the server\'s range', async () => {
  const h = harness({
    real: true,
    text: 'const value = 1;\nva',
    response: responseFor([item({
      textEdit: {
        kind: 'textEdit',
        range: { start: { line: 1, character: 0 }, end: { line: 1, character: 2 } },
        newText: 'value',
      },
    })]),
  });
  const option = (await h.completion.completionSource(h.context({ pos: 19 }))).options[0];
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(text(h.view), 'const value = 1;\nvalue');
  assert.strictEqual(
    h.view.state.selection.main.head, 22,
    'and the caret ends up after the inserted text',
  );
});

check('an InsertReplaceEdit uses its insert range and leaves the tail alone', async () => {
  const h = harness({
    real: true,
    text: 'const value = 1;\nvalue',
    response: responseFor([item({
      textEdit: {
        kind: 'insertReplaceEdit',
        insert: { start: { line: 1, character: 0 }, end: { line: 1, character: 2 } },
        replace: { start: { line: 1, character: 0 }, end: { line: 1, character: 5 } },
        newText: 'valueOf',
      },
    })]),
  });
  const option = (await h.completion.completionSource(h.context({ pos: 19 }))).options[0];
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(
    text(h.view),
    'const value = 1;\nvalueOflue',
    'insert, not replace — this phase never eats text to the right of the caret',
  );
});

check('an item with no text edit falls back to insertText, then to the label', async () => {
  const h = harness({
    real: true,
    response: responseFor([
      item({ id: 'a', label: 'value', insertText: 'value()' }),
      item({ id: 'b', label: 'other', insertText: null }),
    ]),
  });
  const options = (await h.completion.completionSource(h.context())).options;
  options[0].apply(h.view, options[0], 17, 19);
  assert.strictEqual(text(h.view), 'const value = 1;\nvalue()');
  const second = harness({ real: true, response: responseFor([item({ label: 'other' })]) });
  const only = (await second.completion.completionSource(second.context())).options[0];
  only.apply(second.view, only, 17, 19);
  assert.strictEqual(text(second.view), 'const value = 1;\nother');
});

check('a snippet expands through CodeMirror\'s applier and selects its first field', async () => {
  const h = harness({
    real: true,
    response: responseFor([item({
      label: 'log',
      insertText: 'console.log(${1:value})',
      isSnippet: true,
    })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(text(h.view), 'const value = 1;\nconsole.log(value)');
  assert.strictEqual(selected(h.view), 'value', 'the placeholder is selected, not the raw template');
  assert.strictEqual(h.view.dispatches, 1);
});

check('a snippet with an additional edit keeps its placeholder on the right text', async () => {
  // The bug this pins: an insertion ABOVE the snippet shifts everything below
  // it, and a selection recorded against "the snippet alone" then lands on
  // "og(va".
  const h = harness({
    real: true,
    text: 'import {} from "./m";\nconst value = 1;\nlo',
    response: responseFor([item({
      label: 'log',
      insertText: 'console.log(${1:value})',
      isSnippet: true,
      additionalTextEdits: [{
        range: { start: { line: 0, character: 8 }, end: { line: 0, character: 8 } },
        newText: 'log',
      }],
    })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  option.apply(h.view, option, 39, 41);
  assert.strictEqual(
    text(h.view),
    'import {log} from "./m";\nconst value = 1;\nconsole.log(value)',
    'both edits landed',
  );
  assert.strictEqual(h.view.dispatches, 1, 'in ONE transaction — undo restores both together');
  assert.strictEqual(
    selected(h.view),
    'value',
    'and the placeholder is on the field, not shifted by the import insertion',
  );
});

check('a multi-field snippet keeps its later fields reachable across an added import', async () => {
  const h = harness({
    real: true,
    text: 'import {} from "./m";\nconst value = 1;\nlo',
    response: responseFor([item({
      label: 'log',
      insertText: 'log(${1:first}, ${2:second})',
      isSnippet: true,
      additionalTextEdits: [{
        range: { start: { line: 0, character: 8 }, end: { line: 0, character: 8 } },
        newText: 'log',
      }],
    })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  option.apply(h.view, option, 39, 41);
  assert.strictEqual(
    text(h.view),
    'import {log} from "./m";\nconst value = 1;\nlog(first, second)',
  );
  assert.strictEqual(selected(h.view), 'first');
  // A two-field snippet installs placeholder state through an effect; if the
  // effect were dropped or left unmapped, Tab could not reach "second".
  const snippetText = text(h.view);
  assert.ok(
    snippetText.indexOf('second') > snippetText.indexOf('first'),
    'both fields are present and ordered',
  );
});

check('same-document additional edits ride in the same transaction', async () => {
  const h = harness({
    real: true,
    text: 'import {} from "./m";\nconst value = 1;\nva',
    response: responseFor([item({
      textEdit: {
        kind: 'textEdit',
        range: { start: { line: 2, character: 0 }, end: { line: 2, character: 2 } },
        newText: 'value',
      },
      additionalTextEdits: [{
        range: { start: { line: 0, character: 8 }, end: { line: 0, character: 8 } },
        newText: 'value',
      }],
    })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  option.apply(h.view, option, 39, 41);
  assert.strictEqual(
    text(h.view),
    'import {value} from "./m";\nconst value = 1;\nvalue',
  );
  assert.strictEqual(
    h.view.dispatches, 1,
    'undo, dirty tracking and LSP synchronisation must see ONE change',
  );
  assert.strictEqual(selected(h.view), '', 'the caret is a cursor');
  assert.strictEqual(
    h.view.state.selection.main.head, 49,
    'placed after the completion, corrected for the import inserted above it',
  );
});

check('overlapping additional edits are refused rather than half-applied', async () => {
  const h = harness({
    real: true,
    text: 'const value = 1;\nva',
    response: responseFor([item({
      textEdit: {
        kind: 'textEdit',
        range: { start: { line: 1, character: 0 }, end: { line: 1, character: 2 } },
        newText: 'value',
      },
      additionalTextEdits: [{
        range: { start: { line: 1, character: 1 }, end: { line: 1, character: 2 } },
        newText: 'X',
      }],
    })]),
  });
  const option = (await h.completion.completionSource(h.context({ pos: 19 }))).options[0];
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(
    text(h.view), 'const value = 1;\nvalue',
    'the primary edit still lands; the conflicting extra one does not',
  );
  assert.ok(
    h.completion.sessionLog().some((entry) => /overlap/i.test(entry.message)),
    'and the refusal is recorded',
  );
});

check('a snippet applier that cannot be captured degrades to plain TEXT, not a raw template', async () => {
  const h = harness({
    real: true,
    text: 'const value = 1;\nlo',
    response: responseFor([item({
      label: 'log',
      insertText: 'console.log(${1:value}, $2)',
      isSnippet: true,
      additionalTextEdits: [{
        range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } },
        newText: '// ',
      }],
    })]),
  });
  // A CodeMirror whose snippet applier throws: the expansion is impossible, so
  // the item must still land as readable text in one transaction.
  h.cm.CM.snippet = () => { throw new Error('no snippet support'); };
  const option = (await h.completion.completionSource(h.context({ pos: 19 }))).options[0];
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(
    text(h.view),
    '// const value = 1;\nconsole.log(value, )',
    'placeholders resolved to their defaults — never "${1:value}" in the buffer',
  );
  assert.strictEqual(h.view.dispatches, 1);
  assert.ok(
    h.completion.sessionLog().some((entry) => /plain text/.test(entry.message)),
    'and the degradation is recorded rather than silent',
  );
});

check('snippetPlainText resolves placeholders and respects escapes', () => {
  const h = harness({ real: true });
  const plain = h.apply.snippetPlainText;
  assert.strictEqual(plain('console.log(${1:value})'), 'console.log(value)');
  assert.strictEqual(plain('fn($1, ${2})'), 'fn(, )');
  assert.strictEqual(plain('${1:a}${2:b}'), 'ab');
  assert.strictEqual(plain('cost: \\$5'), 'cost: $5', 'an escaped dollar is money, not a tab stop');
  assert.strictEqual(plain('plain text'), 'plain text');
  // LSP escapes `$`, `}` and `\` inside snippet text, including inside a
  // placeholder's default.
  assert.strictEqual(plain('a\\{b\\}c'), 'a{b}c');
  assert.strictEqual(
    plain('\\\\${1:a}'), '\\a',
    'an escaped BACKSLASH does not escape the placeholder that follows it',
  );
  assert.strictEqual(
    plain('${1:\\$2}'), '$2',
    'and a default\'s own escapes are resolved rather than left in the buffer',
  );
});

// One left-to-right pass, not four. A multi-pass form re-scans text it has
// already produced: "$${1}1" is a literal dollar, an empty tab stop and a
// literal "1", but stripping the tab stop first leaves "$1", which the next
// pass then eats as a tab stop that was never in the template.
check('snippetPlainText never re-reads its own output', () => {
  const h = harness({ real: true });
  assert.strictEqual(h.apply.snippetPlainText('$${1}1'), '$1');
});

// Lookbehind assertions reached JavaScriptCore only in Safari 16.4 (macOS
// 13.3); this app declares no minimumSystemVersion, so Tauri's floor applies.
// A regex literal is validated when the FILE is parsed, so one lookbehind
// would stop these modules loading at all on an older WKWebView — and because
// every call site guards on the module being present, the failure would be
// silent: the popup opens and accepting an item does nothing.
check('the completion modules use no regex lookbehind', () => {
  for (const file of [COMPLETION, APPLY]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(
      !/\(\?<[=!]/.test(source),
      `${file} uses a lookbehind — it costs the whole file on an older WKWebView`,
    );
  }
});

check('a cross-document workspace edit is visible and refused, not partially applied', async () => {
  const h = harness({
    real: true,
    response: responseFor([item({ label: 'value', unsupportedEffects: ['workspaceEdit'] })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  const rendered = JSON.stringify(await option.info(option));
  assert.ok(
    /other files|another file|workspace/i.test(rendered),
    'the item says its workspace edit is not applied',
  );
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(text(h.view), 'const value = 1;\nvalue', 'only this document changed');
  assert.strictEqual(h.view.dispatches, 1);
  assert.ok(
    h.completion.sessionLog().some((entry) => /workspace/i.test(entry.message)),
    'a session-log entry records the refusal',
  );
});

check('an unsupported command is surfaced the same way', async () => {
  const h = harness({ response: responseFor([item({ unsupportedEffects: ['command'] })]) });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.ok(/command/i.test(JSON.stringify(await option.info(option))));
});

check('the session log is bounded', async () => {
  const h = harness({
    real: true,
    response: responseFor([item({ unsupportedEffects: ['workspaceEdit'] })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  for (let i = 0; i < 400; i += 1) option.apply(h.view, option, 17, 19);
  const logged = h.completion.sessionLog();
  assert.ok(logged.length > 0 && logged.length <= 200, `bounded, got ${logged.length}`);
});

// ===========================================================================
// The source: when a request happens, and what comes back
// ===========================================================================

check('typing an identifier character requests completion', async () => {
  const h = harness({ response: responseFor([item()]) });
  const result = await h.completion.completionSource(h.context());
  assert.strictEqual(h.requests.length, 1);
  assert.strictEqual(h.requests[0].kind, 'completion');
  assert.strictEqual(h.requests[0].target, h.pane);
  assert.strictEqual(h.requests[0].trigger, null, 'ordinary typing carries no trigger character');
  assert.strictEqual(result.options.length, 1);
});

check('the position sent is the LSP position of the caret, not a CodeMirror offset', async () => {
  const h = harness({ response: responseFor([item()]) });
  await h.completion.completionSource(h.context());
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.requests[0].position)),
    { line: 1, character: 2 },
  );
});

check('a server trigger character requests completion and names itself', async () => {
  const h = harness({
    text: 'value.', triggerCharacters: ['.', '"'], response: responseFor([item()]),
  });
  await h.completion.completionSource(h.context());
  assert.strictEqual(h.requests[0].trigger, '.');
});

check('a character that is neither an identifier nor a trigger makes no request', async () => {
  const h = harness({ text: 'value + ', triggerCharacters: ['.'] });
  assert.strictEqual(await h.completion.completionSource(h.context()), null);
  assert.deepStrictEqual(h.requests, [], 'a space must not wake the language server');
});

check('an explicit invocation requests even with nothing typed', async () => {
  const h = harness({ text: 'value + ', response: responseFor([item()]) });
  assert.ok(await h.completion.completionSource(h.context({ explicit: true })));
  assert.strictEqual(h.requests.length, 1);
});

check('suggestions-while-typing off leaves manual completion working', async () => {
  const h = harness({ response: responseFor([item()]) });
  h.completion.setSuggestionsWhileTyping(false);
  assert.strictEqual(await h.completion.completionSource(h.context()), null);
  assert.deepStrictEqual(h.requests, [], 'nothing automatic');
  assert.ok(await h.completion.completionSource(h.context({ explicit: true })), 'Ctrl-Space works');
  assert.strictEqual(h.requests.length, 1);
  h.completion.setSuggestionsWhileTyping(true);
  assert.ok(await h.completion.completionSource(h.context()), 'and it can be turned back on');
});

check('a remote pane never reaches the language service', async () => {
  const h = harness({ remote: { paneId: 4, remotePath: '/srv/main.ts', hostLabel: 'box' } });
  assert.strictEqual(await h.completion.completionSource(h.context({ explicit: true })), null);
  assert.deepStrictEqual(h.requests, []);
});

check('a pane with no attached document makes no request', async () => {
  const h = harness({ documentState: null });
  assert.strictEqual(await h.completion.completionSource(h.context({ explicit: true })), null);
  assert.deepStrictEqual(h.requests, []);
});

check('a session that cannot complete makes no request', async () => {
  const h = harness({
    status: {
      ...DEFAULT_STATUS,
      state: 'starting',
      capabilities: { ...DEFAULT_STATUS.capabilities, completion: false },
    },
  });
  assert.strictEqual(await h.completion.completionSource(h.context({ explicit: true })), null);
  assert.deepStrictEqual(h.requests, [], 'plain editing continues while a server starts');
});

check('an aborted context discards the response', async () => {
  const box = {};
  const h = harness({ respond: () => { box.ctx.aborted = true; return responseFor([item()]); } });
  box.ctx = h.context({ explicit: true });
  assert.strictEqual(await h.completion.completionSource(box.ctx), null);
});

check('a response for another document is discarded', async () => {
  const h = harness({ response: responseFor([item()], { documentId: 'doc-2' }) });
  assert.strictEqual(await h.completion.completionSource(h.context({ explicit: true })), null);
});

check('a response arriving after the pane detached is discarded', async () => {
  const h = harness({
    respond: () => {
      h.sandbox.termlabLspState.get = () => null;
      return responseFor([item()]);
    },
  });
  assert.strictEqual(await h.completion.completionSource(h.context({ explicit: true })), null);
});

// The version guard belongs to editor-service's barrier, which captures it
// AFTER flushing the 40 ms change batch. Re-checking a version captured before
// the flush threw away good results whenever a keystroke was still batched —
// which, with a 100 ms activateOnTypingDelay, is most of them.
check('a batched keystroke flushed by the request does NOT discard the result', async () => {
  const h = harness({
    respond: () => {
      // What withFixedBarrier does: drains the pending batch, so the document
      // version moves on between the source capturing it and the reply.
      h.documentState.version = 9;
      return responseFor([item()], { sourceVersion: 9 });
    },
  });
  const result = await h.completion.completionSource(h.context({ explicit: true }));
  assert.ok(result, 'the completion still opens');
  assert.strictEqual(result.options.length, 1);
});

check('a null response (the barrier already rejected it) yields no popup', async () => {
  const h = harness({ response: null });
  assert.strictEqual(await h.completion.completionSource(h.context({ explicit: true })), null);
});

// ===========================================================================
// Option translation
// ===========================================================================

check('the completion range starts at the word being typed', async () => {
  const h = harness({ response: responseFor([item()]) });
  assert.strictEqual((await h.completion.completionSource(h.context())).from, 17);
});

check('filter text drives matching while the label drives display', async () => {
  const h = harness({
    response: responseFor([item({ label: 'value (property)', filterText: 'value' })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.strictEqual(option.label, 'value');
  assert.strictEqual(option.displayLabel, 'value (property)');
  const plain = harness({ response: responseFor([item({ label: 'value' })]) });
  const bare = (await plain.completion.completionSource(plain.context())).options[0];
  assert.strictEqual(bare.displayLabel, undefined, 'no pointless duplicate');
});

check('detail and documentation are carried across, as text and never as markup', async () => {
  const h = harness({
    response: responseFor([item({
      documentation: [
        { markdown: false, value: 'A number.' },
        { markdown: true, value: '`value` is **not** html <img onerror=x>' },
      ],
    })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.strictEqual(option.detail, 'const value: number');
  const rendered = JSON.stringify(await option.info(option));
  assert.ok(/A number\./.test(rendered));
  assert.ok(/onerror=x/.test(rendered), 'markdown source is shown as text, not interpreted');
  for (const file of [COMPLETION, APPLY]) {
    assert.ok(!/innerHTML/.test(fs.readFileSync(file, 'utf8')), `${file} never uses innerHTML`);
  }
});

check('item kinds map to CodeMirror completion types', async () => {
  const kinds = [
    ['function', 'function'], ['method', 'method'], ['variable', 'variable'],
    ['class', 'class'], ['interface', 'interface'], ['keyword', 'keyword'],
    ['field', 'property'], ['module', 'namespace'], ['enumMember', 'enum'],
    ['constant', 'constant'], ['struct', 'class'], ['typeParameter', 'type'],
    ['file', 'text'], [null, 'text'], ['somethingNew', 'text'],
  ];
  const h = harness({
    response: responseFor(kinds.map(([kind], i) => item({ id: `k${i}`, label: `l${i}`, kind }))),
  });
  const options = (await h.completion.completionSource(h.context())).options;
  assert.deepStrictEqual(options.map((o) => o.type), kinds.map(([, type]) => type));
});

check('commit characters are carried to CodeMirror', async () => {
  const h = harness({ response: responseFor([item({ commitCharacters: ['.', '('] })]) });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.deepStrictEqual(JSON.parse(JSON.stringify(option.commitCharacters)), ['.', '(']);
  const plain = harness({ response: responseFor([item()]) });
  const bare = (await plain.completion.completionSource(plain.context())).options[0];
  assert.strictEqual(bare.commitCharacters, undefined, 'no empty array noise');
});

check('items are ranked by sortText, which is the client\'s job and not Rust\'s', async () => {
  const h = harness({
    response: responseFor([
      item({ id: 'a', label: 'zeta', sortText: '0000' }),
      item({ id: 'b', label: 'alpha', sortText: '0001' }),
      item({ id: 'c', label: 'aaa', sortText: null }),
    ]),
  });
  const options = (await h.completion.completionSource(h.context())).options;
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(options.map((o) => o.label))),
    ['zeta', 'alpha', 'aaa'],
  );
  assert.ok(options[0].boost > options[1].boost && options[0].boost <= 99);
});

check('ties fall back to the order the server sent', async () => {
  const h = harness({
    response: responseFor([
      item({ id: 'a', label: 'second', sortText: '0001' }),
      item({ id: 'b', label: 'first', sortText: '0000' }),
      item({ id: 'c', label: 'also second', sortText: '0001' }),
    ]),
  });
  const options = (await h.completion.completionSource(h.context())).options;
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(options.map((o) => o.label))),
    ['first', 'second', 'also second'],
  );
});

check('a long list stays inside CodeMirror\'s boost range', async () => {
  const many = [];
  for (let i = 0; i < 400; i += 1) {
    many.push(item({ id: `i${i}`, label: `name${String(i).padStart(4, '0')}`, sortText: null }));
  }
  const h = harness({ response: responseFor(many) });
  for (const option of (await h.completion.completionSource(h.context())).options) {
    assert.ok(option.boost >= -99 && option.boost <= 99, `boost ${option.boost} out of range`);
  }
});

// ===========================================================================
// completionItem/resolve, through the real bridge method
// ===========================================================================

check('lsp-bridge exposes resolveCompletionItem over the real command', async () => {
  const calls = [];
  const { sandbox } = load([BRIDGE], {
    termlabServices: {
      tauriClient: {
        invoke: (command, args) => { calls.push([command, args]); return Promise.resolve(null); },
        listenOnCurrentWindow: () => Promise.resolve(() => {}),
      },
    },
  });
  assert.strictEqual(
    typeof sandbox.termlabLspBridge.resolveCompletionItem, 'function',
    'the module resolves through the bridge, so the bridge has to offer it',
  );
  await sandbox.termlabLspBridge.resolveCompletionItem('doc-1', 'doc-1:7:2:0');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(calls)),
    [['lsp_resolve_completion_item', { documentId: 'doc-1', itemId: 'doc-1:7:2:0' }]],
  );
  // And the Rust side really declares it, with those argument names.
  const commands = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/lsp/commands.rs'), 'utf8',
  );
  assert.ok(/lsp_resolve_completion_item/.test(commands), 'the command exists in Rust');
  assert.ok(
    /name: "lsp_resolve_completion_item",\s*args: &\["documentId", "itemId"\]/.test(commands),
    'and its contract names the same arguments the bridge sends',
  );
});

check('completion-item resolve enriches documentation and is asked once', async () => {
  const resolved = [];
  const h = harness({
    bridge: {
      resolveCompletionItem: async (documentId, id) => {
        resolved.push([documentId, id]);
        return item({ id, documentation: [{ markdown: false, value: 'Resolved docs.' }] });
      },
    },
    response: responseFor([item({ id: 'item-9', documentation: [] })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  const rendered = JSON.stringify(await option.info(option));
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(resolved)), [['doc-1', 'item-9']],
    'resolve is addressed to the owning document and the shown item',
  );
  assert.ok(/Resolved docs\./.test(rendered));
  await option.info(option);
  assert.strictEqual(resolved.length, 1, 'and asked once — the result is cached');
});

check('a failing resolve still renders what the item already had', async () => {
  const h = harness({
    bridge: { resolveCompletionItem: async () => { throw new Error('server gone'); } },
    response: responseFor([item({ documentation: [{ markdown: false, value: 'Local docs.' }] })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.ok(/Local docs\./.test(JSON.stringify(await option.info(option))));
});

check('no resolve method on the bridge is simply no resolve', async () => {
  const h = harness({ response: responseFor([item()]) });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.ok(await option.info(option));
});

// ===========================================================================
// Wiring and contracts
// ===========================================================================

check('vendor-entry exports exactly the completion API the modules need', () => {
  const source = fs.readFileSync(VENDOR_ENTRY, 'utf8');
  for (const name of [
    'autocompletion', 'startCompletion', 'closeCompletion', 'acceptCompletion',
    'moveCompletionSelection', 'completionStatus', 'insertCompletionText', 'snippet',
    'Prec', 'ChangeSet', 'StateEffect',
  ]) {
    assert.ok(new RegExp(`\\b${name}\\b`).test(source), `vendor-entry.mjs must re-export ${name}`);
  }
  assert.ok(
    !/\bsnippetCompletion\b/.test(source),
    'snippetCompletion has no consumer — the apply module drives snippet() itself',
  );
  // The lint exports arrived with diagnostics (see test_lsp_diagnostics.mjs).
  // What is asserted here is that they stayed narrow: CodeMirror's own lint
  // PANEL and keymap would be a second, competing problems list next to the
  // Problems tool window. Read off the export lists rather than the whole
  // file — the same way check-vendor.mjs does — so naming one of these in a
  // comment that explains why it is absent is not itself a failure.
  const exported = new Set(
    [...source.matchAll(/export\s*\{([^}]*)\}/g)]
      .flatMap((match) => match[1].split(','))
      .map((name) => name.trim().split(/\s+as\s+/).pop().trim())
      .filter(Boolean),
  );
  for (const name of [
    'lintKeymap', 'openLintPanel', 'closeLintPanel', 'nextDiagnostic', 'previousDiagnostic',
  ]) {
    assert.ok(!exported.has(name), `${name} has no consumer — Problems owns diagnostic navigation`);
  }
  // Every name the modules read off CM6 has to be in the built bundle.
  const bundle = path.join(ROOT, 'vendor/codemirror/codemirror.js');
  if (fs.existsSync(bundle)) {
    const sandbox = { console };
    sandbox.window = sandbox;
    vm.createContext(sandbox);
    vm.runInContext(fs.readFileSync(bundle, 'utf8'), sandbox, { filename: bundle });
    const used = new Set();
    for (const file of [COMPLETION, APPLY]) {
      for (const match of fs.readFileSync(file, 'utf8').matchAll(/\bCM\.([A-Za-z]+)/g)) {
        used.add(match[1]);
      }
    }
    for (const name of used) {
      assert.ok(sandbox.CM6[name] !== undefined, `the built bundle is missing CM6.${name}`);
    }
  }
});

check('package.json declares autocomplete and lint rather than inheriting them', () => {
  const pkg = JSON.parse(fs.readFileSync(PACKAGE_JSON, 'utf8'));
  const declared = { ...(pkg.dependencies || {}), ...(pkg.devDependencies || {}) };
  assert.ok(declared['@codemirror/autocomplete']);
  assert.ok(declared['@codemirror/lint'], '@codemirror/lint is declared for Task 12');
  const lock = JSON.parse(fs.readFileSync(path.join(ROOT, 'package-lock.json'), 'utf8'));
  const root = (lock.packages && lock.packages['']) || {};
  const locked = { ...(root.dependencies || {}), ...(root.devDependencies || {}) };
  assert.ok(locked['@codemirror/autocomplete'] && locked['@codemirror/lint']);
});

check('index.html loads both completion modules in dependency order', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (file) => html.indexOf(`app/features/editor/${file}`);
  assert.ok(at('lsp-completion-apply.js') > 0, 'the apply module is loaded');
  assert.ok(at('lsp-completion.js') > at('lsp-completion-apply.js'), 'and loaded first');
  assert.ok(at('lsp-completion.js') > at('lsp-state.js'));
  assert.ok(at('lsp-completion.js') > at('lsp-bridge.js'));
  assert.ok(at('lsp-completion.js') < at('editor-pane.js'), 'before the pane that mounts it');
});

check('manager-compose-runtime configures the completion module', () => {
  const source = fs.readFileSync(COMPOSE, 'utf8');
  assert.ok(/termlabLspCompletion/.test(source));
  assert.ok(/paneForView/.test(source), 'it supplies the view -> pane lookup it alone can build');
});

check('the suggestions-while-typing setting is carried from Rust and hot-reloaded', () => {
  const rust = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/commands.rs'), 'utf8',
  );
  assert.ok(
    /"editor_lsp_suggestions_while_typing":\s*cfg\.editor\.lsp\.suggestions_while_typing/.test(rust),
    'get_app_config carries the flag, like editor_vim_mode next to it',
  );
  for (const file of ['startup-runtime.js', 'config-runtime.js']) {
    const source = fs.readFileSync(path.join(APP, file), 'utf8');
    assert.ok(
      /editor_lsp_suggestions_while_typing/.test(source)
      && /setSuggestionsWhileTyping/.test(source),
      `${file} applies it`,
    );
  }
});

check('editor-pane mounts the completion extensions without displacing vim', () => {
  const { sandbox } = load([VIM_MODE, POSITION, APPLY, COMPLETION, EDITOR_PANE]);
  const host = sandbox.document.createElement('div');
  const view = sandbox.termlabEditorPane.createEditorView(host, { doc: 'x', vimMode: true });
  const extensions = view.state.spec.extensions;
  const vimAt = extensions.findIndex(
    (e) => e && Array.isArray(e.contents) && e.contents.some((c) => c && c.ext === 'vim'),
  );
  assert.strictEqual(vimAt, 0, 'vim keeps the first slot — test_editor_vim_glue.mjs pins it');
  assert.ok(extensions.some((e) => e && e.ext === 'autocompletion'));
  const raised = extensions.find((e) => e && e.ext === 'prec' && e.level === 'highest');
  assert.ok(raised, 'the key handler is mounted at raised precedence');
  assert.strictEqual(
    raised.contents.ext, 'domEventHandlers',
    'and it is a DOM handler, not a keymap — a keymap cannot outrank vim',
  );
});

check('editor-pane no longer claims keymaps resolve in extension order', () => {
  const source = fs.readFileSync(EDITOR_PANE, 'utf8');
  assert.ok(
    !/CodeMirror resolves keymaps in\s*\n?\s*\/\/\s*extension order/.test(source),
    'that model is wrong and is what the Prec.highest keymap design was built on',
  );
  assert.ok(/Prec\.default/.test(source), 'the comment names where the shared keymap handler sits');
});

check('the modules register no window or document key handlers', () => {
  for (const file of [COMPLETION, APPLY]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(
      !/addEventListener\(\s*['"]key(down|up|press)['"]/.test(source),
      `${file}: keyboard behaviour belongs in CodeMirror, never a global handler`,
    );
    assert.ok(!/\bdocument\.addEventListener\b/.test(source), `${file}: no document listeners`);
  }
  const { windowListeners } = harness();
  for (const type of windowListeners.keys()) {
    assert.ok(!/^key/.test(type), `configure() registered a "${type}" listener`);
  }
});

check('the info panel classes it renders are styled', () => {
  const css = fs.readFileSync(
    path.join(ROOT, 'styles/design-system/components/editor.css'), 'utf8',
  );
  for (const name of [
    'tl-completion-info', 'tl-completion-info-detail',
    'tl-completion-info-doc', 'tl-completion-info-note',
  ]) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  const block = css.slice(css.indexOf('.tl-completion-info'));
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(block), 'tokens only, no hex colours');
});

check('the configured editor_completion shortcut reaches the focused editor pane', () => {
  const h = harness();
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-completion'));
  assert.deepStrictEqual(h.cm.calls.map((c) => c[0]), ['startCompletion']);
  assert.strictEqual(h.cm.calls[0][1], h.view);
});

check('the shortcut is inert on a pane with no language service', () => {
  for (const focused of [
    { paneId: 9, tabId: 1, kind: 'terminal', term: {} },
    null,
  ]) {
    const h = harness({ focused });
    h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-completion'));
    assert.deepStrictEqual(h.cm.calls, []);
  }
  const plain = harness({ documentState: null });
  plain.sandbox.dispatchEvent(new plain.sandbox.CustomEvent('termlab:editor-completion'));
  assert.deepStrictEqual(plain.cm.calls, [], 'a plain-text editor pane opens nothing');
});

check('dispose removes the shortcut listener', () => {
  const h = harness();
  h.completion.dispose();
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-completion'));
  assert.deepStrictEqual(h.cm.calls, []);
});

for (const { name, fn } of queued) {
  ran += 1;
  try {
    await fn();
    console.log(`  ok   ${name}`);
  } catch (error) {
    failures += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${(error && error.stack) || error}`);
  }
}
if (failures) {
  console.log(`lsp completion: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`lsp completion: all ${ran} checks passed`);
}
