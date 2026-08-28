// Run: node scripts/tests/test_lsp_navigation.mjs
//
// Go to Definition, the multiple-definition chooser, and the per-window
// back/forward navigation history.
//
// Three halves, following test_lsp_tooltips.mjs:
//
//   * the REAL half — the chooser lives in a CodeMirror StateField read by the
//     showTooltip facet, and its keydown handler has to beat vim, so those are
//     asserted against the shipped packages in
//     crates/termlab_tauri/frontend/node_modules rather than against a stub.
//
//   * the CONTROLLER half — request gating, URI rejection, history stacks and
//     rendering, with editor-service stubbed.
//
//   * the SERVICE half — the real features/editor/editor-service.js over a
//     stubbed Tauri client, because "route every target through the editor
//     service" is only true if the service's own ownership answers (focus an
//     open tab, focus an owner in another window) reach the navigator.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const MODULES = path.join(APP, 'features/editor');
const NAVIGATION = path.join(MODULES, 'lsp-navigation.js');
// The history stacks and the chooser's field/DOM are siblings of the
// controller; index.html loads all three, and so does every harness here.
const HISTORY = path.join(MODULES, 'lsp-navigation-history.js');
const CHOOSER = path.join(MODULES, 'lsp-navigation-chooser.js');
const NAVIGATION_MODULES = [HISTORY, CHOOSER, NAVIGATION];
const URI = path.join(MODULES, 'lsp-uri.js');
const TOOLTIPS = path.join(MODULES, 'lsp-tooltips.js');
// Position conversion lives in its own module now (lsp-position.js); every
// harness that loads a converting module loads it too, the way index.html does.
const POSITION = path.join(MODULES, 'lsp-position.js');
const VIM_MODE = path.join(MODULES, 'vim-mode.js');
const PANE_MANAGER = path.join(APP, 'pane-manager.js');
const EDITOR_SERVICE = path.join(MODULES, 'editor-service.js');
const LSP_STATE = path.join(MODULES, 'lsp-state.js');
const LSP_BRIDGE = path.join(MODULES, 'lsp-bridge.js');
const LANGUAGE_MAP = path.join(MODULES, 'language-map.js');
const TAB_LABEL = path.join(MODULES, 'tab-label.js');
const EDITOR_PANE = path.join(MODULES, 'editor-pane.js');
const INDEX_HTML = path.join(ROOT, 'index.html');
const COMPOSE = path.join(APP, 'manager-compose-runtime.js');
const EDITOR_CSS = path.join(ROOT, 'styles/design-system/components/editor.css');
const NODE_MODULES = path.join(ROOT, 'node_modules');
const INPUT_RUNTIME = path.join(APP, 'input-runtime.js');
const SHORTCUT_RUNTIME = path.join(APP, 'shortcut-runtime.js');
const KEYBOARD_DEFAULTS = path.resolve(
  import.meta.dirname, '../../crates/termlab_core/src/config/termlab.rs',
);
const LSP_CLIENT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/lsp/client.rs');
const LSP_TYPES = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/lsp/types.rs');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

const sleep = (ms) => new Promise((resolve) => { setTimeout(resolve, ms); });

// --- fake DOM ---------------------------------------------------------------
//
// Deliberately without innerHTML: a chooser row that reaches for it is putting
// a path (which the user may not have typed) into the document as markup.
function makeDocument() {
  function element(tag) {
    return {
      tagName: String(tag).toUpperCase(),
      className: '',
      textContent: '',
      children: [],
      style: {},
      listeners: new Map(),
      classList: { add() {}, remove() {} },
      appendChild(child) { this.children.push(child); return child; },
      setAttribute(name, value) { this[`attr:${name}`] = String(value); },
      addEventListener(type, handler) {
        if (!this.listeners.has(type)) this.listeners.set(type, []);
        this.listeners.get(type).push(handler);
      },
      fire(type, event) {
        for (const handler of (this.listeners.get(type) || []).slice()) handler(event);
      },
    };
  }
  return { createElement: (tag) => element(tag) };
}

function flatten(node) {
  const out = [node];
  for (const child of node.children || []) out.push(...flatten(child));
  return out;
}

function textOf(node) {
  if (!node.children || !node.children.length) return node.textContent || '';
  return (node.textContent || '') + node.children.map(textOf).join('');
}

// --- the real CodeMirror packages -------------------------------------------
async function loadRealCM() {
  const pkg = async (name) => import(
    pathToFileURL(path.join(NODE_MODULES, name, 'dist/index.js')).href
  );
  return {
    state: await pkg('@codemirror/state'),
    view: await pkg('@codemirror/view'),
    autocomplete: await pkg('@codemirror/autocomplete'),
    vim: await pkg('@replit/codemirror-vim'),
  };
}
const REAL = await loadRealCM();

const VIEW_PLUGIN_FACET = REAL.view.ViewPlugin.define(() => ({})).extension[0].facet;

function keydownOwners(state) {
  return state.facet(VIEW_PLUGIN_FACET)
    .filter((entry) => entry.plugin.domEventHandlers && entry.plugin.domEventHandlers.keydown)
    .map((entry) => entry.plugin.id);
}

function realCM6() {
  const { state, view, autocomplete } = REAL;
  return {
    EditorState: state.EditorState,
    StateField: state.StateField,
    StateEffect: state.StateEffect,
    Compartment: state.Compartment,
    Prec: state.Prec,
    EditorView: view.EditorView,
    keymap: view.keymap,
    showTooltip: view.showTooltip,
    lineNumbers: view.lineNumbers,
    completionStatus: autocomplete.completionStatus,
  };
}

// --- sandbox -----------------------------------------------------------------
function load(files, extra, cm) {
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
  const toasts = [];
  sandbox.toast = {
    error(title, body) { toasts.push(['error', title, body]); },
    success(title, body) { toasts.push(['success', title, body]); },
    info(title, body) { toasts.push(['info', title, body]); },
    warn(title, body) { toasts.push(['warn', title, body]); },
  };
  sandbox.CM6 = cm === undefined ? realCM6() : cm;
  Object.assign(sandbox, extra || {});
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return { sandbox, windowListeners, toasts };
}

const CAPABILITIES = {
  completion: true, hover: true, signatureHelp: true, definition: true, diagnostics: true,
};

const ORIGIN_TEXT = 'import { format } from "./fmt";\nconst value = format(1);\n';
const TARGET_TEXT = 'export function format(value: number): string {\n  return String(value);\n}\n';

// --- the controller harness -----------------------------------------------------
//
// A window with one or more editor panes, a stubbed lsp-state and a stubbed
// editor-service, wired the way manager-compose-runtime wires them.
function harness(options = {}) {
  const opts = options || {};
  const { sandbox, windowListeners, toasts } = load([URI, POSITION].concat(NAVIGATION_MODULES));
  const navigation = sandbox.termlabLspNavigation;
  const extensions = navigation.extensions();

  const panes = new Map();
  const states = new Map();
  const focused = [];
  let nextPane = 1;

  function addPane(filePath, text, paneOptions) {
    const id = nextPane++;
    const view = {
      state: REAL.state.EditorState.create({ doc: text, extensions }),
      dispatches: [],
      dispatch(spec) {
        this.dispatches.push(spec);
        this.state = this.state.update(spec).state;
      },
      focus() { focused.push(`view:${id}`); },
      posAtCoords: (coords) => coords.x,
      hasFocus: true,
    };
    const pane = {
      paneId: id, tabId: id + 100, kind: 'editor', view, filePath, remote: null,
    };
    panes.set(id, pane);
    states.set(pane, {
      documentId: `doc-${id}`,
      version: 3,
      capabilities: { ...CAPABILITIES, ...((paneOptions || {}).capabilities || {}) },
      status: { revision: 2, state: 'ready', capabilities: CAPABILITIES },
    });
    return pane;
  }

  const origin = addPane(
    opts.originPath === undefined ? '/repo/src/main.ts' : opts.originPath,
    opts.originText === undefined ? ORIGIN_TEXT : opts.originText,
  );
  sandbox.termlabLspState = { get: (pane) => states.get(pane) || null };

  const opens = [];
  let openResponder = opts.open || (() => ({ status: 'opened', pane: null, revealed: false }));
  // The focused pane follows a successful open, the way it does in the app —
  // which is what makes the location Back captures the one the user is looking
  // at rather than the one they started the session in.
  let focusedPane = origin;
  sandbox.termlabEditorService = {
    openLocalFileAt: (filePath, range, openOptions) => {
      opens.push({ filePath, range, options: openOptions });
      const result = openResponder(filePath, range, opens.length);
      if (result && result.pane) focusedPane = result.pane;
      return Promise.resolve(result);
    },
  };

  const requests = [];
  let responder = opts.respond || (() => null);
  navigation.configure({
    paneForView: (view) => {
      for (const pane of panes.values()) if (pane.view === view) return pane;
      return null;
    },
    currentPane: () => (opts.currentPane ? opts.currentPane(panes) : focusedPane),
    allPanes: () => panes,
    windowLabel: 'main',
    requestFeature: (pane, kind, position, trigger) => {
      requests.push({ pane, kind, position, trigger });
      return Promise.resolve(responder(kind, position, requests.length));
    },
  });

  // What pane-manager does on every focus change: the focused pane moves, and
  // the navigator is told. Tests drive switches through this rather than
  // poking the module, so `currentPane()` and the notification cannot drift
  // apart the way they never do in the app.
  function focus(pane) {
    const previous = focusedPane;
    focusedPane = pane;
    return navigation.noteFocusedPaneChanged(previous, pane);
  }

  return {
    sandbox,
    navigation,
    extensions,
    focus,
    panes,
    states,
    origin,
    opens,
    requests,
    toasts,
    focused,
    windowListeners,
    addPane,
    setResponder: (fn) => { responder = fn; },
    setOpen: (fn) => { openResponder = fn; },
    history: () => JSON.parse(JSON.stringify(navigation.historyState())),
    chooser: (view) => JSON.parse(JSON.stringify(navigation.chooserState(view || origin.view))),
  };
}

function definitionResponse(locations, documentId = 'doc-1') {
  return { documentId, sourceVersion: 3, locations };
}

function location(uri, line, character, endCharacter) {
  return {
    uri,
    range: {
      start: { line, character },
      end: { line, character: endCharacter === undefined ? character + 6 : endCharacter },
    },
  };
}

const FMT_URI = 'file:///repo/src/fmt.ts';

// --- one definition ---------------------------------------------------------------

check('a single definition navigates immediately through the editor service', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen(() => ({ status: 'opened', pane: target, revealed: true }));
  const status = await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(status, 'navigated');
  assert.strictEqual(h.opens.length, 1, 'exactly one target is opened');
  assert.strictEqual(h.opens[0].filePath, '/repo/src/fmt.ts', 'the URI is converted by lsp-uri');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.opens[0].range)),
    { start: { line: 0, character: 16 }, end: { line: 0, character: 22 } },
    'the server range travels to the reveal',
  );
  assert.strictEqual(h.navigation.chooserState(h.origin.view).open, false, 'no chooser for one result');
});

check('the definition request is made at the requested position, with no trigger', async () => {
  const h = harness({ respond: () => definitionResponse([]) });
  await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(h.requests.length, 1);
  assert.strictEqual(h.requests[0].kind, 'definition');
  assert.strictEqual(h.requests[0].trigger, null);
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.requests[0].position)), { line: 1, character: 13 },
  );
});

check('a target in the same file still routes through the editor service', async () => {
  const h = harness({
    respond: () => definitionResponse([location('file:///repo/src/main.ts', 0, 9, 15)]),
  });
  h.setOpen(() => ({ status: 'focused', pane: h.origin, revealed: true }));
  const status = await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(status, 'navigated');
  assert.deepStrictEqual(
    h.opens.map((entry) => entry.filePath), ['/repo/src/main.ts'],
    'even a same-file jump asks the service, which owns reservation and focus',
  );
});

check('an unopened local file is opened through the service, not by this module', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 2, 4)]) });
  h.setOpen(() => ({ status: 'opened', pane: h.addPane('/repo/src/fmt.ts', TARGET_TEXT), revealed: true }));
  assert.strictEqual(await h.navigation.goToDefinition(h.origin.view, 45), 'navigated');
  assert.strictEqual(h.opens.length, 1);
  const source = fs.readFileSync(NAVIGATION, 'utf8');
  assert.ok(!/editor_read_file|__termlabCreateEditorTab/.test(source), 'it opens nothing itself');
});

check('an owner in another window ends the jump quietly and records no history', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 2, 4)]) });
  h.setOpen(() => ({ status: 'ownerElsewhere' }));
  const status = await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(status, 'elsewhere');
  assert.deepStrictEqual(h.history().back, [], 'this window did not move, so Back has nothing to undo');
  assert.deepStrictEqual(h.toasts, [], 'focusing the real owner is a success, not an error');
});

check('no result reports a status and leaves the editor alone', async () => {
  const h = harness({ respond: () => definitionResponse([]) });
  const status = await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(status, 'none');
  assert.strictEqual(h.opens.length, 0);
  assert.deepStrictEqual(h.history().back, []);
  assert.strictEqual(h.toasts.length, 1, 'a non-blocking status, not silence');
  assert.strictEqual(h.toasts[0][0], 'info');
});

check('a null response is a no-result, not a crash', async () => {
  const h = harness({ respond: () => null });
  assert.strictEqual(await h.navigation.goToDefinition(h.origin.view, 45), 'none');
});

check('a response for another document is rejected', async () => {
  const h = harness({
    respond: () => definitionResponse([location(FMT_URI, 2, 4)], 'doc-other'),
  });
  assert.strictEqual(await h.navigation.goToDefinition(h.origin.view, 45), 'none');
  assert.strictEqual(h.opens.length, 0);
});

check('a pane whose session cannot do definitions makes no request', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 2, 4)]) });
  h.states.get(h.origin).capabilities = { ...CAPABILITIES, definition: false };
  assert.strictEqual(await h.navigation.goToDefinition(h.origin.view, 45), 'unavailable');
  assert.strictEqual(h.requests.length, 0);
});

check('a remote pane is never asked for a definition', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 2, 4)]) });
  h.origin.remote = { host: 'example' };
  assert.strictEqual(await h.navigation.goToDefinition(h.origin.view, 45), 'unavailable');
  assert.strictEqual(h.requests.length, 0);
});

// --- URI rejection -----------------------------------------------------------------

check('non-file URIs are dropped, and a jump with nothing left reports it', async () => {
  const h = harness({
    respond: () => definitionResponse([
      location('untitled:Untitled-1', 0, 0),
      location('jdt://contents/rt.jar', 1, 2),
      location('https://example.com/a.ts', 0, 0),
    ]),
  });
  const status = await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(status, 'unsupported');
  assert.strictEqual(h.opens.length, 0, 'nothing outside file: is opened');
  assert.strictEqual(h.toasts.length, 1);
  assert.strictEqual(h.toasts[0][0], 'info');
});

check('a mixed result keeps only the file targets', async () => {
  const h = harness({
    respond: () => definitionResponse([
      location('untitled:Untitled-1', 0, 0),
      location(FMT_URI, 2, 4),
    ]),
  });
  h.setOpen(() => ({ status: 'opened', pane: h.addPane('/repo/src/fmt.ts', TARGET_TEXT), revealed: true }));
  assert.strictEqual(
    await h.navigation.goToDefinition(h.origin.view, 45), 'navigated',
    'one survivor is a single result, so it opens without a chooser',
  );
  assert.deepStrictEqual(h.opens.map((entry) => entry.filePath), ['/repo/src/fmt.ts']);
});

check('URI conversion is lsp-uri’s job, in one place', () => {
  const source = fs.readFileSync(NAVIGATION, 'utf8');
  assert.ok(/termlabLspUri/.test(source), 'it uses the shared converter');
  assert.ok(!/decodeURIComponent|encodeURI\(/.test(source), 'and never re-implements it');
});

// --- the chooser --------------------------------------------------------------------

const TWO_TARGETS = [location(FMT_URI, 0, 16, 22), location('file:///repo/src/fmt.d.ts', 4, 2, 8)];

check('multiple definitions open a chooser and open nothing', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  const status = await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(status, 'chooser');
  assert.strictEqual(h.opens.length, 0, 'never open all results');
  const chooser = h.chooser();
  assert.strictEqual(chooser.open, true);
  assert.strictEqual(chooser.index, 0);
  assert.strictEqual(chooser.items.length, 2);
  assert.strictEqual(chooser.items[0].name, 'fmt.ts');
  assert.strictEqual(chooser.items[0].line, 1, 'lines are shown 1-based');
  assert.strictEqual(chooser.items[1].name, 'fmt.d.ts');
  assert.strictEqual(chooser.items[1].line, 5);
});

check('the chooser is anchored at the position the definition was requested for', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  await h.navigation.goToDefinition(h.origin.view, 45);
  const tooltips = h.origin.view.state.facet(REAL.view.showTooltip).filter(Boolean);
  assert.strictEqual(tooltips.length, 1, 'exactly one overlay');
  assert.strictEqual(tooltips[0].pos, 45, 'near the cursor, not pinned to a corner');
  assert.strictEqual(typeof tooltips[0].create, 'function');
});

check('the chooser previews the target line when that file is open here', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  await h.navigation.goToDefinition(h.origin.view, 45);
  const items = h.chooser().items;
  assert.strictEqual(items[0].preview, 'export function format(value: number): string {');
  assert.strictEqual(items[1].preview, null, 'a file this window has not opened has no preview');
  assert.strictEqual(items[1].context, '/repo/src', 'so the row shows where the file lives instead');
});

check('arrow keys move the selection and are consumed', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(h.navigation.handleKeydown({ key: 'ArrowDown' }, h.origin.view), true);
  assert.strictEqual(h.chooser().index, 1);
  assert.strictEqual(h.navigation.handleKeydown({ key: 'ArrowDown' }, h.origin.view), true);
  assert.strictEqual(h.chooser().index, 0, 'the list wraps');
  assert.strictEqual(h.navigation.handleKeydown({ key: 'ArrowUp' }, h.origin.view), true);
  assert.strictEqual(h.chooser().index, 1, 'and wraps backwards too');
});

check('Enter opens the highlighted target and closes the chooser', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  const target = h.addPane('/repo/src/fmt.d.ts', 'declare function format(v: number): string;\n');
  h.setOpen(() => ({ status: 'opened', pane: target, revealed: true }));
  await h.navigation.goToDefinition(h.origin.view, 45);
  h.navigation.handleKeydown({ key: 'ArrowDown' }, h.origin.view);
  assert.strictEqual(h.navigation.handleKeydown({ key: 'Enter' }, h.origin.view), true);
  await sleep(5);
  assert.strictEqual(h.chooser().open, false);
  assert.deepStrictEqual(h.opens.map((entry) => entry.filePath), ['/repo/src/fmt.d.ts']);
  assert.strictEqual(h.history().back.length, 1, 'choosing records the jump');
});

check('Escape closes the chooser without navigating', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(h.navigation.handleKeydown({ key: 'Escape' }, h.origin.view), true);
  assert.strictEqual(h.chooser().open, false);
  assert.strictEqual(h.opens.length, 0);
  assert.deepStrictEqual(h.history().back, [], 'a cancelled chooser is not a jump');
});

check('with no chooser open the keys fall through, so vim keeps its arrows and Escape', () => {
  const h = harness();
  for (const key of ['ArrowDown', 'ArrowUp', 'Enter', 'Escape']) {
    assert.strictEqual(h.navigation.handleKeydown({ key }, h.origin.view), false, key);
  }
});

check('a modified arrow is not the chooser’s', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(
    h.navigation.handleKeydown({ key: 'ArrowDown', metaKey: true }, h.origin.view), false,
  );
  assert.strictEqual(h.chooser().index, 0);
});

check('an edit closes the chooser, because its origin position no longer holds', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  await h.navigation.goToDefinition(h.origin.view, 45);
  h.origin.view.dispatch({ changes: { from: 0, insert: 'x' }, userEvent: 'input.type' });
  assert.strictEqual(h.chooser().open, false);
  assert.strictEqual(
    h.origin.view.state.facet(REAL.view.showTooltip).filter(Boolean).length, 0,
    'the field drops it, so no stale overlay is left behind',
  );
});

check('opening the chooser dismisses an open hover or signature overlay', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  const dismissed = [];
  h.sandbox.termlabLspTooltips = { dismiss: (view) => dismissed.push(view === h.origin.view) };
  await h.navigation.goToDefinition(h.origin.view, 45);
  assert.deepStrictEqual(dismissed, [true], 'one overlay at a time');
});

check('the chooser renders file, line and preview as text, never as markup', () => {
  const h = harness();
  const node = h.navigation.renderChooser({
    index: 1,
    items: [
      {
        name: 'fmt.ts', line: 1, column: 17, preview: 'export function format() {}', context: '/repo/src',
      },
      {
        name: '<img src=x onerror="steal()">.ts',
        line: 5,
        column: 3,
        preview: null,
        context: '/repo/<script>',
      },
    ],
  });
  const all = flatten(node);
  assert.ok(all.every((child) => child.innerHTML === undefined), 'nothing set innerHTML');
  assert.ok(!all.some((child) => child.tagName === 'IMG' || child.tagName === 'SCRIPT'));
  const text = textOf(node);
  assert.ok(text.indexOf('fmt.ts') >= 0 && text.indexOf('1') >= 0);
  assert.ok(text.indexOf('export function format() {}') >= 0, 'the preview is shown');
  assert.ok(text.indexOf('onerror') >= 0, 'a hostile name survives as literal text');
  const active = all.filter(
    (child) => String(child.className).indexOf('tl-definition-chooser__item--active') >= 0,
  );
  assert.strictEqual(active.length, 1, 'exactly one row is highlighted');
  assert.ok(String(active[0]['attr:aria-selected']) === 'true', 'and it says so to a screen reader');
});

check('clicking a row chooses it', async () => {
  const h = harness({ respond: () => definitionResponse(TWO_TARGETS) });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen(() => ({ status: 'opened', pane: target, revealed: true }));
  await h.navigation.goToDefinition(h.origin.view, 45);
  const tooltip = h.origin.view.state.facet(REAL.view.showTooltip).filter(Boolean)[0];
  // CodeMirror hands `create` the view; the chooser needs it to know which
  // editor's list is being clicked.
  const dom = tooltip.create(h.origin.view).dom;
  const rows = flatten(dom).filter(
    (child) => String(child.className).indexOf('tl-definition-chooser__item') >= 0,
  );
  assert.strictEqual(rows.length, 2);
  let defaultPrevented = false;
  rows[0].fire('mousedown', { preventDefault() { defaultPrevented = true; }, button: 0 });
  await sleep(5);
  assert.strictEqual(defaultPrevented, true, 'the click must not steal focus from the editor');
  assert.deepStrictEqual(h.opens.map((entry) => entry.filePath), ['/repo/src/fmt.ts']);
});

// --- history ---------------------------------------------------------------------------

check('the source location is captured before navigation, not after it', async () => {
  let resolveRequest = null;
  const h = harness({
    respond: () => new Promise((resolve) => { resolveRequest = resolve; }),
  });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen(() => ({ status: 'opened', pane: target, revealed: true }));
  const jump = h.navigation.goToDefinition(h.origin.view, 45);
  // The caret moves while the request is in flight; Back must return to where
  // F12 was pressed.
  h.origin.view.dispatch({ selection: { anchor: 3 } });
  resolveRequest(definitionResponse([location(FMT_URI, 0, 16, 22)]));
  await jump;
  const back = h.history().back;
  assert.strictEqual(back.length, 1);
  assert.strictEqual(back[0].uri, 'file:///repo/src/main.ts');
  assert.deepStrictEqual(back[0].position, { line: 1, character: 13 });
  assert.deepStrictEqual(back[0].range, {
    start: { line: 1, character: 13 }, end: { line: 1, character: 13 },
  }, 'the selection at the source, so Back restores it');
  assert.deepStrictEqual(back[0].owner, { windowLabel: 'main', paneId: '1' });
});

check('back returns to the source and forward returns to the target', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen((filePath) => ({
    status: 'focused',
    pane: filePath === '/repo/src/fmt.ts' ? target : h.origin,
    revealed: true,
  }));
  await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(h.history().back.length, 1);

  assert.strictEqual(await h.navigation.navigateBack(), 'navigated');
  assert.strictEqual(h.opens[1].filePath, '/repo/src/main.ts');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.opens[1].range)),
    { start: { line: 1, character: 13 }, end: { line: 1, character: 13 } },
    'Back restores the caret, not just the file',
  );
  let state = h.history();
  assert.strictEqual(state.back.length, 0);
  assert.strictEqual(state.forward.length, 1, 'the place Back left becomes Forward');
  assert.strictEqual(state.forward[0].uri, FMT_URI);

  assert.strictEqual(await h.navigation.navigateForward(), 'navigated');
  assert.strictEqual(h.opens[2].filePath, '/repo/src/fmt.ts');
  state = h.history();
  assert.strictEqual(state.back.length, 1);
  assert.strictEqual(state.forward.length, 0);
});

check('back with nothing behind reports it and opens nothing', async () => {
  const h = harness();
  assert.strictEqual(await h.navigation.navigateBack(), 'none');
  assert.strictEqual(await h.navigation.navigateForward(), 'none');
  assert.strictEqual(h.opens.length, 0);
});

check('a new jump after going back truncates the forward stack', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  const fmt = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  const other = h.addPane('/repo/src/other.ts', 'export const other = 1;\n');
  h.setOpen((filePath) => ({
    status: 'focused',
    pane: { '/repo/src/fmt.ts': fmt, '/repo/src/other.ts': other }[filePath] || h.origin,
    revealed: true,
  }));
  await h.navigation.goToDefinition(h.origin.view, 45);
  await h.navigation.navigateBack();
  assert.strictEqual(h.history().forward.length, 1);

  h.setResponder(() => definitionResponse([location('file:///repo/src/other.ts', 0, 13, 18)]));
  await h.navigation.goToDefinition(h.origin.view, 45);
  const state = h.history();
  assert.deepStrictEqual(state.forward, [], 'a new branch discards the abandoned one');
  assert.strictEqual(state.back.length, 1);
});

check('the history is bounded at 100 entries per window', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen(() => ({ status: 'focused', pane: target, revealed: true }));
  for (let i = 0; i < 105; i += 1) {
    // Each jump starts from the origin pane again, so each one records a source.
    await h.navigation.goToDefinition(h.origin.view, 45);
  }
  const state = h.history();
  assert.strictEqual(state.back.length, 100, 'bounded, so a long session cannot grow without limit');
});

check('a target that disappeared reports a status and leaves the history untouched', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen(() => ({ status: 'focused', pane: target, revealed: true }));
  await h.navigation.goToDefinition(h.origin.view, 45);
  const before = h.history();

  h.setOpen(() => ({ status: 'failed', error: new Error('No such file or directory') }));
  assert.strictEqual(await h.navigation.navigateBack(), 'failed');
  assert.deepStrictEqual(
    h.history(), before,
    'a failed Back must not consume the entry it could not reach',
  );
  assert.strictEqual(h.toasts.length, 1);
  assert.strictEqual(h.toasts[0][0], 'info', 'quiet: the current editor did not change');
});

// A document can move windows under a history entry (Save As, or an owner in a
// window opened later). Focusing that window IS the navigation the user asked
// for, so the entry has to be consumed like any other — an entry that stays on
// top forever puts every older entry permanently out of reach.
check('Back through documents that moved to another window still drains the stack', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen(() => ({ status: 'focused', pane: target, revealed: true }));
  await h.navigation.goToDefinition(h.origin.view, 45);
  await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(h.history().back.length, 2);

  h.setOpen(() => ({ status: 'ownerElsewhere' }));
  assert.strictEqual(await h.navigation.navigateBack(), 'elsewhere');
  assert.strictEqual(h.history().back.length, 1, 'the focused owner consumed the entry');
  assert.strictEqual(await h.navigation.navigateBack(), 'elsewhere');
  assert.strictEqual(h.history().back.length, 0, 'so the next Back reaches the older entry');
  assert.strictEqual(h.history().forward.length, 2, 'and Forward can retrace both');
  assert.strictEqual(await h.navigation.navigateBack(), 'none');
});

check('a jump that opened nothing to select is not recorded as history', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  // The degraded path: an open that produced no pane to select a range in.
  h.setOpen(() => ({ status: 'opened', pane: null }));
  assert.strictEqual(await h.navigation.goToDefinition(h.origin.view, 45), 'unrevealed');
  assert.deepStrictEqual(h.history().back, [], 'Back would have nowhere to return from');
  assert.deepStrictEqual(h.toasts, [], 'and the file did open, so there is nothing to report');
});

check('a definition jump to a file that disappeared records nothing', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  h.setOpen(() => ({ status: 'failed', error: new Error('gone') }));
  assert.strictEqual(await h.navigation.goToDefinition(h.origin.view, 45), 'failed');
  assert.deepStrictEqual(h.history().back, []);
  assert.strictEqual(h.toasts.length, 1);
});

// --- shortcuts ---------------------------------------------------------------------

check('the configured shortcuts arrive as window events', async () => {
  const h = harness({ respond: () => definitionResponse([]) });
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-go-to-definition'));
  await sleep(5);
  assert.strictEqual(h.requests.length, 1, 'F12 requests at the caret of the focused pane');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.requests[0].position)), { line: 0, character: 0 },
  );
  for (const name of ['termlab:editor-navigate-back', 'termlab:editor-navigate-forward']) {
    h.sandbox.dispatchEvent(new h.sandbox.CustomEvent(name));
  }
  await sleep(5);
  assert.ok(
    h.windowListeners.has('termlab:editor-navigate-back')
    && h.windowListeners.has('termlab:editor-navigate-forward'),
    'Ctrl-minus and Ctrl-Shift-minus are routed by the shortcut runtime, not by a key handler here',
  );
});

check('dispose removes the window listeners', async () => {
  const h = harness({ respond: () => definitionResponse([]) });
  h.navigation.dispose();
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-go-to-definition'));
  await sleep(5);
  assert.strictEqual(h.requests.length, 0);
});

check('Command-click requests a definition at the clicked position', async () => {
  const h = harness({ respond: () => definitionResponse([]) });
  const consumed = h.navigation.handleMousedown(
    { metaKey: true, button: 0, clientX: 45, clientY: 8 }, h.origin.view,
  );
  await sleep(5);
  assert.strictEqual(consumed, false, 'the click still places the caret; it is not swallowed');
  assert.strictEqual(h.requests.length, 1);
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.requests[0].position)), { line: 1, character: 13 },
  );
});

check('a plain click, a right click and a Ctrl-click are not Command-click', async () => {
  const h = harness({ respond: () => definitionResponse([]) });
  h.navigation.handleMousedown({ button: 0, clientX: 45, clientY: 8 }, h.origin.view);
  h.navigation.handleMousedown({ metaKey: true, button: 2, clientX: 45, clientY: 8 }, h.origin.view);
  h.navigation.handleMousedown({ ctrlKey: true, button: 0, clientX: 45, clientY: 8 }, h.origin.view);
  await sleep(5);
  assert.strictEqual(h.requests.length, 0);
});

// --- the REAL half ------------------------------------------------------------------

check('the chooser keydown handler is raised above vim, which is a plugin not a keymap', () => {
  const h = harness();
  const state = REAL.state.EditorState.create({
    doc: 'x',
    extensions: [REAL.vim.vim(), h.navigation.extensions()],
  });
  const owners = keydownOwners(state);
  assert.ok(owners.length >= 2, 'both vim and the chooser handler registered a keydown');
  const vimOnly = keydownOwners(REAL.state.EditorState.create({
    doc: 'x', extensions: [REAL.vim.vim()],
  }));
  const vimId = vimOnly[vimOnly.length - 1];
  assert.ok(
    owners.indexOf(vimId) > 0,
    'Enter and the arrows reach the open chooser before vim moves the caret',
  );
});

check('extensions() is stable, so mounting twice does not install two fields', () => {
  const h = harness();
  const state = REAL.state.EditorState.create({
    doc: 'x',
    extensions: [h.navigation.extensions(), h.navigation.extensions()],
  });
  assert.strictEqual(state.facet(REAL.view.showTooltip).filter(Boolean).length, 0);
  assert.ok(state, 'two mounts of the same field must not throw');
});

check('extensions() is empty when the bundle lacks the tooltip exports', () => {
  const { sandbox } = load([URI, POSITION].concat(NAVIGATION_MODULES), null, { EditorState: REAL.state.EditorState });
  assert.strictEqual(sandbox.termlabLspNavigation.extensions().length, 0);
});

// The one-overlay rule has to hold in both directions. Opening the chooser
// dismisses hover and signature help; this is the other half — while the
// chooser is up, the pointer dwell must not render a hover box on top of it.
check('an open chooser stands down the hover dwell, so only one overlay is on screen', async () => {
  const { sandbox } = load([URI, POSITION, TOOLTIPS].concat(NAVIGATION_MODULES));
  const navigation = sandbox.termlabLspNavigation;
  const tooltips = sandbox.termlabLspTooltips;
  const view = {
    state: REAL.state.EditorState.create({
      doc: ORIGIN_TEXT,
      extensions: [tooltips.extensions(), navigation.extensions()],
    }),
    dispatch(spec) { this.state = this.state.update(spec).state; },
    posAtCoords: (coords) => coords.x,
    focus() {},
    hasFocus: true,
  };
  const pane = {
    paneId: 1, tabId: 101, kind: 'editor', view, filePath: '/repo/src/main.ts', remote: null,
  };
  const panes = new Map([[1, pane]]);
  sandbox.termlabLspState = {
    get: (candidate) => (candidate === pane ? {
      documentId: 'doc-1',
      version: 3,
      capabilities: { ...CAPABILITIES },
      status: { revision: 2, state: 'ready', capabilities: CAPABILITIES },
    } : null),
  };
  sandbox.termlabEditorService = {
    openLocalFileAt: () => Promise.resolve({ status: 'focused', pane, revealed: true }),
  };
  const lookups = {
    paneForView: (candidate) => (candidate === view ? pane : null),
    currentPane: () => pane,
  };
  const hoverRequests = [];
  tooltips.configure({
    ...lookups,
    hoverDelayMs: 5,
    requestFeature: (target, kind, position) => {
      hoverRequests.push({ kind, position });
      return Promise.resolve({
        documentId: 'doc-1',
        sourceVersion: 3,
        range: null,
        blocks: [{ markdown: false, value: 'string' }],
      });
    },
  });
  navigation.configure({
    ...lookups,
    allPanes: () => panes,
    windowLabel: 'main',
    requestFeature: () => Promise.resolve(definitionResponse(TWO_TARGETS)),
  });

  assert.strictEqual(await navigation.goToDefinition(view, 45), 'chooser');
  tooltips.handlePointerMove({ clientX: 16, clientY: 0 }, view);
  await sleep(30);
  assert.deepStrictEqual(hoverRequests, [], 'the dwell stood down while the chooser was open');
  assert.strictEqual(
    view.state.facet(REAL.view.showTooltip).filter(Boolean).length, 1,
    'one overlay, and it is the chooser',
  );
  assert.strictEqual(
    await tooltips.showHover(view), false,
    'an explicit Show Hover stands down too; Escape closes the chooser first',
  );

  navigation.closeChooser(view);
  tooltips.handlePointerMove({ clientX: 16, clientY: 0 }, view);
  await sleep(30);
  assert.strictEqual(hoverRequests.length, 1, 'and hover is available again the moment it closes');
});

// --- the vim keys, end to end -------------------------------------------------
//
// The owner's case: `gd` into another file, then Ctrl-O back to exactly where
// the cursor was. Driven through the REAL vim engine and the REAL history
// module, because the bug this replaces was precisely that vim's own jumplist
// cannot cross files.

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
    getLine: () => 'const value = format(1);',
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

// Load vim-mode into an existing navigation harness and wire it to the real
// navigator, the way manager-compose-runtime does.
function withVim(h) {
  h.sandbox.CM6.Vim = REAL.vim.Vim;
  vm.runInContext(fs.readFileSync(VIM_MODE, 'utf8'), h.sandbox, { filename: VIM_MODE });
  h.sandbox.termlabVimMode.registerNavigationCommands({
    goToDefinition: (view) => h.navigation.goToDefinition(view),
    navigateBack: () => h.navigation.navigateBack(),
    navigateForward: () => h.navigation.navigateForward(),
    recordJump: (view, position) => h.navigation.recordJump(view, position),
  });
  return (keys, view) => {
    const adapter = vimAdapter(view);
    let command;
    for (const key of keys) command = REAL.vim.Vim.findKey(adapter, key, 'test');
    if (typeof command === 'function') command();
    return adapter;
  };
}

check('gd into another file, then Ctrl-O, returns to the exact pre-jump position', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen((filePath) => ({
    status: 'focused',
    pane: filePath === '/repo/src/fmt.ts' ? target : h.origin,
    revealed: true,
  }));
  const press = withVim(h);
  // The caret sits inside `format(1)` on line 2 of the origin file.
  h.origin.view.dispatch({ selection: { anchor: 45 } });

  press(['g', 'd'], h.origin.view);
  await sleep(10);
  assert.deepStrictEqual(
    h.opens.map((entry) => entry.filePath), ['/repo/src/fmt.ts'],
    'gd opened the definition\'s file',
  );

  press(['<C-o>'], target.view);
  await sleep(10);
  assert.strictEqual(h.opens.length, 2, 'Ctrl-O went somewhere');
  assert.strictEqual(h.opens[1].filePath, '/repo/src/main.ts', 'back to the file gd left');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.opens[1].range)),
    { start: { line: 1, character: 13 }, end: { line: 1, character: 13 } },
    'and to the exact cursor position it left from',
  );

  press(['<C-i>'], h.origin.view);
  await sleep(10);
  assert.strictEqual(h.opens[2].filePath, '/repo/src/fmt.ts', 'Ctrl-I re-jumps');
});

check('a vim jump motion inside a file joins the same trail', async () => {
  const h = harness();
  const press = withVim(h);
  h.setOpen(() => ({ status: 'focused', pane: h.origin, revealed: true }));
  press(['G'], h.origin.view);
  const back = h.history().back;
  assert.strictEqual(back.length, 1, 'G recorded where it jumped from');
  assert.strictEqual(back[0].uri, 'file:///repo/src/main.ts');

  await h.navigation.navigateBack();
  assert.strictEqual(h.opens.length, 1, 'Ctrl-O has something in-file to return to');
  assert.strictEqual(h.opens[0].filePath, '/repo/src/main.ts');
});

check('recordJump collapses on the position vim reported, not the live selection', () => {
  const h = harness();
  assert.strictEqual(h.navigation.recordJump(h.origin.view, { line: 1, character: 13 }), true);
  const entry = h.history().back[0];
  assert.deepStrictEqual(entry.position, { line: 1, character: 13 });
  assert.deepStrictEqual(entry.range, {
    start: { line: 1, character: 13 }, end: { line: 1, character: 13 },
  });
});

check('a jump recorded from an untitled buffer is not history', () => {
  const h = harness({ originPath: null });
  assert.strictEqual(h.navigation.recordJump(h.origin.view, { line: 0, character: 0 }), false);
  assert.deepStrictEqual(h.history().back, [], 'there is no URI to come back to');
});

// --- plain file switches ------------------------------------------------------
//
// Opening a file by hand — the explorer, the palette, a tab click, `termlab
// msg` from the CLI — is a jump in vim's sense (`:e` and `:b` go on the
// jumplist), so Ctrl-O has to come back from it. Only a change of DOCUMENT
// counts, and a switch this module caused itself must not be recorded twice
// nor truncate the forward stack.

check('switching to another file records where the last editor was left', async () => {
  const h = harness();
  const other = h.addPane('/repo/src/other.ts', 'export const other = 1;\n');
  h.setOpen((filePath) => ({
    status: 'focused',
    pane: filePath === '/repo/src/other.ts' ? other : h.origin,
    revealed: true,
  }));
  h.origin.view.dispatch({ selection: { anchor: 45 } });
  h.focus(h.origin);
  h.focus(other);

  const back = h.history().back;
  assert.strictEqual(back.length, 1, 'the switch is a jump');
  assert.strictEqual(back[0].uri, 'file:///repo/src/main.ts');
  assert.deepStrictEqual(back[0].position, { line: 1, character: 13 }, 'at the caret it left');

  assert.strictEqual(await h.navigation.navigateBack(), 'navigated');
  assert.strictEqual(h.opens[0].filePath, '/repo/src/main.ts');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.opens[0].range)),
    { start: { line: 1, character: 13 }, end: { line: 1, character: 13 } },
  );
  assert.strictEqual(await h.navigation.navigateForward(), 'navigated');
  assert.strictEqual(h.opens[1].filePath, '/repo/src/other.ts', 'Ctrl-I returns to the file left');
});

check('walking the trail neither appends entries nor truncates the way forward', async () => {
  const h = harness();
  const other = h.addPane('/repo/src/other.ts', 'export const other = 1;\n');
  const paneFor = (filePath) => (filePath === '/repo/src/other.ts' ? other : h.origin);
  // The service focuses the pane it opened, exactly as the app does — which is
  // what would feed a second entry back in if walking recorded switches.
  h.setOpen((filePath) => {
    const pane = paneFor(filePath);
    h.focus(pane);
    return { status: 'focused', pane, revealed: true };
  });
  h.focus(h.origin);
  h.focus(other);
  assert.strictEqual(h.history().back.length, 1);

  await h.navigation.navigateBack();
  let state = h.history();
  assert.strictEqual(state.back.length, 0, 'the walk consumed the entry and added none');
  assert.strictEqual(state.forward.length, 1, 'and Ctrl-I still has somewhere to go');

  await h.navigation.navigateForward();
  state = h.history();
  assert.strictEqual(state.back.length, 1);
  assert.strictEqual(state.forward.length, 0);
});

check('a definition jump records exactly one entry, not one per surface', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen(() => {
    // The open focuses the target pane; the switch listener sees it too.
    h.focus(target);
    return { status: 'focused', pane: target, revealed: true };
  });
  h.focus(h.origin);
  await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(h.history().back.length, 1, 'gd records its origin once');
});

check('a reveal inside the same document is not a jump', () => {
  const h = harness();
  const twin = h.addPane('/repo/src/main.ts', ORIGIN_TEXT);
  h.focus(h.origin);
  assert.strictEqual(h.focus(twin), false);
  assert.deepStrictEqual(h.history().back, [], 'two panes on one file are one document');
});

check('a detour through a terminal does not lose the jump', () => {
  const h = harness();
  const terminal = { paneId: 99, tabId: 199, kind: 'terminal' };
  const other = h.addPane('/repo/src/other.ts', 'export const other = 1;\n');
  h.focus(h.origin);
  assert.strictEqual(h.focus(terminal), false);
  assert.strictEqual(h.focus(other), true);
  const back = h.history().back;
  assert.strictEqual(back.length, 1, 'the jump is editor-to-editor, whatever was focused between');
  assert.strictEqual(back[0].uri, 'file:///repo/src/main.ts');
});

check('an untitled or remote buffer contributes no entry', () => {
  const h = harness();
  const untitled = h.addPane(null, '');
  const other = h.addPane('/repo/src/other.ts', 'export const other = 1;\n');
  h.focus(untitled);
  assert.strictEqual(h.focus(other), false);
  assert.deepStrictEqual(h.history().back, [], 'there is no URI to come back to');
});

check('the same switch notified twice records once', () => {
  const h = harness();
  const other = h.addPane('/repo/src/other.ts', 'export const other = 1;');
  h.focus(h.origin);
  assert.strictEqual(h.focus(other), true);
  assert.strictEqual(h.focus(other), false, 'the second notification is not a second jump');
  assert.strictEqual(h.history().back.length, 1);
});

check('a focus notification that lands after a jump does not stack it twice', async () => {
  const h = harness({ respond: () => definitionResponse([location(FMT_URI, 0, 16, 22)]) });
  const target = h.addPane('/repo/src/fmt.ts', TARGET_TEXT);
  h.setOpen(() => ({ status: 'focused', pane: target, revealed: true }));
  h.focus(h.origin);
  h.origin.view.dispatch({ selection: { anchor: 45 } });
  await h.navigation.goToDefinition(h.origin.view, 45);
  assert.strictEqual(h.history().back.length, 1, 'the jump recorded its origin');
  // The focus change arrives late, after the jump has already resolved.
  assert.strictEqual(
    h.navigation.noteFocusedPaneChanged(h.origin, target), false,
    'the trail already holds that exact location',
  );
  assert.strictEqual(h.history().back.length, 1);
});

check('pane-manager notifies on every focus change, which is the one choke point', () => {
  const { sandbox } = load([], null, null);
  vm.runInContext(fs.readFileSync(PANE_MANAGER, 'utf8'), sandbox, { filename: PANE_MANAGER });
  const panes = new Map();
  const tabs = new Map();
  const make = (paneId) => {
    const pane = {
      paneId,
      tabId: 1,
      kind: 'editor',
      root: { classList: { add() {}, remove() {} } },
      view: { focus() {} },
    };
    panes.set(paneId, pane);
    return pane;
  };
  const first = make(1);
  const second = make(2);
  tabs.set(1, { id: 1, focusedPaneId: 1 });
  let focusedPaneId = null;
  const changes = [];
  const manager = sandbox.termlabPaneManager.create({
    getPanes: () => panes,
    getTabs: () => tabs,
    getFocusedPaneId: () => focusedPaneId,
    setFocusedPaneId: (id) => { focusedPaneId = id; },
    onFocusedPaneChanged: (previous, next) => { changes.push([previous, next]); },
  });
  manager.setFocusedPane(1);
  manager.setFocusedPane(2);
  manager.setFocusedPane(2);
  assert.strictEqual(changes.length, 2, 'refocusing the same pane is not a change');
  assert.deepStrictEqual(changes[0], [null, first]);
  assert.deepStrictEqual(changes[1], [first, second]);
});

check('the compose runtime feeds that notification to the navigator', () => {
  const source = fs.readFileSync(COMPOSE, 'utf8');
  assert.ok(/onFocusedPaneChanged/.test(source), 'the hook is wired at composition');
  const at = source.indexOf('onFocusedPaneChanged');
  const block = source.slice(at, at + 600);
  assert.ok(/noteFocusedPaneChanged/.test(block), 'and lands on the navigator');
});

// --- the keys that reach this module ------------------------------------------
//
// The controller listens for `termlab:editor-*` window events, which is only
// half an answer: something has to DISPATCH them. This drives the real
// shortcut runtime with the real shipped defaults, so "what do I press today"
// has a test rather than an assumption.

function keyboardDefault(field) {
  const source = fs.readFileSync(KEYBOARD_DEFAULTS, 'utf8');
  const match = new RegExp(`${field}: "([^"]+)"`).exec(source);
  return match ? match[1] : null;
}

function shortcutHarness(pane) {
  const sandbox = { console, document: { activeElement: null } };
  sandbox.window = sandbox;
  const dispatched = [];
  sandbox.CustomEvent = class CustomEvent {
    constructor(type, init) { this.type = type; this.detail = init && init.detail; }
  };
  sandbox.dispatchEvent = (event) => { dispatched.push(event.type); return true; };
  vm.createContext(sandbox);
  for (const file of [INPUT_RUNTIME, SHORTCUT_RUNTIME]) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  const keyboard = {
    editor_go_to_definition: keyboardDefault('editor_go_to_definition'),
    editor_navigate_back: keyboardDefault('editor_navigate_back'),
    editor_navigate_forward: keyboardDefault('editor_navigate_forward'),
  };
  const handlers = new Map();
  sandbox.termlabKeyboardRouter = { register: (handler) => handlers.set(handler.name, handler) };
  const runtime = sandbox.termlabShortcutRuntime.create({
    invoke: async (command) => {
      if (command === 'get_all_settings') return { termlab: { keyboard } };
      if (command === 'get_plugin_menu_items') return [];
      return null;
    },
    isMacPlatform: true,
    isTextInputTarget: sandbox.termlabInputRuntime.create().isTextInputTarget,
    handleMenuAction: () => {},
    shouldDebugKeyEvent: () => false,
    formatKeyEventForDebug: () => '',
    shortcutDebugEnabled: false,
    openCommandPalette: () => {},
    closeCommandPalette: () => {},
    isCommandPaletteOpen: () => false,
    getTabIds: () => [],
    activateTab: () => {},
    getCurrentPane: () => pane,
    writeTextToCurrentPane: () => {},
    getActiveTab: () => null,
    getFocusedPaneId: () => null,
    setFocusedPane: () => {},
    findAdjacentPane: () => null,
  });
  return {
    runtime,
    keyboard,
    dispatched,
    press: (event) => handlers.get('shortcut-fallbacks').onKeyDown({
      metaKey: false,
      ctrlKey: false,
      altKey: false,
      shiftKey: false,
      target: { tagName: 'DIV', className: 'cm-content' },
      ...event,
    }),
  };
}

check('the shipped defaults bind F12, Ctrl-minus and Ctrl-Shift-minus', () => {
  assert.strictEqual(keyboardDefault('editor_go_to_definition'), 'f12');
  assert.strictEqual(keyboardDefault('editor_navigate_back'), 'ctrl+-');
  assert.strictEqual(keyboardDefault('editor_navigate_forward'), 'ctrl+shift+-');
});

check('F12 in an editor pane dispatches the event this module listens for', async () => {
  const h = shortcutHarness({ kind: 'editor' });
  await h.runtime.init();
  const consumed = h.press({ code: 'F12', key: 'F12' });
  assert.strictEqual(consumed, true, 'the keystroke is claimed');
  assert.deepStrictEqual(h.dispatched, ['termlab:editor-go-to-definition']);
});

check('Ctrl-minus and Ctrl-Shift-minus dispatch back and forward', async () => {
  const h = shortcutHarness({ kind: 'editor' });
  await h.runtime.init();
  h.press({ code: 'Minus', key: '-', ctrlKey: true });
  h.press({ code: 'Minus', key: '-', ctrlKey: true, shiftKey: true });
  assert.deepStrictEqual(h.dispatched, [
    'termlab:editor-navigate-back', 'termlab:editor-navigate-forward',
  ]);
});

check('the same keys in a terminal pane are left to the shell', async () => {
  const h = shortcutHarness({ kind: 'terminal' });
  await h.runtime.init();
  h.press({ code: 'F12', key: 'F12' });
  h.press({ code: 'Minus', key: '-', ctrlKey: true });
  assert.deepStrictEqual(h.dispatched, [], 'editor-scoped bindings are dropped, not consumed');
});

// --- the SERVICE half ----------------------------------------------------------------
//
// The real editor-service over a stubbed Tauri client: the navigator's promise
// that "every target routes through the service" is only worth something if the
// service's answers are the ones it handles.

function serviceHarness(options = {}) {
  const calls = [];
  const panes = new Map();
  const focused = [];
  let nextPane = 1;
  let current = options.pane || null;
  const invoke = async (command, args = {}) => {
    calls.push({ command, args });
    if (typeof options.invoke === 'function') {
      const custom = await options.invoke(command, args, calls);
      if (custom !== undefined) return custom;
    }
    if (command === 'editor_reserve_document') {
      return { kind: 'reserved', reservationId: `r-${calls.length}`, canonicalPath: args.path };
    }
    if (command === 'editor_read_file') return options.contents || TARGET_TEXT;
    if (command === 'lsp_open_document') {
      return {
        documentId: `doc-${calls.length}`,
        version: 1,
        projectCandidates: [],
        status: {
          revision: 1,
          documentId: `doc-${calls.length}`,
          state: 'ready',
          capabilities: { ...CAPABILITIES },
          errorCount: 0,
          warningCount: 0,
        },
      };
    }
    return null;
  };

  const { sandbox, toasts } = load([], {
    termlabServices: {
      tauriClient: {
        invoke,
        listen: () => Promise.resolve(() => {}),
        listenOnCurrentWindow: () => Promise.resolve(() => {}),
        currentWindow: { setFocus: async () => { focused.push('window'); } },
      },
    },
  });
  sandbox.__termlabPaneAccess = {
    currentPane: () => current,
    allPanes: () => panes,
    activateTab: (id) => focused.push(`tab:${id}`),
    setFocusedPane: (id) => focused.push(`pane:${id}`),
    setTabLabel: () => true,
  };
  sandbox.__termlabCreateEditorTab = (opts) => {
    const id = nextPane++;
    const created = {
      paneId: id,
      tabId: id + 100,
      kind: 'editor',
      filePath: opts.filePath || null,
      remote: opts.remote || null,
      dirty: false,
      view: {
        state: REAL.state.EditorState.create({ doc: opts.contents || '' }),
        dispatch(spec) { this.state = this.state.update(spec).state; },
        focus() { focused.push(`view:${id}`); },
        termlabResetDirty() {},
        termlabSetReadOnly() {},
      },
    };
    panes.set(id, created);
    current = created;
    if (typeof opts.onPaneCreated === 'function') opts.onPaneCreated(created);
    return created.tabId;
  };
  vm.runInContext(fs.readFileSync(POSITION, 'utf8'), sandbox, { filename: POSITION });
  vm.runInContext(fs.readFileSync(LANGUAGE_MAP, 'utf8'), sandbox, { filename: LANGUAGE_MAP });
  vm.runInContext(fs.readFileSync(TAB_LABEL, 'utf8'), sandbox, { filename: TAB_LABEL });
  vm.runInContext(fs.readFileSync(LSP_STATE, 'utf8'), sandbox, { filename: LSP_STATE });
  vm.runInContext(fs.readFileSync(LSP_BRIDGE, 'utf8'), sandbox, { filename: LSP_BRIDGE });
  sandbox.termlabEditorPane = { setLanguage() {} };
  sandbox.termlabLspBridge.configure({
    windowLabel: 'main', paneAccess: sandbox.__termlabPaneAccess,
  });
  vm.runInContext(fs.readFileSync(EDITOR_SERVICE, 'utf8'), sandbox, { filename: EDITOR_SERVICE });
  return {
    sandbox,
    service: sandbox.termlabEditorService,
    calls,
    panes,
    focused,
    toasts,
    get pane() { return current; },
  };
}

check('the service answers an open with the pane it opened', async () => {
  const h = serviceHarness();
  const result = await h.service.openLocalFile('/repo/src/fmt.ts');
  assert.strictEqual(result.status, 'opened');
  assert.ok(result.pane, 'the caller needs the pane to reveal a range in it');
  assert.strictEqual(result.pane.filePath, '/repo/src/fmt.ts');
});

check('the service answers a second open of the same file with the pane it focused', async () => {
  const h = serviceHarness();
  await h.service.openLocalFile('/repo/src/fmt.ts');
  const before = h.calls.length;
  const result = await h.service.openLocalFile('/repo/src/fmt.ts');
  assert.strictEqual(result.status, 'focused');
  assert.strictEqual(result.pane.filePath, '/repo/src/fmt.ts');
  assert.strictEqual(h.calls.length, before, 'no second reservation and no second read');
});

check('the service answers an owner in another window without opening a second view', async () => {
  const h = serviceHarness({
    invoke(command) {
      if (command === 'editor_reserve_document') {
        return {
          kind: 'focusOwner', documentId: 'doc-owner', windowLabel: 'main-2', paneId: '9',
        };
      }
      return undefined;
    },
  });
  const result = await h.service.openLocalFile('/repo/src/fmt.ts');
  assert.strictEqual(result.status, 'ownerElsewhere');
  assert.strictEqual(result.pane, undefined, 'this window owns no pane for it');
  assert.strictEqual(h.calls.some((entry) => entry.command === 'editor_read_file'), false);
});

check('the service answers a missing file with a failure the caller can act on', async () => {
  const h = serviceHarness({
    invoke(command) {
      if (command === 'editor_read_file') throw new Error('No such file or directory');
      return undefined;
    },
  });
  const result = await h.service.openLocalFile('/repo/src/gone.ts');
  assert.strictEqual(result.status, 'failed');
  assert.ok(result.error, 'and says why');
  assert.strictEqual(
    h.calls.some((entry) => entry.command === 'editor_release_document'), true,
    'the reservation is still released',
  );
});

check('openLocalFileAt selects and centres the range in the pane it opened', async () => {
  const h = serviceHarness();
  const result = await h.service.openLocalFileAt('/repo/src/fmt.ts', {
    start: { line: 0, character: 16 }, end: { line: 0, character: 22 },
  });
  assert.strictEqual(result.status, 'opened');
  assert.strictEqual(result.revealed, true);
  const selection = result.pane.view.state.selection.main;
  assert.strictEqual(selection.from, 16);
  assert.strictEqual(selection.to, 22);
  assert.strictEqual(
    h.focused.some((entry) => String(entry).indexOf('view:') === 0), true,
    'and puts the keyboard back in the editor',
  );
});

check('a range past the end of the document is clamped, not thrown', async () => {
  const h = serviceHarness();
  const result = await h.service.openLocalFileAt('/repo/src/fmt.ts', {
    start: { line: 900, character: 4 }, end: { line: 900, character: 9 },
  });
  assert.strictEqual(result.revealed, true);
  const doc = result.pane.view.state.doc;
  assert.ok(result.pane.view.state.selection.main.to <= doc.length);
});

check('openLocalFileAt on an owner elsewhere reveals nothing and reports it', async () => {
  const h = serviceHarness({
    invoke(command) {
      if (command === 'editor_reserve_document') {
        return { kind: 'focusOwner', documentId: 'd', windowLabel: 'main-2', paneId: '9' };
      }
      return undefined;
    },
  });
  const result = await h.service.openLocalFileAt('/repo/src/fmt.ts', {
    start: { line: 0, character: 0 }, end: { line: 0, character: 1 },
  });
  assert.strictEqual(result.status, 'ownerElsewhere');
  assert.strictEqual(result.revealed, undefined);
});

// --- the payload the frontend reads -----------------------------------------------

// A definition in the standard library or a cargo registry crate is a `file:`
// URI outside every session root. The owner's `gd` lands there constantly, so
// it has to open like any other local file: a new tab, read-write, and NO
// project-root chooser or trust prompt on the way — "editing is immediate
// before any project choice" is the spec's rule, and a std source is exactly
// the file nobody wants to be asked about.
const OUT_OF_PROJECT = '/Users/dev/.rustup/toolchains/stable/lib/rustlib/src/rust/library/std/src/vec.rs';

check('a definition outside every project root opens in a new tab', async () => {
  const h = serviceHarness();
  const before = h.panes.size;
  const result = await h.service.openLocalFileAt(OUT_OF_PROJECT, {
    start: { line: 0, character: 7 }, end: { line: 0, character: 15 },
  });
  assert.strictEqual(result.status, 'opened', 'not focused: no pane held it before');
  assert.strictEqual(h.panes.size, before + 1, 'a new editor tab was created for it');
  assert.strictEqual(result.pane.filePath, OUT_OF_PROJECT);
  assert.strictEqual(result.revealed, true, 'and the caret is on the definition');
  assert.strictEqual(
    result.pane.view.state.readOnly === true, false,
    'it opens read-write, like any other local file',
  );
});

check('opening one asks for no project choice and no trust decision', async () => {
  const h = serviceHarness();
  await h.service.openLocalFileAt(OUT_OF_PROJECT, {
    start: { line: 0, character: 0 }, end: { line: 0, character: 1 },
  });
  const commands = h.calls.map((entry) => entry.command);
  for (const nagging of ['lsp_set_project_context', 'lsp_set_project_trust', 'lsp_project_candidates']) {
    assert.ok(
      commands.indexOf(nagging) < 0,
      `${nagging} fired while merely opening a file outside every root`,
    );
  }
  assert.deepStrictEqual(
    commands.filter((name) => name.indexOf('editor_') === 0 || name.indexOf('lsp_') === 0),
    ['editor_reserve_document', 'editor_read_file', 'lsp_open_document'],
    'reserve, read, commit — the same three calls any local open makes',
  );
});

check('a second gd to the same out-of-project file focuses the tab it already opened', async () => {
  const h = serviceHarness();
  await h.service.openLocalFileAt(OUT_OF_PROJECT, {
    start: { line: 0, character: 0 }, end: { line: 0, character: 1 },
  });
  const opened = h.panes.size;
  const again = await h.service.openLocalFileAt(OUT_OF_PROJECT, {
    start: { line: 2, character: 2 }, end: { line: 2, character: 6 },
  });
  assert.strictEqual(again.status, 'focused');
  assert.strictEqual(h.panes.size, opened, 'no second view of the same bytes');
  assert.strictEqual(again.revealed, true, 'the new range is revealed in the tab that exists');
});

check('Rust normalizes LocationLink to a Location on its target selection range', () => {
  const source = fs.readFileSync(LSP_CLIENT, 'utf8');
  assert.ok(
    /target_selection_range/.test(source),
    'a LocationLink must land on the name, not on the whole declaration body',
  );
});

check('DefinitionResponse carries the document identity the frontend checks', () => {
  const source = fs.readFileSync(LSP_TYPES, 'utf8');
  const at = source.indexOf('pub(crate) struct DefinitionResponse');
  assert.ok(at > 0, 'DefinitionResponse exists');
  const block = source.slice(at, source.indexOf('}', at));
  for (const field of ['document_id', 'source_version', 'locations']) {
    assert.ok(block.includes(field), `DefinitionResponse must carry ${field}`);
  }
});

// --- wiring and hygiene -------------------------------------------------------------

check('index.html loads lsp-navigation.js before editor-pane.js', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(name);
  assert.ok(at('app/features/editor/lsp-navigation.js') > 0, 'the module is loaded at all');
  assert.ok(
    at('app/features/editor/lsp-uri.js') < at('app/features/editor/lsp-navigation.js'),
    'it reads termlabLspUri',
  );
  assert.ok(
    at('app/features/editor/lsp-navigation.js') < at('app/features/editor/editor-pane.js'),
    'editor-pane reads termlabLspNavigation at view-construction time',
  );
});

check('editor-pane mounts the navigation extensions defensively', () => {
  const source = fs.readFileSync(EDITOR_PANE, 'utf8');
  assert.ok(/termlabLspNavigation/.test(source));
  assert.ok(
    /typeof global\.termlabLspNavigation\.extensions === 'function'/.test(source),
    'a stale vendor bundle must cost the chooser and nothing else',
  );
});

check('the compose runtime configures the navigator with the lookups only it has', () => {
  const source = fs.readFileSync(COMPOSE, 'utf8');
  assert.ok(/termlabLspNavigation/.test(source));
  // The configure() call specifically — the module is now named in more than
  // one place in this file (the focus hook is wired separately).
  const at = source.indexOf('termlabLspNavigation.configure');
  assert.ok(at > 0, 'the navigator is configured at composition');
  const block = source.slice(at, at + 900);
  assert.ok(/paneForView/.test(block));
  assert.ok(/currentPane/.test(block));
  assert.ok(/allPanes/.test(block), 'previews come from the panes this window has open');
  assert.ok(/windowLabel/.test(block), 'history entries name their preferred owner');
});

check('the module reaches the backend only through editor-service', () => {
  const source = NAVIGATION_MODULES.map((file) => fs.readFileSync(file, 'utf8')).join('\n');
  assert.ok(!/\binvoke\(/.test(source), 'never calls invoke() directly');
  assert.ok(!/listenOnCurrentWindow|__TAURI__/.test(source));
  assert.ok(/requestFeature/.test(source), 'requests use the flush/version barrier');
  assert.ok(/openLocalFileAt/.test(source), 'and targets go through the ownership authority');
});

check('the module registers no window or document key handlers', () => {
  const source = NAVIGATION_MODULES.map((file) => fs.readFileSync(file, 'utf8')).join('\n');
  assert.ok(!/addEventListener\(\s*['"]key(down|up|press)['"]/.test(source));
  assert.ok(!/\bdocument\.addEventListener\b/.test(source));
});

check('the module never uses alert or confirm', () => {
  const source = NAVIGATION_MODULES.map((file) => fs.readFileSync(file, 'utf8')).join('\n');
  assert.ok(!/\balert\(|\bconfirm\(/.test(source));
});

check('the module never sets innerHTML', () => {
  const source = NAVIGATION_MODULES.map((file) => fs.readFileSync(file, 'utf8')).join('\n');
  assert.ok(!/innerHTML/.test(source), 'paths go in as textContent, always');
});

check('the navigation module uses no regex lookbehind', () => {
  for (const file of NAVIGATION_MODULES.concat([EDITOR_SERVICE])) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(
      !/\(\?<[=!]/.test(source),
      `${file} uses a lookbehind — it costs the whole file on an older WKWebView`,
    );
  }
});

check('the navigation modules contain no control bytes', () => {
  for (const file of NAVIGATION_MODULES) {
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      const byte = bytes[i];
      assert.ok(
        byte >= 0x20 || byte === 0x0a || byte === 0x09,
        `control byte 0x${byte.toString(16)} at ${file}:${i} — git treats the file as binary`,
      );
    }
  }
});

check('index.html loads the history and chooser modules before the controller', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(`app/features/editor/${name}`);
  for (const name of ['lsp-navigation-history.js', 'lsp-navigation-chooser.js']) {
    assert.ok(at(name) > 0, `${name} is loaded at all`);
    assert.ok(at(name) < at('lsp-navigation.js'), `${name} is read by the controller`);
  }
});

check('the split modules stay in their lanes', () => {
  const historySource = fs.readFileSync(HISTORY, 'utf8');
  assert.ok(
    !/termlabEditorService|requestFeature|CM6/.test(historySource),
    'the history keeps stacks; it opens nothing and requests nothing',
  );
  const chooserSource = fs.readFileSync(CHOOSER, 'utf8');
  assert.ok(
    !/termlabEditorService|requestFeature/.test(chooserSource),
    'the chooser shows candidates; deciding what a pick means is the controller\'s job',
  );
});

check('the chooser classes it renders are styled with tokens', () => {
  const css = fs.readFileSync(EDITOR_CSS, 'utf8');
  const names = [
    'tl-definition-chooser',
    'tl-definition-chooser__item',
    'tl-definition-chooser__item--active',
    'tl-definition-chooser__where',
    'tl-definition-chooser__preview',
  ];
  for (const name of names) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  const block = css.slice(css.indexOf('.tl-definition-chooser'));
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(block), 'tokens only, no hex colours');
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
  console.log(`lsp navigation: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`lsp navigation: all ${ran} checks passed`);
}
