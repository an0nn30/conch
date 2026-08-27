// Run: node scripts/tests/test_lsp_completion.mjs
//
// LSP-backed CodeMirror completion, and the part of it that is easy to get
// wrong: living next to vim without stealing its keys.
//
// CM6 is stubbed (the real bundle needs a DOM and there is no jsdom here); the
// module under test — features/editor/lsp-completion.js — is the real file, as
// are editor-pane.js and the wiring files this scans.
//
// What this pins:
//   - the vendor bundle really exports the completion API the module reaches
//     for, and @codemirror/autocomplete + @codemirror/lint are declared rather
//     than inherited transitively;
//   - a request is made only for a local, attached, completion-capable pane;
//   - responses for a different document or an older version are dropped;
//   - one CodeMirror transaction carries the primary edit, the snippet and the
//     supported same-document additional edits;
//   - cross-document workspace edits are refused visibly and logged, bounded;
//   - the completion keymap outranks vim by PRECEDENCE, not by displacing vim
//     from the first extension slot, and every binding falls through unless the
//     popup is actually open.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const COMPLETION = path.join(APP, 'features/editor/lsp-completion.js');
const EDITOR_PANE = path.join(APP, 'features/editor/editor-pane.js');
const VIM_MODE = path.join(APP, 'features/editor/vim-mode.js');
const VENDOR_ENTRY = path.join(ROOT, 'vendor-entry.mjs');
const PACKAGE_JSON = path.join(ROOT, 'package.json');
const INDEX_HTML = path.join(ROOT, 'index.html');
const COMPOSE = path.join(APP, 'manager-compose-runtime.js');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

// --- fake DOM --------------------------------------------------------------
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
      // Deliberately absent: innerHTML. A completion info panel that reaches
      // for it is rendering server text as markup.
    };
  }
  return { createElement: (tag) => element(tag) };
}

// --- CM6 stand-in ----------------------------------------------------------
//
// Mirrors the shapes @codemirror/autocomplete and @codemirror/state document.
// Everything the module calls is recorded so the test can assert on calls
// rather than on rendered pixels.
function makeCM6(sandbox) {
  const calls = [];
  let status = null;
  const CM = {
    Prec: {
      highest: (ext) => ({ ext: 'prec', level: 'highest', contents: ext }),
    },
    keymap: { of: (bindings) => ({ ext: 'keymap', bindings }) },
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
    // Faithful to the real applier in one way that matters: it takes a
    // duck-typed {state, dispatch} editor and dispatches
    // `editor.state.update(spec)` — a Transaction, not a spec. Merging the
    // additional edits into it therefore depends on intercepting `update`.
    snippet: (template) => (editor, completion, from, to) => {
      calls.push(['snippet', template, from, to]);
      editor.dispatch(editor.state.update({
        changes: { from, to, insert: String(template).replace(/\$\{\d+:([^}]*)\}/g, '$1').replace(/\$\d+/g, '') },
        selection: { anchor: from + 1 },
        effects: [{ ext: 'snippetField' }],
        annotations: ['pickedCompletion'],
      }));
    },
    snippetCompletion: (template, completion) => ({ ...completion, template }),
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
  return {
    CM,
    calls,
    setStatus: (value) => { status = value; },
  };
}

// --- document stand-in -----------------------------------------------------
//
// Enough of CodeMirror's Text for the two conversions the module owns:
// LSP {line, character} -> offset, and offset -> {line, character}.
function makeDoc(text) {
  const lines = String(text).split('\n');
  const starts = [];
  let at = 0;
  for (const line of lines) {
    starts.push(at);
    at += line.length + 1;
  }
  return {
    text: String(text),
    lines: lines.length,
    length: String(text).length,
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
      for (let i = 0; i < lines.length; i += 1) {
        if (offset >= starts[i]) index = i;
      }
      return this.line(index + 1);
    },
    sliceString(from, to) { return String(text).slice(from, to); },
    toString() { return String(text); },
  };
}

function makeView(text) {
  const dispatched = [];
  const view = {
    state: {
      doc: makeDoc(text),
      // A Transaction, the way EditorState.update really builds one: it has a
      // resulting `.state`, which is how a spec can be told apart from it.
      update: (spec) => ({ state: {}, spec }),
    },
    dispatch: (value) => { dispatched.push(value); },
    focus() {},
    dispatched,
  };
  return view;
}

// --- harness ---------------------------------------------------------------
function load(files, extra) {
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    Promise,
    Map,
    Set,
    WeakMap,
    Array,
    Object,
    JSON,
    Date,
    RegExp,
    String,
    Number,
    Boolean,
    Math,
    Error,
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
  const cm = makeCM6(sandbox);
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

// A pane, its lsp-state record and a stubbed request path, all wired the way
// manager-compose-runtime wires them in the app.
function harness(options = {}) {
  const { sandbox, cm, windowListeners } = load([COMPLETION]);
  const view = makeView(options.text === undefined ? 'const value = 1;\nva' : options.text);
  const pane = {
    paneId: 4,
    tabId: 2,
    kind: 'editor',
    view,
    filePath: '/repo/main.ts',
    remote: options.remote || null,
  };
  const status = options.status === undefined
    ? { ...DEFAULT_STATUS, completionTriggerCharacters: options.triggerCharacters }
    : options.status;
  const documentState = options.documentState === undefined
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
    : options.documentState;

  sandbox.termlabLspState = {
    get: (candidate) => (candidate === pane ? documentState : null),
  };
  const requests = [];
  sandbox.termlabLspBridge = options.bridge || {};
  sandbox.termlabEditorService = {
    requestFeature: async (target, kind, position, trigger) => {
      requests.push({ target, kind, position, trigger });
      if (typeof options.respond === 'function') return options.respond({ position, trigger });
      return options.response === undefined ? null : options.response;
    },
  };

  const completion = sandbox.termlabLspCompletion;
  completion.configure({
    paneForView: (candidate) => (candidate === view ? pane : null),
    currentPane: () => (options.focused === undefined ? pane : options.focused),
  });

  function context(overrides) {
    const pos = overrides && overrides.pos !== undefined ? overrides.pos : view.state.doc.length;
    return {
      state: view.state,
      view,
      pos,
      explicit: !!(overrides && overrides.explicit),
      aborted: !!(overrides && overrides.aborted),
      matchBefore() { return null; },
      ...(overrides || {}),
      ...(overrides && overrides.pos !== undefined ? { pos: overrides.pos } : { pos }),
    };
  }

  return {
    sandbox, cm, windowListeners, view, pane, documentState, requests, completion, context,
  };
}

function bindingFor(bindings, key) {
  return bindings.find((binding) => binding.key === key) || null;
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

// --- vendor contract -------------------------------------------------------
check('vendor-entry exports exactly the completion API the module needs', () => {
  const source = fs.readFileSync(VENDOR_ENTRY, 'utf8');
  for (const name of [
    'autocompletion', 'startCompletion', 'closeCompletion', 'acceptCompletion',
    'moveCompletionSelection', 'completionStatus', 'snippet', 'snippetCompletion',
  ]) {
    assert.ok(
      new RegExp(`\\b${name}\\b`).test(source),
      `vendor-entry.mjs must re-export ${name} — the bundle is the app's whole CodeMirror surface`,
    );
  }
  assert.ok(
    /@codemirror\/autocomplete/.test(source),
    'the completion exports come from @codemirror/autocomplete',
  );
  // Task 12 owns the lint exports. Declaring the package early is the point;
  // exporting from it early is not.
  assert.ok(
    !/export[^;]*from\s*'@codemirror\/lint'/.test(source),
    'no lint exports yet — Task 12 adds them',
  );
  // The completion keymap has to outrank vim without displacing vim from the
  // first extension slot, which takes Prec.
  assert.ok(/\bPrec\b/.test(source), 'Prec must be exported so the keymap can be raised');
});

check('package.json declares autocomplete and lint rather than inheriting them', () => {
  const pkg = JSON.parse(fs.readFileSync(PACKAGE_JSON, 'utf8'));
  const declared = { ...(pkg.dependencies || {}), ...(pkg.devDependencies || {}) };
  assert.ok(declared['@codemirror/autocomplete'], '@codemirror/autocomplete must be a direct dependency');
  assert.ok(declared['@codemirror/lint'], '@codemirror/lint must be a direct dependency (Task 12 uses it)');
  const lock = JSON.parse(fs.readFileSync(path.join(ROOT, 'package-lock.json'), 'utf8'));
  const root = lock.packages && lock.packages[''];
  const locked = { ...((root && root.dependencies) || {}), ...((root && root.devDependencies) || {}) };
  assert.ok(locked['@codemirror/autocomplete'], 'the lockfile records the direct autocomplete dependency');
  assert.ok(locked['@codemirror/lint'], 'the lockfile records the direct lint dependency');
});

// --- wiring ----------------------------------------------------------------
check('index.html loads lsp-completion.js after its dependencies and before editor-pane.js', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (file) => html.indexOf(`app/features/editor/${file}`);
  assert.ok(at('lsp-completion.js') > 0, 'lsp-completion.js is loaded at all');
  assert.ok(at('lsp-completion.js') > at('lsp-state.js'), 'after lsp-state.js');
  assert.ok(at('lsp-completion.js') > at('lsp-bridge.js'), 'after lsp-bridge.js');
  assert.ok(
    at('lsp-completion.js') < at('editor-pane.js'),
    'before editor-pane.js, which mounts its extensions',
  );
});

check('manager-compose-runtime configures the completion module where the panes are in scope', () => {
  const source = fs.readFileSync(COMPOSE, 'utf8');
  assert.ok(/termlabLspCompletion/.test(source), 'the compose runtime wires the module');
  assert.ok(/paneForView/.test(source), 'it supplies the view -> pane lookup it alone can build');
});

check('editor-pane mounts the completion extensions without displacing vim', () => {
  const { sandbox } = load([VIM_MODE, COMPLETION, EDITOR_PANE]);
  const host = sandbox.document.createElement('div');
  const view = sandbox.termlabEditorPane.createEditorView(host, { doc: 'x', vimMode: true });
  const extensions = view.state.spec.extensions;
  const vimAt = extensions.findIndex(
    (e) => e && Array.isArray(e.contents) && e.contents.some((c) => c && c.ext === 'vim'),
  );
  assert.strictEqual(vimAt, 0, 'vim keeps the first slot — test_editor_vim_glue.mjs pins it');
  const autocompleteAt = extensions.findIndex((e) => e && e.ext === 'autocompletion');
  assert.ok(autocompleteAt > 0, 'the autocompletion extension is mounted');
  const raisedAt = extensions.findIndex((e) => e && e.ext === 'prec' && e.level === 'highest');
  assert.ok(raisedAt > 0, 'the completion keymap is mounted at raised precedence');
});

check('the module registers no window or document key handlers', () => {
  const source = fs.readFileSync(COMPLETION, 'utf8');
  assert.ok(
    !/addEventListener\(\s*['"]key(down|up|press)['"]/.test(source),
    'keyboard behaviour belongs in the CodeMirror keymap, never a global handler',
  );
  assert.ok(
    !/\bdocument\.addEventListener\b/.test(source),
    'no document-level listeners at all',
  );
  const { windowListeners } = harness();
  for (const type of windowListeners.keys()) {
    assert.ok(
      !/^key/.test(type),
      `configure() registered a "${type}" listener — a terminal pane would feel it`,
    );
  }
});

// --- manual invocation -----------------------------------------------------
check('Ctrl-Space in the keymap starts completion', () => {
  const h = harness();
  const binding = bindingFor(h.completion.keymapBindings(), 'Ctrl-Space');
  assert.ok(binding, 'Ctrl-Space is bound');
  assert.strictEqual(binding.run(h.view), true, 'and it consumes the key');
  assert.deepStrictEqual(
    h.cm.calls.map((c) => c[0]),
    ['startCompletion'],
  );
});

check('the configured editor_completion shortcut reaches the focused editor pane', () => {
  const h = harness();
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-completion'));
  assert.deepStrictEqual(h.cm.calls.map((c) => c[0]), ['startCompletion']);
  assert.strictEqual(h.cm.calls[0][1], h.view, 'on the focused pane\'s own view');
});

check('the shortcut is inert when the focused pane is not a local editor', () => {
  const terminal = harness({ focused: { paneId: 9, tabId: 1, kind: 'terminal', term: {} } });
  terminal.sandbox.dispatchEvent(new terminal.sandbox.CustomEvent('termlab:editor-completion'));
  assert.deepStrictEqual(terminal.cm.calls, [], 'a terminal pane never opens a completion popup');

  const none = harness({ focused: null });
  none.sandbox.dispatchEvent(new none.sandbox.CustomEvent('termlab:editor-completion'));
  assert.deepStrictEqual(none.cm.calls, []);
});

// --- when a request is made ------------------------------------------------
check('typing an identifier character requests completion', async () => {
  const h = harness({ response: responseFor([item()]) });
  const result = await h.completion.completionSource(h.context());
  assert.strictEqual(h.requests.length, 1, 'one request');
  assert.strictEqual(h.requests[0].kind, 'completion');
  assert.strictEqual(h.requests[0].target, h.pane);
  assert.strictEqual(h.requests[0].trigger, null, 'ordinary typing carries no trigger character');
  assert.ok(result && result.options.length === 1);
});

check('the position sent is the LSP position of the caret, not a CodeMirror offset', async () => {
  const h = harness({ text: 'const value = 1;\nva', response: responseFor([item()]) });
  await h.completion.completionSource(h.context());
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.requests[0].position)),
    { line: 1, character: 2 },
    'zero-based line, UTF-16 character offset within it',
  );
});

check('a server trigger character requests completion and names itself', async () => {
  const h = harness({
    text: 'value.',
    triggerCharacters: ['.', '"'],
    response: responseFor([item()]),
  });
  await h.completion.completionSource(h.context());
  assert.strictEqual(h.requests.length, 1);
  assert.strictEqual(h.requests[0].trigger, '.', 'the character the server asked to be woken by');
});

check('a character that is neither an identifier nor a trigger makes no request', async () => {
  const h = harness({ text: 'value + ', triggerCharacters: ['.'] });
  const result = await h.completion.completionSource(h.context());
  assert.strictEqual(result, null);
  assert.deepStrictEqual(h.requests, [], 'a space must not wake the language server');
});

check('an explicit invocation requests even with nothing typed', async () => {
  const h = harness({ text: 'value + ', response: responseFor([item()]) });
  const result = await h.completion.completionSource(h.context({ explicit: true }));
  assert.strictEqual(h.requests.length, 1);
  assert.strictEqual(h.requests[0].trigger, null);
  assert.ok(result);
});

check('suggestions-while-typing off leaves manual completion working', async () => {
  const h = harness({ response: responseFor([item()]) });
  h.completion.setSuggestionsWhileTyping(false);
  assert.strictEqual(await h.completion.completionSource(h.context()), null);
  assert.deepStrictEqual(h.requests, [], 'nothing automatic');
  assert.ok(await h.completion.completionSource(h.context({ explicit: true })), 'Ctrl-Space still works');
  assert.strictEqual(h.requests.length, 1);
});

check('a remote pane never reaches the language service', async () => {
  const h = harness({ remote: { paneId: 4, remotePath: '/srv/main.ts', hostLabel: 'box' } });
  assert.strictEqual(await h.completion.completionSource(h.context({ explicit: true })), null);
  assert.deepStrictEqual(h.requests, [], 'remote buffers are local-file-only territory');
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

// --- staleness -------------------------------------------------------------
check('an aborted context discards the response', async () => {
  const context = {};
  const h = harness({
    respond: () => { context.value.aborted = true; return responseFor([item()]); },
  });
  context.value = h.context({ explicit: true });
  assert.strictEqual(await h.completion.completionSource(context.value), null);
});

check('a response for another document is discarded', async () => {
  const h = harness({ response: responseFor([item()], { documentId: 'doc-2' }) });
  assert.strictEqual(await h.completion.completionSource(h.context({ explicit: true })), null);
});

check('a response tagged with an older document version is discarded', async () => {
  const h = harness({ response: responseFor([item()], { sourceVersion: 6 }) });
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

check('a null response (the service barrier already rejected it) yields no popup', async () => {
  const h = harness({ response: null });
  assert.strictEqual(await h.completion.completionSource(h.context({ explicit: true })), null);
});

// --- option translation ----------------------------------------------------
check('the completion range starts at the word being typed', async () => {
  const h = harness({ text: 'const value = 1;\nva', response: responseFor([item()]) });
  const result = await h.completion.completionSource(h.context());
  assert.strictEqual(result.from, 17, 'the "va" prefix, not the caret');
});

check('filter text drives matching while the label drives display', async () => {
  const h = harness({
    response: responseFor([item({ label: 'value (property)', filterText: 'value' })]),
  });
  const result = await h.completion.completionSource(h.context());
  const option = result.options[0];
  assert.strictEqual(option.label, 'value', 'CodeMirror filters on label');
  assert.strictEqual(option.displayLabel, 'value (property)', 'and shows the server label');
});

check('an item with no filter text matches on its label', async () => {
  const h = harness({ response: responseFor([item({ label: 'value' })]) });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.strictEqual(option.label, 'value');
  assert.strictEqual(option.displayLabel, undefined, 'no pointless duplicate');
});

check('detail and documentation are carried across, as text and never as markup', async () => {
  const h = harness({
    response: responseFor([item({
      detail: 'const value: number',
      documentation: [
        { markdown: false, value: 'A number.' },
        { markdown: true, value: '`value` is **not** html <img onerror=x>' },
      ],
    })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.strictEqual(option.detail, 'const value: number');
  assert.strictEqual(typeof option.info, 'function', 'documentation renders lazily');
  const node = await option.info(option);
  const text = JSON.stringify(node);
  assert.ok(/A number\./.test(text), 'the documentation is rendered');
  assert.ok(!/innerHTML/.test(fs.readFileSync(COMPLETION, 'utf8')), 'never through innerHTML');
  assert.ok(/onerror=x/.test(text), 'markdown source is shown as text, not interpreted');
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
    response: responseFor(kinds.map(([kind], index) => item({ id: `k${index}`, label: `l${index}`, kind }))),
  });
  const options = (await h.completion.completionSource(h.context())).options;
  assert.deepStrictEqual(
    options.map((option) => option.type),
    kinds.map(([, type]) => type),
  );
});

check('commit characters are carried to CodeMirror', async () => {
  const h = harness({ response: responseFor([item({ commitCharacters: ['.', '('] })]) });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(option.commitCharacters)),
    ['.', '('],
  );
  const plain = harness({ response: responseFor([item()]) });
  const bare = (await plain.completion.completionSource(plain.context())).options[0];
  assert.strictEqual(bare.commitCharacters, undefined, 'no empty array noise');
});

check('items are ranked by sortText, which is the client\'s job and not Rust\'s', async () => {
  const h = harness({
    response: responseFor([
      item({ id: 'a', label: 'zeta', sortText: '0000' }),
      item({ id: 'b', label: 'alpha', sortText: '0001' }),
      // No sortText: LSP says fall back to the label.
      item({ id: 'c', label: 'aaa', sortText: null }),
    ]),
  });
  const options = (await h.completion.completionSource(h.context())).options;
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(options.map((option) => option.label))),
    ['zeta', 'alpha', 'aaa'],
    '"0000" < "0001" < "aaa" — the wire order was zeta, alpha, aaa and stays so here only by accident of the keys',
  );
  assert.ok(options[0].boost > options[1].boost, 'and the ranking survives CodeMirror\'s own sort');
  assert.ok(options[0].boost <= 99, 'within the documented boost range');
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
    JSON.parse(JSON.stringify(options.map((option) => option.label))),
    ['first', 'second', 'also second'],
  );
});

check('a long list stays inside CodeMirror\'s boost range', async () => {
  const many = [];
  for (let i = 0; i < 400; i += 1) {
    many.push(item({ id: `i${i}`, label: `name${String(i).padStart(4, '0')}`, sortText: null }));
  }
  const h = harness({ response: responseFor(many) });
  const options = (await h.completion.completionSource(h.context())).options;
  for (const option of options) {
    assert.ok(option.boost >= -99 && option.boost <= 99, `boost ${option.boost} out of range`);
  }
});

// --- applying an item ------------------------------------------------------
check('a plain TextEdit applies as one transaction over the server\'s range', async () => {
  const h = harness({
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
  const option = (await h.completion.completionSource(h.context())).options[0];
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(h.view.dispatched.length, 1, 'exactly one transaction');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.view.dispatched[0].changes)),
    [{ from: 17, to: 19, insert: 'value' }],
    'the server range, converted to offsets — not CodeMirror\'s guess',
  );
});

check('an item with no text edit falls back to insertText, then to the label', async () => {
  const h = harness({
    response: responseFor([
      item({ id: 'a', label: 'value', insertText: 'value()' }),
      item({ id: 'b', label: 'other', insertText: null }),
    ]),
  });
  const options = (await h.completion.completionSource(h.context())).options;
  options[0].apply(h.view, options[0], 17, 19);
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.view.dispatched[0].changes)),
    [{ from: 17, to: 19, insert: 'value()' }],
  );
  options[1].apply(h.view, options[1], 17, 19);
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.view.dispatched[1].changes)),
    [{ from: 17, to: 19, insert: 'other' }],
  );
});

check('an InsertReplaceEdit uses its insert range', async () => {
  const h = harness({
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
  const option = (await h.completion.completionSource(h.context())).options[0];
  option.apply(h.view, option, 17, 22);
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.view.dispatched[0].changes)),
    [{ from: 17, to: 19, insert: 'valueOf' }],
    'insert, not replace — this phase never eats text to the right of the caret',
  );
});

check('a snippet expands through CodeMirror\'s own applier, in one transaction', async () => {
  const h = harness({
    response: responseFor([item({
      label: 'log',
      insertText: 'console.log(${1:value})',
      isSnippet: true,
    })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  option.apply(h.view, option, 17, 19);
  assert.ok(
    h.cm.calls.some((call) => call[0] === 'snippet'),
    'the snippet template goes through CM.snippet, not a naive insert',
  );
  assert.strictEqual(h.view.dispatched.length, 1, 'one transaction reaches the view');
  assert.ok(h.view.dispatched[0].spec.selection, 'the applier placed the first field');
  assert.ok(h.view.dispatched[0].spec.effects, 'and installed its placeholder state');
});

check('a snippet with additional edits still lands as one transaction', async () => {
  const h = harness({
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
  assert.strictEqual(h.view.dispatched.length, 1, 'one transaction, not snippet-then-import');
  const spec = h.view.dispatched[0];
  assert.ok(
    !spec.state,
    'the applier\'s transaction was intercepted as a spec so it could be merged',
  );
  assert.ok(spec.selection, 'the snippet\'s selection survived the merge');
  assert.ok(spec.effects, 'and so did its placeholder fields');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(spec.changes)),
    [
      { from: 39, to: 41, insert: 'console.log(value)' },
      { from: 8, to: 8, insert: 'log' },
    ],
    'the snippet\'s own change plus the additional edit, both against the pre-change document',
  );
});

check('a snippet applier that refuses to be intercepted still lands atomically', async () => {
  const h = harness({
    text: 'const value = 1;\nlo',
    response: responseFor([item({
      label: 'log',
      insertText: 'console.log(1)',
      isSnippet: true,
      additionalTextEdits: [{
        range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } },
        newText: '// ',
      }],
    })]),
  });
  // A future CodeMirror that builds its transaction without going through
  // state.update: the merge is impossible, so the snippet degrades to plain
  // text rather than splitting into two changes.
  h.cm.CM.snippet = () => (editor, completion, from, to) => {
    editor.dispatch({ state: {}, spec: { changes: { from, to, insert: 'console.log(1)' } } });
  };
  const option = (await h.completion.completionSource(h.context())).options[0];
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(h.view.dispatched.length, 1, 'still one transaction');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.view.dispatched[0].changes)),
    [
      { from: 0, to: 0, insert: '// ' },
      { from: 17, to: 19, insert: 'console.log(1)' },
    ],
  );
  assert.ok(
    h.completion.sessionLog().some((entry) => /plain text/.test(entry.message)),
    'and the degradation is recorded rather than silent',
  );
});

check('same-document additional edits ride in the same transaction', async () => {
  const h = harness({
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
    h.view.dispatched.length,
    1,
    'undo, dirty tracking and LSP synchronisation must see ONE change',
  );
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.view.dispatched[0].changes)),
    [
      { from: 8, to: 8, insert: 'value' },
      { from: 39, to: 41, insert: 'value' },
    ],
    'sorted ascending, all against the pre-change document',
  );
});

check('overlapping additional edits are refused rather than half-applied', async () => {
  const h = harness({
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
  const option = (await h.completion.completionSource(h.context())).options[0];
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(h.view.dispatched.length, 1);
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.view.dispatched[0].changes)),
    [{ from: 17, to: 19, insert: 'value' }],
    'the primary edit still lands; the conflicting extra one does not',
  );
  const logged = h.completion.sessionLog();
  assert.ok(
    logged.some((entry) => /overlap/i.test(entry.message)),
    'and the refusal is recorded',
  );
});

check('a cross-document workspace edit is visible and refused, not partially applied', async () => {
  const h = harness({
    response: responseFor([item({
      label: 'value',
      unsupportedEffects: ['workspaceEdit'],
    })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  const rendered = JSON.stringify(await option.info(option));
  assert.ok(
    /other files|another file|workspace/i.test(rendered),
    'the item says its workspace edit is not applied',
  );
  option.apply(h.view, option, 17, 19);
  assert.strictEqual(h.view.dispatched.length, 1, 'only this document changed');
  const logged = h.completion.sessionLog();
  assert.ok(
    logged.some((entry) => /workspace/i.test(entry.message)),
    'a session-log entry records the refusal',
  );
});

check('an unsupported command is surfaced the same way', async () => {
  const h = harness({
    response: responseFor([item({ unsupportedEffects: ['command'] })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  const rendered = JSON.stringify(await option.info(option));
  assert.ok(/command/i.test(rendered), 'the item explains what it could not run');
});

check('the session log is bounded', async () => {
  const h = harness({
    response: responseFor([item({ unsupportedEffects: ['workspaceEdit'] })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  for (let i = 0; i < 400; i += 1) option.apply(h.view, option, 17, 19);
  const logged = h.completion.sessionLog();
  assert.ok(logged.length > 0, 'it records something');
  assert.ok(logged.length <= 200, `bounded, got ${logged.length}`);
});

// --- resolve ---------------------------------------------------------------
check('completion-item resolve enriches documentation when the bridge offers it', async () => {
  const resolved = [];
  const h = harness({
    bridge: {
      resolveCompletionItem: async (id) => {
        resolved.push(id);
        return item({ id, documentation: [{ markdown: false, value: 'Resolved docs.' }] });
      },
    },
    response: responseFor([item({ id: 'item-9', documentation: [] })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  const rendered = JSON.stringify(await option.info(option));
  assert.deepStrictEqual(resolved, ['item-9'], 'resolve is asked for exactly the shown item');
  assert.ok(/Resolved docs\./.test(rendered));
  await option.info(option);
  assert.deepStrictEqual(resolved, ['item-9'], 'and asked once — the result is cached');
});

check('a failing resolve still renders what the item already had', async () => {
  const h = harness({
    bridge: { resolveCompletionItem: async () => { throw new Error('server gone'); } },
    response: responseFor([item({ documentation: [{ markdown: false, value: 'Local docs.' }] })]),
  });
  const option = (await h.completion.completionSource(h.context())).options[0];
  const rendered = JSON.stringify(await option.info(option));
  assert.ok(/Local docs\./.test(rendered), 'editing continues, documentation degrades');
});

check('no resolve method on the bridge is simply no resolve', async () => {
  const h = harness({ response: responseFor([item()]) });
  const option = (await h.completion.completionSource(h.context())).options[0];
  assert.ok(await option.info(option), 'the info panel still renders');
});

// --- vim ------------------------------------------------------------------
check('the completion keymap is raised above vim by precedence, not by position', () => {
  const h = harness();
  const extensions = h.completion.extensions();
  const raised = extensions.find((e) => e && e.ext === 'prec');
  assert.ok(raised, 'a Prec-wrapped extension is present');
  assert.strictEqual(raised.level, 'highest', 'vim\'s keymap only loses to Prec.highest');
  assert.strictEqual(raised.contents.ext, 'keymap', 'and what it raises is the keymap');
});

check('insert mode can open and accept completion', () => {
  const h = harness();
  const bindings = h.completion.keymapBindings();
  h.cm.setStatus(null);
  assert.strictEqual(bindingFor(bindings, 'Ctrl-Space').run(h.view), true, 'opens');
  h.cm.setStatus('active');
  assert.strictEqual(bindingFor(bindings, 'Enter').run(h.view), true, 'Enter accepts');
  assert.strictEqual(bindingFor(bindings, 'Tab').run(h.view), true, 'Tab accepts');
  assert.deepStrictEqual(
    h.cm.calls.map((c) => c[0]),
    ['startCompletion', 'acceptCompletion', 'acceptCompletion'],
  );
});

check('with the popup closed, Enter and Tab fall through to vim', () => {
  const h = harness();
  h.cm.setStatus(null);
  const bindings = h.completion.keymapBindings();
  assert.strictEqual(bindingFor(bindings, 'Enter').run(h.view), false, 'Enter is vim\'s again');
  assert.strictEqual(bindingFor(bindings, 'Tab').run(h.view), false);
  assert.deepStrictEqual(h.cm.calls, [], 'and nothing was even asked of the popup');
});

check('Escape closes the popup ONLY while it is open, so insert mode survives one press', () => {
  const h = harness();
  const escape = bindingFor(h.completion.keymapBindings(), 'Escape');
  assert.ok(escape, 'Escape is bound');

  h.cm.setStatus('active');
  assert.strictEqual(escape.run(h.view), true, 'the popup goes first');
  assert.deepStrictEqual(h.cm.calls.map((c) => c[0]), ['closeCompletion']);

  h.cm.calls.length = 0;
  h.cm.setStatus(null);
  assert.strictEqual(
    escape.run(h.view),
    false,
    'with no popup, Escape belongs to vim — leaving insert mode must not need two presses',
  );
  assert.deepStrictEqual(h.cm.calls, []);
});

check('a pending (not yet open) popup still owns Escape', () => {
  const h = harness();
  h.cm.setStatus('pending');
  assert.strictEqual(bindingFor(h.completion.keymapBindings(), 'Escape').run(h.view), true);
});

check('normal-mode keys are not bound at all', () => {
  const h = harness();
  const keys = h.completion.keymapBindings().map((binding) => binding.key);
  for (const key of ['j', 'k', 'h', 'l', 'i', 'a', 'o', 'd', 'w', 'y', 'p', 'u', 'v', 'x', ':', '/']) {
    assert.ok(!keys.includes(key), `"${key}" must stay vim's`);
  }
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(keys)).sort(),
    ['ArrowDown', 'ArrowUp', 'Ctrl-Space', 'Enter', 'Escape', 'PageDown', 'PageUp', 'Tab'],
    'the popup keys and nothing else, each guarded on the popup being open',
  );
});

check('the arrows navigate the list only while it is open', () => {
  const h = harness();
  const bindings = h.completion.keymapBindings();
  h.cm.setStatus(null);
  for (const key of ['ArrowDown', 'ArrowUp', 'PageDown', 'PageUp']) {
    assert.strictEqual(
      bindingFor(bindings, key).run(h.view),
      false,
      `${key} belongs to vim and the default keymap while no popup is open`,
    );
  }
  assert.deepStrictEqual(h.cm.calls, []);

  h.cm.setStatus('active');
  for (const key of ['ArrowDown', 'ArrowUp', 'PageDown', 'PageUp']) {
    assert.strictEqual(bindingFor(bindings, key).run(h.view), true, `${key} moves the selection`);
  }
  assert.deepStrictEqual(
    h.cm.calls.map((call) => [call[1], call[2]]),
    [[true, 'option'], [false, 'option'], [true, 'page'], [false, 'page']],
  );
});

check('automatic completion is driven by document changes, so vim normal mode is quiet', () => {
  const h = harness();
  const autocompletion = h.completion.extensions().find((e) => e && e.ext === 'autocompletion');
  assert.ok(autocompletion, 'the extension is configured here, not left to defaults');
  assert.strictEqual(
    autocompletion.config.activateOnTyping,
    true,
    'typing opens it — and vim normal-mode motion types nothing',
  );
  assert.strictEqual(
    autocompletion.config.defaultKeymap,
    false,
    'the dedicated keymap owns the keys; two competing keymaps is how Escape breaks',
  );
});

check('dispose removes the shortcut listener', () => {
  const h = harness();
  h.completion.dispose();
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-completion'));
  assert.deepStrictEqual(h.cm.calls, [], 'a disposed module answers nothing');
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
