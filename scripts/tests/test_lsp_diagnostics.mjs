// Run: node scripts/tests/test_lsp_diagnostics.mjs
//
// LSP diagnostics rendered into CodeMirror editor panes.
//
// Same two halves as test_lsp_completion.mjs, for the same reason:
//
//   * the REAL half imports @codemirror/state and @codemirror/lint from
//     crates/termlab_tauri/frontend/node_modules and asserts on what actually
//     ends up in a view's state — the offsets forEachDiagnostic reports, the
//     severities, what a second publication replaced. A stub CM cannot fail
//     an offset-conversion bug, which is the only kind this module really has.
//
//   * the stub half covers routing, revision guards and message rendering,
//     where no CodeMirror behaviour is involved.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const MODULES = path.join(APP, 'features/editor');
const DIAGNOSTICS = path.join(MODULES, 'lsp-diagnostics.js');
const LSP_URI = path.join(MODULES, 'lsp-uri.js');
const BRIDGE = path.join(MODULES, 'lsp-bridge.js');
const EDITOR_PANE = path.join(MODULES, 'editor-pane.js');
const VIM_MODE = path.join(MODULES, 'vim-mode.js');
const VENDOR_ENTRY = path.join(ROOT, 'vendor-entry.mjs');
const INDEX_HTML = path.join(ROOT, 'index.html');
const COMPOSE = path.join(APP, 'manager-compose-runtime.js');
const NODE_MODULES = path.join(ROOT, 'node_modules');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

// --- fake DOM (message rendering only) --------------------------------------
function makeDocument() {
  function element(tag) {
    return {
      tagName: String(tag).toUpperCase(),
      className: '',
      textContent: '',
      children: [],
      style: {},
      dataset: {},
      classList: { add() {}, remove() {} },
      appendChild(child) { this.children.push(child); return child; },
      setAttribute() {},
      // Deliberately absent: innerHTML. Server text is never markup.
    };
  }
  return { createElement: (tag) => element(tag) };
}

function flatText(node) {
  if (!node) return '';
  const own = node.textContent ? String(node.textContent) : '';
  return own + (node.children || []).map(flatText).join('');
}

// --- the real CodeMirror packages -------------------------------------------
const REAL = {
  state: await import(pathToFileURL(path.join(NODE_MODULES, '@codemirror/state/dist/index.js')).href),
  view: await import(pathToFileURL(path.join(NODE_MODULES, '@codemirror/view/dist/index.js')).href),
  lint: await import(pathToFileURL(path.join(NODE_MODULES, '@codemirror/lint/dist/index.js')).href),
};

function realCM6() {
  return {
    EditorState: REAL.state.EditorState,
    Compartment: REAL.state.Compartment,
    Prec: REAL.state.Prec,
    EditorView: REAL.view.EditorView,
    linter: REAL.lint.linter,
    setDiagnostics: REAL.lint.setDiagnostics,
    lintGutter: REAL.lint.lintGutter,
    diagnosticCount: REAL.lint.diagnosticCount,
    forEachDiagnostic: REAL.lint.forEachDiagnostic,
  };
}

// A view with a REAL EditorState behind it: dispatch applies the transaction
// the way an EditorView would, so the marks can be read back out of the state.
function realView(doc) {
  return {
    state: REAL.state.EditorState.create({ doc }),
    dispatches: 0,
    dispatch(spec) {
      this.dispatches += 1;
      this.state = this.state.update(spec).state;
    },
  };
}

// What actually landed: [{from, to, severity, message}], in document order.
function marks(view) {
  const found = [];
  REAL.lint.forEachDiagnostic(view.state, (diagnostic, from, to) => {
    found.push({
      from, to, severity: diagnostic.severity, message: diagnostic.message,
    });
  });
  return JSON.parse(JSON.stringify(found));
}

// --- CM6 stand-in (stub half) ------------------------------------------------
function makeStubCM(sandbox) {
  const dispatched = [];
  const CM = {
    Compartment: class Compartment {
      constructor() { this.id = Math.random(); }
      of(ext) { return { compartment: this.id, contents: ext }; }
      reconfigure(ext) { return { reconfigure: this.id, contents: ext }; }
    },
    Prec: { highest: (ext) => ({ ext: 'prec', level: 'highest', contents: ext }) },
    keymap: { of: (bindings) => ({ ext: 'keymap', bindings }) },
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
    linter: (source, config) => ({ ext: 'linter', source, config }),
    lintGutter: () => ({ ext: 'lintGutter' }),
    setDiagnostics: (state, diagnostics) => {
      const spec = { ext: 'setDiagnostics', state, diagnostics };
      dispatched.push(spec);
      return spec;
    },
    diagnosticCount: () => 0,
    forEachDiagnostic: () => {},
    autocompletion: (config) => ({ ext: 'autocompletion', config }),
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
  return { CM, dispatched };
}

// --- document stand-in (stub half) ------------------------------------------
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

// --- sandbox -----------------------------------------------------------------
function load(files, extra, cmFactory) {
  const sandbox = {
    console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap, WeakSet,
    Array, Object, JSON, Date, RegExp, String, Number, Boolean, Math, Error,
    decodeURIComponent, encodeURIComponent, Intl,
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

const range = (sl, sc, el, ec) => ({
  start: { line: sl, character: sc },
  end: { line: el, character: ec },
});

const diag = (overrides) => ({
  id: 'd-1',
  uri: 'file:///repo/main.ts',
  range: range(0, 6, 0, 11),
  severity: 'error',
  code: null,
  source: null,
  message: 'Type error',
  relatedInformation: [],
  ...(overrides || {}),
});

const update = (items, overrides) => {
  const list = items || [];
  const counts = { errors: 0, warnings: 0, information: 0, hints: 0 };
  for (const entry of list) {
    if (entry.severity === 'error') counts.errors += 1;
    else if (entry.severity === 'warning') counts.warnings += 1;
    else if (entry.severity === 'information') counts.information += 1;
    else counts.hints += 1;
  }
  return {
    revision: 1,
    sessionId: 'session-1',
    documentId: 'doc-1',
    uri: 'file:///repo/main.ts',
    snapshot: { revision: 1, items: list, counts },
    ...(overrides || {}),
  };
};

// A pane plus the module wired the way manager-compose-runtime wires it.
function harness(options = {}) {
  const opts = options || {};
  const docText = opts.text === undefined ? 'const value = 1;\nconst other = 2;\n' : opts.text;
  const cmFactory = opts.real ? (sandbox) => {
    const CM = realCM6();
    sandbox.CM6 = CM;
    return { CM, dispatched: [] };
  } : undefined;
  const { sandbox, cm, windowListeners } = load([LSP_URI, DIAGNOSTICS], null, cmFactory);

  const view = opts.real ? realView(docText) : {
    state: { doc: makeDoc(docText) },
    dispatches: 0,
    dispatched: [],
    dispatch(spec) { this.dispatches += 1; this.dispatched.push(spec); },
  };

  const pane = {
    paneId: 4,
    tabId: 2,
    kind: 'editor',
    view,
    filePath: opts.filePath === undefined ? '/repo/main.ts' : opts.filePath,
    remote: opts.remote || null,
  };
  const panes = new Map([[pane.paneId, pane]]);
  for (const extra of (opts.extraPanes || [])) panes.set(extra.paneId, extra);

  const diagnostics = sandbox.termlabLspDiagnostics;
  diagnostics.configure({
    paneForView: (candidate) => (candidate === view ? pane : null),
    allPanes: () => panes,
  });

  return {
    sandbox, cm, windowListeners, view, pane, panes, diagnostics,
  };
}

// ===========================================================================
// REAL CodeMirror: what actually lands in the state
// ===========================================================================

check('a single-line range becomes the exact document offsets', () => {
  const h = harness({ real: true });
  h.diagnostics.applyUpdate(update([diag({ range: range(0, 6, 0, 11) })]));
  assert.deepStrictEqual(marks(h.view), [
    { from: 6, to: 11, severity: 'error', message: 'Type error' },
  ]);
});

check('a multiline range spans lines rather than clamping to the first', () => {
  const h = harness({ real: true, text: 'alpha\nbravo\ncharlie\n' });
  h.diagnostics.applyUpdate(update([diag({ range: range(0, 2, 2, 3) })]));
  // 'alpha\n' is 0..6, 'bravo\n' is 6..12, 'charlie' starts at 12.
  assert.deepStrictEqual(marks(h.view), [
    { from: 2, to: 15, severity: 'error', message: 'Type error' },
  ]);
});

check('a zero-width range survives as a zero-width mark', () => {
  const h = harness({ real: true, text: 'alpha\nbravo\n' });
  h.diagnostics.applyUpdate(update([diag({ range: range(1, 2, 1, 2) })]));
  assert.deepStrictEqual(marks(h.view), [
    { from: 8, to: 8, severity: 'error', message: 'Type error' },
  ]);
  assert.strictEqual(REAL.lint.diagnosticCount(h.view.state), 1);
});

check('every LSP severity maps to the CodeMirror severity CodeMirror knows', () => {
  const h = harness({ real: true, text: 'aaaa bbbb cccc dddd' });
  h.diagnostics.applyUpdate(update([
    diag({ id: 'a', severity: 'error', range: range(0, 0, 0, 4), message: 'e' }),
    diag({ id: 'b', severity: 'warning', range: range(0, 5, 0, 9), message: 'w' }),
    diag({ id: 'c', severity: 'information', range: range(0, 10, 0, 14), message: 'i' }),
    diag({ id: 'd', severity: 'hint', range: range(0, 15, 0, 19), message: 'h' }),
  ]));
  assert.deepStrictEqual(
    marks(h.view).map((m) => m.severity),
    ['error', 'warning', 'info', 'hint'],
    'CodeMirror knows hint/info/warning/error — "information" is not one of them',
  );
});

check('an out-of-range position is clamped instead of throwing', () => {
  const h = harness({ real: true, text: 'alpha\n' });
  h.diagnostics.applyUpdate(update([diag({ range: range(9, 40, 9, 80) })]));
  const found = marks(h.view);
  assert.strictEqual(found.length, 1);
  assert.ok(found[0].from <= h.view.state.doc.length);
  assert.ok(found[0].to <= h.view.state.doc.length);
});

check('a newer revision REPLACES the previous marks rather than adding to them', () => {
  const h = harness({ real: true, text: 'alpha\nbravo\n' });
  h.diagnostics.applyUpdate(update(
    [diag({ id: 'old', range: range(0, 0, 0, 5), message: 'old' })],
    { revision: 4 },
  ));
  h.diagnostics.applyUpdate(update(
    [diag({ id: 'new', range: range(1, 0, 1, 5), message: 'new' })],
    { revision: 5 },
  ));
  assert.deepStrictEqual(marks(h.view), [
    { from: 6, to: 11, severity: 'error', message: 'new' },
  ]);
});

check('an empty publication for this URI clears the marks', () => {
  const h = harness({ real: true });
  h.diagnostics.applyUpdate(update([diag()], { revision: 2 }));
  assert.strictEqual(REAL.lint.diagnosticCount(h.view.state), 1);
  h.diagnostics.applyUpdate(update([], { revision: 3 }));
  assert.strictEqual(REAL.lint.diagnosticCount(h.view.state), 0);
});

check('a stale revision is rejected and leaves the current marks alone', () => {
  const h = harness({ real: true });
  h.diagnostics.applyUpdate(update([diag({ message: 'current' })], { revision: 9 }));
  const before = h.view.dispatches;
  h.diagnostics.applyUpdate(update([], { revision: 8 }));
  assert.strictEqual(h.view.dispatches, before, 'no transaction at all for a stale event');
  assert.deepStrictEqual(marks(h.view).map((m) => m.message), ['current']);
});

check('an equal revision is rejected too — the store never reuses one', () => {
  const h = harness({ real: true });
  h.diagnostics.applyUpdate(update([diag({ message: 'current' })], { revision: 9 }));
  const before = h.view.dispatches;
  h.diagnostics.applyUpdate(update([diag({ message: 'replay' })], { revision: 9 }));
  assert.strictEqual(h.view.dispatches, before);
  assert.deepStrictEqual(marks(h.view).map((m) => m.message), ['current']);
});

check('only this pane\'s URI reaches this pane', () => {
  const h = harness({ real: true, text: 'alpha\nbravo\n' });
  h.diagnostics.applyUpdate(update([
    diag({ id: 'mine', uri: 'file:///repo/main.ts', range: range(0, 0, 0, 5), message: 'mine' }),
    diag({ id: 'theirs', uri: 'file:///repo/other.ts', range: range(0, 0, 0, 5), message: 'theirs' }),
  ]));
  assert.deepStrictEqual(marks(h.view).map((m) => m.message), ['mine']);
});

check('a percent-encoded URI still matches its pane', () => {
  const h = harness({ real: true, filePath: '/repo/my project/main.ts' });
  h.diagnostics.applyUpdate(update(
    [diag({ uri: 'file:///repo/my%20project/main.ts' })],
    { uri: 'file:///repo/my%20project/main.ts' },
  ));
  assert.strictEqual(REAL.lint.diagnosticCount(h.view.state), 1);
});

// ===========================================================================
// Routing and ownership (stub half)
// ===========================================================================

check('an update for another file leaves an unaffected pane untouched', () => {
  const h = harness();
  h.diagnostics.applyUpdate(update(
    [diag({ uri: 'file:///repo/other.ts' })],
    { uri: 'file:///repo/other.ts' },
  ));
  assert.strictEqual(h.view.dispatches, 0, 'no dispatch, so no wasted transaction');
});

check('a remote pane never renders diagnostics', () => {
  const h = harness({ remote: { paneId: 'ssh-1' } });
  h.diagnostics.applyUpdate(update([diag()]));
  assert.strictEqual(h.view.dispatches, 0);
});

check('an untitled pane with no path is skipped', () => {
  const h = harness({ filePath: null });
  h.diagnostics.applyUpdate(update([diag()]));
  assert.strictEqual(h.view.dispatches, 0);
});

check('a terminal pane in the same window is skipped', () => {
  const terminal = { paneId: 7, tabId: 1, kind: 'terminal', term: {} };
  const h = harness({ extraPanes: [terminal] });
  h.diagnostics.applyUpdate(update([diag()]));
  assert.strictEqual(h.view.dispatches, 1, 'the editor pane still renders');
});

// Rust ships the whole workspace snapshot with every publication, so a
// diagnostic appearing in ANY file re-delivers every other file's marks
// unchanged. Without this guard, one keystroke in a large project costs a
// CodeMirror transaction (and a full decoration rebuild) in every open pane
// whose file happens to appear in the snapshot.
check('a publication that changes only another file does not touch this pane', () => {
  const h = harness({ real: true, text: 'alpha\nbravo\n' });
  const mine = diag({ id: 'mine', uri: 'file:///repo/main.ts', message: 'mine' });
  h.diagnostics.applyUpdate(update(
    [mine, diag({ id: 'a', uri: 'file:///repo/other.ts', message: 'was' })],
    { revision: 4, uri: 'file:///repo/other.ts' },
  ));
  assert.strictEqual(h.view.dispatches, 1);
  h.diagnostics.applyUpdate(update(
    [mine, diag({ id: 'a', uri: 'file:///repo/other.ts', message: 'now' })],
    { revision: 5, uri: 'file:///repo/other.ts' },
  ));
  assert.strictEqual(h.view.dispatches, 1, 'this pane\'s marks are identical — nothing to dispatch');
  assert.deepStrictEqual(marks(h.view).map((m) => m.message), ['mine'], 'and they are still shown');
});

check('the skipped pane still counts as up to date at that revision', () => {
  const h = harness({ real: true, text: 'alpha\nbravo\n' });
  const mine = diag({ id: 'mine', uri: 'file:///repo/main.ts', message: 'mine' });
  h.diagnostics.applyUpdate(update([mine], { revision: 4 }));
  h.diagnostics.applyUpdate(update([mine], { revision: 5 }));
  assert.strictEqual(h.view.dispatches, 1);
  // A revision BELOW the skipped one must still be rejected: skipping the
  // transaction may not quietly rewind what this pane claims to know.
  h.diagnostics.applyUpdate(update([], { revision: 5 }));
  assert.deepStrictEqual(marks(h.view).map((m) => m.message), ['mine']);
  h.diagnostics.applyUpdate(update([], { revision: 6 }));
  assert.deepStrictEqual(marks(h.view), [], 'a genuine change still lands');
});

check('a change confined to source or code still re-renders the tooltip', () => {
  const h = harness();
  h.diagnostics.applyUpdate(update([diag({ source: 'ts', code: '1' })], { revision: 2 }));
  h.diagnostics.applyUpdate(update([diag({ source: 'ts', code: '2' })], { revision: 3 }));
  assert.strictEqual(h.view.dispatches, 2, 'the tooltip text changed even though the range did not');
});

check('a malformed update is ignored rather than thrown on', () => {
  const h = harness();
  for (const bad of [null, {}, { revision: 'x' }, { revision: 1 }]) {
    h.diagnostics.applyUpdate(bad);
  }
  assert.strictEqual(h.view.dispatches, 0);
});

check('the module keeps no merged diagnostics array of its own', () => {
  const source = fs.readFileSync(DIAGNOSTICS, 'utf8');
  assert.ok(
    !/\.concat\(|\.push\(.*diagnostic/i.test(source)
    || !/merged/i.test(source),
    'Rust owns the authoritative list; this module only projects a snapshot',
  );
  const h = harness();
  h.diagnostics.applyUpdate(update([diag()], { revision: 2 }));
  const first = h.cm.dispatched[0];
  h.diagnostics.applyUpdate(update([diag({ id: 'd-2', message: 'second' })], { revision: 3 }));
  const second = h.cm.dispatched[1];
  assert.strictEqual(first.diagnostics.length, 1);
  assert.strictEqual(second.diagnostics.length, 1, 'the second publication replaces, never appends');
  assert.strictEqual(second.diagnostics[0].message, 'second');
});

// ===========================================================================
// Message rendering
// ===========================================================================

check('a tooltip shows the message, then the source and code', () => {
  const h = harness();
  h.diagnostics.applyUpdate(update([diag({
    message: 'Cannot find name "foo".', source: 'typescript', code: '2304',
  })]));
  const rendered = h.cm.dispatched[0].diagnostics[0].renderMessage();
  const text = flatText(rendered);
  assert.ok(text.includes('Cannot find name "foo".'), text);
  assert.ok(text.includes('typescript(2304)'), text);
});

check('a source with no code, and a code with no source, both render', () => {
  const h = harness();
  h.diagnostics.applyUpdate(update([diag({ source: 'eslint', code: null })]));
  assert.ok(flatText(h.cm.dispatched[0].diagnostics[0].renderMessage()).includes('eslint'));

  const other = harness();
  other.diagnostics.applyUpdate(update([diag({ source: null, code: 'E0432' })]));
  assert.ok(flatText(other.cm.dispatched[0].diagnostics[0].renderMessage()).includes('E0432'));
});

check('a diagnostic with neither source nor code renders only its message', () => {
  const h = harness();
  h.diagnostics.applyUpdate(update([diag({ source: null, code: null, message: 'Bare.' })]));
  assert.strictEqual(flatText(h.cm.dispatched[0].diagnostics[0].renderMessage()), 'Bare.');
});

check('server text is inserted as text, never as markup', () => {
  const h = harness();
  h.diagnostics.applyUpdate(update([diag({ message: '<img src=x onerror=alert(1)>' })]));
  const rendered = h.cm.dispatched[0].diagnostics[0].renderMessage();
  assert.ok(flatText(rendered).includes('<img src=x'));
  assert.strictEqual(rendered.innerHTML, undefined, 'the fake element has no innerHTML at all');
  const source = fs.readFileSync(DIAGNOSTICS, 'utf8');
  assert.ok(!/innerHTML/.test(source), 'lsp-diagnostics.js must never touch innerHTML');
});

// ===========================================================================
// Wiring
// ===========================================================================

check('the vendor entry exports the lint primitives the renderer needs', () => {
  const entry = fs.readFileSync(VENDOR_ENTRY, 'utf8');
  for (const name of ['linter', 'setDiagnostics', 'lintGutter', 'diagnosticCount', 'forEachDiagnostic']) {
    assert.ok(new RegExp(`\\b${name}\\b`).test(entry), `${name} is not re-exported`);
  }
  assert.ok(/@codemirror\/lint/.test(entry));
});

check('the built bundle really carries them', () => {
  const bundle = path.join(ROOT, 'vendor/codemirror/codemirror.js');
  if (!fs.existsSync(bundle)) return; // vendor/ is gitignored; built by npm run build:vendor
  const source = fs.readFileSync(bundle, 'utf8');
  for (const name of ['setDiagnostics', 'lintGutter', 'forEachDiagnostic']) {
    assert.ok(source.includes(name), `${name} missing from the built bundle`);
  }
});

check('editor-pane mounts the diagnostics extensions behind vim', () => {
  const { sandbox } = load([VIM_MODE, LSP_URI, DIAGNOSTICS, EDITOR_PANE]);
  const host = sandbox.document.createElement('div');
  const view = sandbox.termlabEditorPane.createEditorView(host, { doc: 'x', vimMode: true });
  const extensions = view.state.spec.extensions;
  const vimAt = extensions.findIndex(
    (e) => e && Array.isArray(e.contents) && e.contents.some((c) => c && c.ext === 'vim'),
  );
  assert.strictEqual(vimAt, 0, 'vim keeps the first slot');
  assert.ok(extensions.some((e) => e && e.ext === 'linter'), 'the lint config is mounted');
  assert.ok(extensions.some((e) => e && e.ext === 'lintGutter'), 'so are the gutter markers');
});

check('editor-pane still works when the diagnostics module is missing', () => {
  const { sandbox } = load([VIM_MODE, EDITOR_PANE]);
  const host = sandbox.document.createElement('div');
  const view = sandbox.termlabEditorPane.createEditorView(host, { doc: 'x' });
  assert.ok(view, 'a stale bundle costs diagnostics and nothing else');
});

check('lsp-bridge fans the diagnostics event out from ONE listener', () => {
  const source = fs.readFileSync(BRIDGE, 'utf8');
  const listens = source.match(/listen\('lsp-diagnostics-updated'/g) || [];
  assert.strictEqual(listens.length, 1, 'exactly one subscription to the Tauri event');
  assert.ok(/subscribeDiagnostics/.test(source), 'and a fan-out other modules subscribe to');
});

check('a bridge subscriber receives the payload and can unsubscribe', () => {
  const { sandbox } = load([BRIDGE]);
  const listeners = new Map();
  sandbox.termlabServices = {
    tauriClient: {
      invoke: () => Promise.resolve(null),
      listenOnCurrentWindow: (name, handler) => {
        listeners.set(name, handler);
        return Promise.resolve(() => listeners.delete(name));
      },
    },
  };
  sandbox.termlabLspBridge.configure({ windowLabel: 'main' });
  const seen = [];
  const off = sandbox.termlabLspBridge.subscribeDiagnostics((payload) => seen.push(payload));
  listeners.get('lsp-diagnostics-updated')({ payload: { revision: 3 } });
  off();
  listeners.get('lsp-diagnostics-updated')({ payload: { revision: 4 } });
  assert.deepStrictEqual(JSON.parse(JSON.stringify(seen)), [{ revision: 3 }]);
});

check('a subscriber that throws does not stop the others', () => {
  const { sandbox } = load([BRIDGE]);
  const listeners = new Map();
  sandbox.termlabServices = {
    tauriClient: {
      invoke: () => Promise.resolve(null),
      listenOnCurrentWindow: (name, handler) => {
        listeners.set(name, handler);
        return Promise.resolve(() => {});
      },
    },
  };
  sandbox.termlabLspBridge.configure({ windowLabel: 'main' });
  const seen = [];
  sandbox.termlabLspBridge.subscribeDiagnostics(() => { throw new Error('boom'); });
  sandbox.termlabLspBridge.subscribeDiagnostics((payload) => seen.push(payload.revision));
  listeners.get('lsp-diagnostics-updated')({ payload: { revision: 7 } });
  assert.deepStrictEqual(seen, [7]);
});

check('the renderer subscribes through the bridge, never to Tauri itself', () => {
  const source = fs.readFileSync(DIAGNOSTICS, 'utf8');
  assert.ok(/subscribeDiagnostics/.test(source), 'it uses the bridge fan-out');
  assert.ok(!/\binvoke\(/.test(source), 'and never calls invoke() directly');
  assert.ok(!/listenOnCurrentWindow|__TAURI__/.test(source));
});

check('index.html loads lsp-diagnostics.js before editor-pane.js', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(name);
  assert.ok(at('app/features/editor/lsp-diagnostics.js') > 0, 'the module is loaded at all');
  assert.ok(
    at('app/features/editor/lsp-diagnostics.js') < at('app/features/editor/editor-pane.js'),
    'editor-pane reads termlabLspDiagnostics at view-construction time',
  );
});

check('the compose runtime configures the renderer with the lookups only it has', () => {
  const source = fs.readFileSync(COMPOSE, 'utf8');
  assert.ok(/termlabLspDiagnostics/.test(source));
  assert.ok(/allPanes/.test(source));
});

check('the diagnostics module uses no regex lookbehind', () => {
  // A regex literal is validated when the FILE is parsed, so one lookbehind
  // would stop the module loading at all on an older WKWebView — and because
  // every call site guards on the module being present, the failure would be
  // silent: the editor would simply never show a squiggle.
  const source = fs.readFileSync(DIAGNOSTICS, 'utf8');
  assert.ok(!/\(\?<[=!]/.test(source), 'lookbehind costs the whole file on an older WKWebView');
});

check('the module registers no window or document key handlers', () => {
  const source = fs.readFileSync(DIAGNOSTICS, 'utf8');
  assert.ok(!/addEventListener\(\s*['"]key(down|up|press)['"]/.test(source));
  assert.ok(!/\bdocument\.addEventListener\b/.test(source));
});

// A raw control byte anywhere in a source file makes git call the whole file
// BINARY: no diff, no blame, no review on a public repo, and grep-based
// tooling skips it silently. Easy to introduce (a "collision-proof" sentinel
// string is the classic way) and invisible in every editor.
check('the editor LSP modules contain no control bytes', () => {
  for (const file of [DIAGNOSTICS, LSP_URI]) {
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      const byte = bytes[i];
      assert.ok(
        byte >= 0x20 || byte === 0x0a || byte === 0x09,
        `${file}: control byte 0x${byte.toString(16)} at offset ${i} — git treats the file as binary`,
      );
    }
  }
});

check('the diagnostic surface classes it renders are styled with tokens', () => {
  const css = fs.readFileSync(
    path.join(ROOT, 'styles/design-system/components/editor.css'), 'utf8',
  );
  for (const name of ['tl-diagnostic', 'tl-diagnostic__message', 'tl-diagnostic__meta']) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  const block = css.slice(css.indexOf('.tl-diagnostic'));
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(block), 'tokens only, no hex colours');
});

check('dispose stops the bridge subscription', () => {
  const h = harness();
  let unsubscribed = false;
  h.sandbox.termlabLspBridge = {
    subscribeDiagnostics: () => () => { unsubscribed = true; },
  };
  h.diagnostics.configure({ allPanes: () => h.panes });
  h.diagnostics.dispose();
  assert.strictEqual(unsubscribed, true);
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
  console.log(`lsp diagnostics: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`lsp diagnostics: all ${ran} checks passed`);
}
