// Run: node scripts/tests/test_editor_view_guards.mjs
//
// features/editor/editor-view-guards.js, and the reason it has to exist.
//
// The bug: with two editor tabs open, `gd` / `<C-o>` / `<C-i>` / a plain tab
// click hides the pane the pointer was last over. @codemirror/lint's hover
// tooltip — mounted on every pane, because lsp-diagnostics.js mounts
// `linter(null, …)` — has by then armed a 300ms dwell timer. When it fires it
// calls `view.posAtCoords(...)` on a view whose DOM has no boxes, and
// CodeMirror walks into
//
//   InlineCoordsScan.scan   -> `if (!below && !above) return { i: positions[0], after: false }`
//   InlineCoordsScan.scanTile -> `let child = tile.children[scan.i]; child.isText()`
//
// where `positions[0]` is the tile's DOCUMENT OFFSET, not a child index. For
// any line past the first, `child` is undefined and the app's status banner
// shows
//
//   Frontend error: TypeError: undefined is not an object (evaluating 'n.isText')
//
// Two kinds of check below:
//
//   * the guard's own behaviour, headless — it suppresses the lookup on a view
//     with no layout, leaves a laid-out view completely alone, and cannot be
//     stacked;
//   * a VENDOR SEAM check, against the shipped bundle, pinning the fact that
//     makes the guard load-bearing. If a CodeMirror upgrade ever fixes that
//     return, this test fails and says so — that is the signal to delete the
//     guard rather than carry it forever.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const EDITOR = path.join(APP, 'features/editor');
const GUARDS = path.join(EDITOR, 'editor-view-guards.js');
const EDITOR_PANE = path.join(EDITOR, 'editor-pane.js');
const INDEX_HTML = path.join(ROOT, 'index.html');
const BUNDLE = path.join(ROOT, 'vendor/codemirror/codemirror.js');

const results = [];
function check(name, fn) { results.push({ name, fn }); }

function load(files, extra) {
  const sandbox = { console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap, Array, Object, JSON };
  sandbox.window = sandbox;
  Object.assign(sandbox, extra || {});
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return sandbox;
}

// A view stand-in whose contentDOM reports exactly what a real one reports in
// each state: a laid-out element has at least one client rect, a `display:
// none` ancestor leaves it with none, and a detached node also reports
// isConnected false.
function makeView(options) {
  const opts = options || {};
  const calls = [];
  const view = {
    contentDOM: {
      isConnected: opts.connected !== false,
      getClientRects: () => (opts.laidOut === false ? [] : [{ top: 0, left: 0, width: 600, height: 400 }]),
    },
    posAtCoords(coords, precise) {
      calls.push({ coords, precise, self: this });
      if (opts.throws) throw new TypeError("undefined is not an object (evaluating 'n.isText')");
      return opts.answer === undefined ? 42 : opts.answer;
    },
  };
  return { view, calls };
}

// --- the predicate -----------------------------------------------------------

check('hasLayout is true for a connected element with client rects', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  assert.strictEqual(guards.hasLayout(makeView().view), true);
});

check('hasLayout is false when the pane is hidden — no rects, still connected', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  // This is the `display: none` case exactly: the node is in the document, it
  // simply has no boxes.
  assert.strictEqual(guards.hasLayout(makeView({ laidOut: false }).view), false);
});

check('hasLayout is false for a detached content element', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  assert.strictEqual(guards.hasLayout(makeView({ connected: false, laidOut: false }).view), false);
});

check('hasLayout errs towards yes when it cannot measure at all', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  // A wrong "no" would silently break coordinate lookups in an environment we
  // merely failed to measure, which is worse than the crash being guarded.
  assert.strictEqual(guards.hasLayout(null), true, 'no view');
  assert.strictEqual(guards.hasLayout({}), true, 'no contentDOM');
  assert.strictEqual(guards.hasLayout({ contentDOM: {} }), true, 'no getClientRects');
  assert.strictEqual(
    guards.hasLayout({ contentDOM: { getClientRects() { throw new Error('gone'); } } }), true,
    'a throwing DOM is not treated as hidden',
  );
});

// --- the shim ------------------------------------------------------------------

check('a hidden view answers null instead of reaching CodeMirror', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  const { view, calls } = makeView({ laidOut: false, throws: true });
  assert.strictEqual(guards.install(view), true);
  // Without the guard this is the crash; null is what posAtCoords already
  // returns for a position its DOM does not cover, so no caller is surprised.
  assert.strictEqual(view.posAtCoords({ x: 30, y: 190 }, false), null);
  assert.deepStrictEqual(calls, [], 'the original was never entered');
});

check('a laid-out view is untouched — same arguments, same answer, same receiver', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  const { view, calls } = makeView({ answer: 317 });
  const original = view.posAtCoords;
  guards.install(view);
  assert.strictEqual(view.posAtCoords({ x: 12, y: 34 }, false), 317);
  assert.strictEqual(calls.length, 1);
  assert.deepStrictEqual(calls[0].coords, { x: 12, y: 34 });
  assert.strictEqual(calls[0].precise, false, 'precise is forwarded, not dropped');
  assert.strictEqual(calls[0].self, view, 'and it still runs as a method of the view');
  assert.notStrictEqual(view.posAtCoords, original, 'the shim really did replace it');
});

check('a one-argument call still gets CodeMirror\'s own default for precise', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  const { view, calls } = makeView();
  guards.install(view);
  view.posAtCoords({ x: 1, y: 2 });
  assert.strictEqual(
    calls[0].precise, undefined,
    'undefined is forwarded so the default parameter applies; passing `false` here would change behaviour',
  );
});

check('installing twice does not stack wrappers', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  const { view } = makeView();
  assert.strictEqual(guards.install(view), true);
  const first = view.posAtCoords;
  assert.strictEqual(guards.install(view), false, 'the second install reports that it did nothing');
  assert.strictEqual(view.posAtCoords, first, 'and left the function alone');
});

check('a view with no posAtCoords is left alone rather than given one', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  assert.strictEqual(guards.install(null), false);
  const bare = {};
  assert.strictEqual(guards.install(bare), false);
  assert.strictEqual('posAtCoords' in bare, false);
});

check('the guard follows the view: hiding it after install changes the answer', () => {
  const guards = load([GUARDS]).termlabEditorViewGuards;
  // The real sequence — the pane is on screen when the dwell is armed and
  // hidden by the time it fires — so the predicate has to be read at CALL
  // time, not baked in at install time.
  let laidOut = true;
  const view = {
    contentDOM: {
      isConnected: true,
      getClientRects: () => (laidOut ? [{ width: 600, height: 400 }] : []),
    },
    posAtCoords: () => 99,
  };
  guards.install(view);
  assert.strictEqual(view.posAtCoords({ x: 1, y: 1 }), 99, 'armed while visible');
  laidOut = false;
  assert.strictEqual(view.posAtCoords({ x: 1, y: 1 }), null, 'fires while hidden');
});

// --- the wiring ------------------------------------------------------------------

// editor-pane.js's CM6 stand-in, cut down to what createEditorView touches.
// Only the view needs to be real-shaped: this asserts the guard is applied to
// what createEditorView returns.
function makePaneSandbox() {
  const sandbox = {
    console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap, Array, Object, JSON,
  };
  sandbox.window = sandbox;
  sandbox.document = {
    createElement: () => ({ appendChild() {}, style: {}, classList: { add() {}, remove() {} } }),
  };
  class Compartment {
    of(ext) { return { contents: ext }; }
    reconfigure(ext) { return { reconfigure: ext }; }
  }
  function EditorView(config) {
    this.state = config.state;
    this.contentDOM = { isConnected: true, getClientRects: () => [] };
    this.posAtCoords = () => { throw new TypeError("undefined is not an object (evaluating 'n.isText')"); };
    this.dispatch = () => {};
    this.destroy = () => {};
  }
  EditorView.updateListener = { of: (fn) => ({ ext: 'updateListener', fn }) };
  EditorView.theme = (spec) => ({ ext: 'theme', spec });
  const tagged = (name) => () => ({ ext: name });
  sandbox.CM6 = {
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
    defaultKeymap: [], historyKeymap: [], searchKeymap: [], foldKeymap: [],
    indentWithTab: 'indentWithTab',
    StreamLanguage: { define: (parser) => ({ ext: 'streamLanguage', parser }) },
  };
  return sandbox;
}

function buildPaneView(files) {
  const sandbox = makePaneSandbox();
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  const host = sandbox.document.createElement('div');
  return { sandbox, view: sandbox.termlabEditorPane.createEditorView(host, { doc: 'x', filename: 'a.rs' }) };
}

check('createEditorView guards every view it builds', () => {
  const { view } = buildPaneView([GUARDS, EDITOR_PANE]);
  // The stubbed view throws the real error from posAtCoords and reports no
  // layout — exactly a hidden pane. If the guard were not applied, this line
  // would throw rather than answer null.
  assert.strictEqual(view.posAtCoords({ x: 30, y: 190 }, false), null);
});

check('editor-pane still works with the guard module absent', () => {
  const { view } = buildPaneView([EDITOR_PANE]);
  assert.ok(view, 'a view is still built');
  assert.throws(
    () => view.posAtCoords({ x: 30, y: 190 }, false),
    /isText/,
    'unguarded is the old behaviour — the module is additive, not required',
  );
});

check('index.html loads the guard module before editor-pane.js', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const guardAt = html.indexOf('app/features/editor/editor-view-guards.js');
  const paneAt = html.indexOf('app/features/editor/editor-pane.js');
  assert.ok(guardAt > 0, 'index.html must load editor-view-guards.js or every view is unguarded');
  assert.ok(paneAt > 0);
  assert.ok(guardAt < paneAt, 'window.termlabEditorViewGuards has to exist before createEditorView runs');
});

// --- the vendor seam ------------------------------------------------------------
//
// Why this is asserted against the SHIPPED bundle and not node_modules: the
// bundle is what the app loads, and it is what the owner's crash came out of.

check('the shipped bundle still contains the scan branch the guard exists for', () => {
  assert.ok(
    fs.existsSync(BUNDLE),
    `vendor bundle missing: run "npm run build:vendor" in ${ROOT} (it is generated and git-ignored)`,
  );
  const bundle = fs.readFileSync(BUNDLE, 'utf8');
  // scan()'s no-rectangle escape hatch, minified: `if (!below && !above)
  // return { i: positions[0], after: false }`. Matched loosely on the shape
  // (two negated locals, then an object literal whose `i` is `<array>[0]` and
  // whose `after` is false) because the identifiers are minified and change
  // between builds.
  assert.match(
    bundle,
    /if\(!\w+&&!\w+\)return\{i:\w+\[0\],after:!1\}/,
    'the branch that returns a document offset where an index is expected is gone — '
    + 'if CodeMirror fixed it, delete app/features/editor/editor-view-guards.js and this test',
  );
  // And the consumer that turns it into the crash: `let child =
  // tile.children[scan.i]` followed by `child.isText()`.
  assert.match(
    bundle,
    /=\w+\.children\[\w+\.i\][^;]*;return \w+\.isText\(\)/,
    'scanTile no longer indexes children with the scan result — the guard may be removable',
  );
});

check('the lint hover that arms the dwell is really in the shipped bundle', () => {
  const bundle = fs.readFileSync(BUNDLE, 'utf8');
  // lsp-diagnostics.js mounts `linter(null, …)`, @codemirror/lint's
  // lintExtensions include `hoverTooltip(lintTooltip)`, and that plugin is
  // what schedules the coordinate lookup 300ms after a mousemove. Without it
  // in the bundle there would be no timer to outlive the pane.
  assert.ok(bundle.includes('hoverTime'), 'the hover dwell option is bundled');
  assert.match(bundle, /startHover/, 'and the deferred lookup that fires after it');
});

check('lsp-diagnostics mounts the linter on every pane — the dwell is not optional', () => {
  const source = fs.readFileSync(path.join(EDITOR, 'lsp-diagnostics.js'), 'utf8');
  assert.match(
    source, /CM\.linter\(/,
    'if the linter is ever mounted conditionally, the guard still holds but this comment does not',
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
  console.log(`editor view guards: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`editor view guards: all ${results.length} checks passed`);
}
