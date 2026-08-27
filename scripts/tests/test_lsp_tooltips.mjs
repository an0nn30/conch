// Run: node scripts/tests/test_lsp_tooltips.mjs
//
// LSP hover and signature help.
//
// Two halves, the same split test_lsp_completion.mjs uses:
//
//   * the REAL half — anything that depends on how CodeMirror actually
//     behaves (a StateField surviving a transaction, the showTooltip facet
//     producing a tooltip, the order the view would run keydown handlers in,
//     what `isUserEvent('input')` says about a real transaction) imports the
//     shipped packages from crates/termlab_tauri/frontend/node_modules and
//     asserts on real EditorState outcomes.
//
//   * the stub half — request gating, staleness, Markdown normalization and
//     rendering, where there is no CodeMirror behaviour involved.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const MODULES = path.join(APP, 'features/editor');
const TOOLTIPS = path.join(MODULES, 'lsp-tooltips.js');
const COMPLETION = path.join(MODULES, 'lsp-completion.js');
const VENDOR_ENTRY = path.join(ROOT, 'vendor-entry.mjs');
const INDEX_HTML = path.join(ROOT, 'index.html');
const COMPOSE = path.join(APP, 'manager-compose-runtime.js');
const PALETTE = path.join(APP, 'command-palette-runtime.js');
const EDITOR_CSS = path.join(ROOT, 'styles/design-system/components/editor.css');
const NODE_MODULES = path.join(ROOT, 'node_modules');
const LSP_TYPES = path.resolve(
  import.meta.dirname, '../../crates/termlab_tauri/src/lsp/types.rs',
);

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

const sleep = (ms) => new Promise((resolve) => { setTimeout(resolve, ms); });

// --- fake DOM ---------------------------------------------------------------
//
// Deliberately without innerHTML: a renderer that reaches for it is putting
// server text into the document as markup.
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
      setAttribute(name, value) { this[`attr:${name}`] = String(value); },
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

function realCM6(overrides) {
  const { state, view, autocomplete, vim } = REAL;
  return {
    EditorState: state.EditorState,
    StateField: state.StateField,
    StateEffect: state.StateEffect,
    Compartment: state.Compartment,
    Prec: state.Prec,
    ChangeSet: state.ChangeSet,
    EditorView: view.EditorView,
    keymap: view.keymap,
    showTooltip: view.showTooltip,
    lineNumbers: view.lineNumbers,
    completionStatus: autocomplete.completionStatus,
    autocompletion: autocomplete.autocompletion,
    vim: vim.vim,
    Vim: vim.Vim,
    ...(overrides || {}),
  };
}

// --- CM6 stand-in -------------------------------------------------------------
//
// Everything here is the shipped package except `completionStatus`, which
// reads a state field only a live EditorView installs — it is the one thing
// that cannot run headlessly, and the collision rule ("completion wins") has
// to be assertable. Stubbing the state field, the effect or the tooltip facet
// instead would let a broken field update pass, which is exactly the class of
// bug this file exists to catch.
function makeStubCM(sandbox) {
  let popup = null;
  const CM = realCM6({ completionStatus: () => popup });
  sandbox.CM6 = CM;
  return { CM, setPopup: (value) => { popup = value; } };
}

// --- sandbox -----------------------------------------------------------------
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

const READY_STATUS = {
  revision: 4,
  documentId: 'doc-1',
  state: 'ready',
  capabilities: {
    completion: true, hover: true, signatureHelp: true, definition: true, diagnostics: true,
  },
  completionTriggerCharacters: ['.', '::'],
  signatureHelpTriggerCharacters: ['(', ','],
  signatureHelpRetriggerCharacters: [','],
};

// A pane, its lsp-state record and a stubbed request path, wired the way
// manager-compose-runtime wires them in the app.
function harness(options = {}) {
  const opts = options || {};
  const docText = opts.text === undefined ? 'const value = format(1);' : opts.text;
  const { sandbox, cm, windowListeners } = load([TOOLTIPS]);
  const tooltips = sandbox.termlabLspTooltips;

  const state = {
    documentId: 'doc-1',
    version: 7,
    capabilities: { ...READY_STATUS.capabilities },
    status: opts.status === undefined ? READY_STATUS : opts.status,
  };
  const pane = { paneId: 'p1', kind: 'editor', view: null, filePath: '/repo/main.ts' };
  sandbox.termlabLspState = {
    get: (candidate) => (candidate === pane ? state : null),
  };

  // A view whose state is a REAL EditorState carrying the module's own
  // extensions, so `stateOf` reads a field that actually survived the
  // transactions the module dispatched.
  const extensions = tooltips.extensions();
  const view = {
    state: REAL.state.EditorState.create({ doc: docText, extensions }),
    dispatches: 0,
    dispatch(spec) {
      this.dispatches += 1;
      this.state = this.state.update(spec).state;
    },
    posAtCoords: (coords) => (opts.posAtCoords ? opts.posAtCoords(coords) : coords.x),
    hasFocus: true,
  };
  pane.view = view;

  const requests = [];
  let responder = opts.respond || (() => null);
  tooltips.configure({
    paneForView: (candidate) => (candidate === view ? pane : null),
    currentPane: () => pane,
    requestFeature: (target, kind, position, trigger) => {
      requests.push({ pane: target, kind, position, trigger });
      return Promise.resolve(responder(kind, position, trigger, requests.length));
    },
    hoverDelayMs: opts.hoverDelayMs === undefined ? 5 : opts.hoverDelayMs,
  });

  return {
    sandbox,
    cm,
    tooltips,
    pane,
    view,
    state,
    requests,
    windowListeners,
    extensions,
    setResponder: (fn) => { responder = fn; },
    phase: () => tooltips.stateOf(view).phase,
    snapshot: () => tooltips.stateOf(view),
  };
}

const HOVER_RESPONSE = {
  documentId: 'doc-1',
  sourceVersion: 7,
  range: { start: { line: 0, character: 14 }, end: { line: 0, character: 20 } },
  blocks: [{ markdown: false, value: 'function format(value: number): string' }],
};

const SIGNATURE_RESPONSE = {
  documentId: 'doc-1',
  sourceVersion: 7,
  signatures: [
    {
      label: 'format(value: number, radix?: number): string',
      documentation: [{ markdown: false, value: 'Formats a number.' }],
      parameters: [
        {
          label: 'value: number',
          labelStartUtf16: 7,
          labelEndUtf16: 20,
          documentation: [{ markdown: false, value: 'The number to format.' }],
        },
        { label: 'radix?: number', labelStartUtf16: null, labelEndUtf16: null, documentation: [] },
      ],
      activeParameter: null,
    },
    {
      label: 'format(value: string): string',
      documentation: [],
      parameters: [{ label: 'value: string', labelStartUtf16: 7, labelEndUtf16: 20, documentation: [] }],
      activeParameter: null,
    },
  ],
  activeSignature: 0,
  activeParameter: 1,
};

// --- the state machine --------------------------------------------------------

check('a fresh view is closed', () => {
  const h = harness();
  assert.strictEqual(h.phase(), 'closed');
});

check('a hover request goes pending and then visible', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE });
  const promise = h.tooltips.showHover(h.view);
  assert.strictEqual(h.phase(), 'pending', 'pending while the request is in flight');
  const pending = h.snapshot();
  assert.strictEqual(pending.kind, 'hover');
  assert.strictEqual(pending.version, 7, 'pending carries the document version it was made at');
  assert.ok(Number.isInteger(pending.request), 'and the request identity');
  await promise;
  const visible = h.snapshot();
  assert.strictEqual(visible.phase, 'visible');
  assert.strictEqual(visible.kind, 'hover');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(visible.anchor)), { from: 14, to: 20 },
    'anchored on the response range',
  );
});

check('a hover with no blocks dismisses instead of opening an empty box', async () => {
  const h = harness({ respond: () => ({ documentId: 'doc-1', sourceVersion: 7, blocks: [] }) });
  await h.tooltips.showHover(h.view);
  assert.strictEqual(h.phase(), 'closed');
});

check('a null response dismisses', async () => {
  const h = harness({ respond: () => null });
  await h.tooltips.showHover(h.view);
  assert.strictEqual(h.phase(), 'closed');
});

check('a hover with no range anchors on the requested position', async () => {
  const h = harness({
    respond: () => ({ ...HOVER_RESPONSE, range: null }),
  });
  await h.tooltips.showHover(h.view, 11);
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.snapshot().anchor)), { from: 11, to: 11 },
  );
});

check('a range past the end of the document is clamped, not thrown', async () => {
  const h = harness({
    respond: () => ({
      ...HOVER_RESPONSE,
      range: { start: { line: 40, character: 0 }, end: { line: 90, character: 5 } },
    }),
  });
  await h.tooltips.showHover(h.view);
  const anchor = h.snapshot().anchor;
  assert.ok(anchor.from <= h.view.state.doc.length && anchor.to <= h.view.state.doc.length);
});

check('only one LSP overlay is visible at a time', async () => {
  const h = harness({
    respond: (kind) => (kind === 'hover' ? HOVER_RESPONSE : SIGNATURE_RESPONSE),
  });
  await h.tooltips.showHover(h.view);
  assert.strictEqual(h.snapshot().kind, 'hover');
  await h.tooltips.showSignatureHelp(h.view);
  const visible = h.snapshot();
  assert.strictEqual(visible.kind, 'signatureHelp');
  assert.strictEqual(
    h.view.state.facet(REAL.view.showTooltip).filter(Boolean).length, 1,
    'the hover tooltip is gone, not merely behind the signature one',
  );
});

check('a superseded request never opens its overlay', async () => {
  let resolveFirst = null;
  const h = harness({
    respond: (kind, position, trigger, index) => (
      index === 1
        ? new Promise((resolve) => { resolveFirst = resolve; })
        : { ...HOVER_RESPONSE, blocks: [{ markdown: false, value: 'second' }] }
    ),
  });
  const first = h.tooltips.showHover(h.view, 3);
  const second = h.tooltips.showHover(h.view, 9);
  await second;
  resolveFirst({ ...HOVER_RESPONSE, blocks: [{ markdown: false, value: 'first' }] });
  await first;
  const visible = h.snapshot();
  assert.strictEqual(visible.phase, 'visible');
  assert.strictEqual(
    visible.payload.segments[0].value, 'second',
    'the late first response must not overwrite the second',
  );
});

check('a response for another document is rejected', async () => {
  const h = harness({ respond: () => ({ ...HOVER_RESPONSE, documentId: 'doc-9' }) });
  await h.tooltips.showHover(h.view);
  assert.strictEqual(h.phase(), 'closed');
});

check('a response that lands after the pane re-attached is rejected', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE });
  const promise = h.tooltips.showHover(h.view);
  h.state.documentId = 'doc-2';
  await promise;
  assert.strictEqual(h.phase(), 'closed');
});

// --- dismissal ----------------------------------------------------------------

check('Escape closes an open overlay and reports it consumed the key', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE });
  await h.tooltips.showHover(h.view);
  assert.strictEqual(h.tooltips.handleKeydown({ key: 'Escape' }, h.view), true);
  assert.strictEqual(h.phase(), 'closed');
});

check('Escape falls through when nothing is open, so vim still leaves insert mode', () => {
  const h = harness();
  assert.strictEqual(h.tooltips.handleKeydown({ key: 'Escape' }, h.view), false);
});

check('Escape with a modifier is not ours', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE });
  await h.tooltips.showHover(h.view);
  assert.strictEqual(h.tooltips.handleKeydown({ key: 'Escape', metaKey: true }, h.view), false);
  assert.strictEqual(h.phase(), 'visible');
});

check('Escape cancels a pending request too', async () => {
  let resolveIt = null;
  const h = harness({ respond: () => new Promise((resolve) => { resolveIt = resolve; }) });
  const promise = h.tooltips.showHover(h.view);
  assert.strictEqual(h.phase(), 'pending');
  assert.strictEqual(h.tooltips.handleKeydown({ key: 'Escape' }, h.view), true);
  assert.strictEqual(h.phase(), 'closed');
  resolveIt(HOVER_RESPONSE);
  await promise;
  assert.strictEqual(h.phase(), 'closed', 'the cancelled request must not open late');
});

check('blur closes', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE });
  await h.tooltips.showHover(h.view);
  h.tooltips.handleBlur(h.view);
  assert.strictEqual(h.phase(), 'closed');
});

check('scroll closes', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE });
  await h.tooltips.showHover(h.view);
  h.tooltips.handleScroll(h.view);
  assert.strictEqual(h.phase(), 'closed');
});

check('pointer movement away from the anchor closes the hover', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE, hoverDelayMs: 10000 });
  await h.tooltips.showHover(h.view);
  h.tooltips.handlePointerMove({ clientX: 2, clientY: 0 }, h.view);
  assert.strictEqual(h.phase(), 'closed');
});

check('pointer movement inside the anchor keeps the hover', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE, hoverDelayMs: 10000 });
  await h.tooltips.showHover(h.view);
  h.tooltips.handlePointerMove({ clientX: 16, clientY: 0 }, h.view);
  assert.strictEqual(h.phase(), 'visible');
});

check('leaving the editor cancels the dwell timer', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE, hoverDelayMs: 5 });
  h.tooltips.handlePointerMove({ clientX: 16, clientY: 0 }, h.view);
  h.tooltips.handlePointerLeave(h.view);
  await sleep(25);
  assert.strictEqual(h.requests.length, 0, 'the dwell request must never have been made');
});

// --- pointer dwell --------------------------------------------------------------

check('a pointer dwell requests hover after the delay', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE, hoverDelayMs: 5 });
  h.tooltips.handlePointerMove({ clientX: 16, clientY: 0 }, h.view);
  assert.strictEqual(h.requests.length, 0, 'not immediately');
  await sleep(30);
  assert.strictEqual(h.requests.length, 1);
  assert.strictEqual(h.requests[0].kind, 'hover');
});

check('movement during the dwell restarts it rather than firing twice', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE, hoverDelayMs: 20 });
  h.tooltips.handlePointerMove({ clientX: 4, clientY: 0 }, h.view);
  await sleep(5);
  h.tooltips.handlePointerMove({ clientX: 16, clientY: 0 }, h.view);
  await sleep(60);
  assert.strictEqual(h.requests.length, 1, 'one request, for the position it settled on');
  assert.strictEqual(h.requests[0].position.character, 16);
});

check('a dwell over a pane with no hover capability makes no request', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE, hoverDelayMs: 5 });
  h.state.capabilities = { ...READY_STATUS.capabilities, hover: false };
  h.tooltips.handlePointerMove({ clientX: 16, clientY: 0 }, h.view);
  await sleep(25);
  assert.strictEqual(h.requests.length, 0);
});

check('a dwell while the completion popup is open makes no request', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE, hoverDelayMs: 5 });
  h.cm.setPopup('active');
  h.tooltips.handlePointerMove({ clientX: 16, clientY: 0 }, h.view);
  await sleep(25);
  assert.strictEqual(h.requests.length, 0, 'completion wins the collision');
});

// --- signature help ---------------------------------------------------------------

check('signature help renders the active signature and highlights the active parameter', async () => {
  const h = harness({ respond: () => SIGNATURE_RESPONSE });
  await h.tooltips.showSignatureHelp(h.view);
  const payload = h.snapshot().payload;
  assert.strictEqual(payload.activeSignature, 0);
  assert.strictEqual(payload.activeParameter, 1);
  const node = h.tooltips.renderSignature(payload);
  const active = flatten(node).filter(
    (child) => String(child.className).indexOf('tl-signature__param--active') >= 0,
  );
  assert.strictEqual(active.length, 1, 'exactly one highlighted parameter');
  assert.strictEqual(active[0].textContent, 'radix?: number');
});

check('a per-signature activeParameter beats the response-level one', async () => {
  const response = {
    ...SIGNATURE_RESPONSE,
    signatures: [{ ...SIGNATURE_RESPONSE.signatures[0], activeParameter: 0 }],
    activeSignature: 0,
    activeParameter: 1,
  };
  const h = harness({ respond: () => response });
  await h.tooltips.showSignatureHelp(h.view);
  const node = h.tooltips.renderSignature(h.snapshot().payload);
  const active = flatten(node).filter(
    (child) => String(child.className).indexOf('tl-signature__param--active') >= 0,
  );
  assert.strictEqual(active[0].textContent, 'value: number');
});

check('a parameter with no label offsets is located by substring, or not highlighted', async () => {
  const response = {
    ...SIGNATURE_RESPONSE,
    signatures: [{
      label: 'draw(width: number)',
      documentation: [],
      parameters: [
        { label: 'width: number', labelStartUtf16: null, labelEndUtf16: null, documentation: [] },
        { label: 'absent', labelStartUtf16: null, labelEndUtf16: null, documentation: [] },
      ],
      activeParameter: null,
    }],
    activeSignature: 0,
    activeParameter: 0,
  };
  const h = harness({ respond: () => response });
  await h.tooltips.showSignatureHelp(h.view);
  const found = h.tooltips.renderSignature(h.snapshot().payload);
  const active = flatten(found).filter(
    (child) => String(child.className).indexOf('tl-signature__param--active') >= 0,
  );
  assert.strictEqual(active[0].textContent, 'width: number');

  const missing = h.tooltips.renderSignature({
    ...h.snapshot().payload,
    activeParameter: 1,
  });
  assert.strictEqual(
    flatten(missing).filter(
      (child) => String(child.className).indexOf('tl-signature__param--active') >= 0,
    ).length,
    0,
    'a parameter the label does not contain is left unhighlighted rather than guessed',
  );
});

check('the signature count is shown only when there is more than one', async () => {
  const h = harness({ respond: () => SIGNATURE_RESPONSE });
  await h.tooltips.showSignatureHelp(h.view);
  const node = h.tooltips.renderSignature(h.snapshot().payload);
  const count = flatten(node).find(
    (child) => String(child.className).indexOf('tl-signature__count') >= 0,
  );
  assert.ok(count, 'two signatures get a counter');
  assert.strictEqual(count.textContent, '1 of 2');

  const single = h.tooltips.renderSignature({
    ...h.snapshot().payload,
    signatures: [h.snapshot().payload.signatures[0]],
  });
  assert.ok(!flatten(single).some(
    (child) => String(child.className).indexOf('tl-signature__count') >= 0,
  ));
});

check('an empty signature list dismisses', async () => {
  const h = harness({
    respond: () => ({ documentId: 'doc-1', sourceVersion: 7, signatures: [] }),
  });
  await h.tooltips.showSignatureHelp(h.view);
  assert.strictEqual(h.phase(), 'closed');
});

// --- trigger characters -------------------------------------------------------

check('server trigger characters come off the status', () => {
  const h = harness();
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.tooltips.signatureTriggersFor(h.state))),
    { trigger: ['(', ','], retrigger: [','] },
  );
});

check('a status without trigger characters yields none, not a crash', () => {
  const h = harness({ status: { state: 'ready', capabilities: READY_STATUS.capabilities } });
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.tooltips.signatureTriggersFor(h.state))),
    { trigger: [], retrigger: [] },
  );
});

check('a trigger character opens signature help; a retrigger only re-requests an open one', () => {
  const h = harness();
  const decide = (character, open) => h.tooltips.signatureActionFor(h.state, character, open);
  assert.strictEqual(decide('(', false), 'trigger');
  assert.strictEqual(decide(',', false), 'trigger', 'a character in both lists still opens');
  assert.strictEqual(decide(')', true), null);
  assert.strictEqual(decide('x', false), null);
  assert.strictEqual(decide('', false), null);
});

check('a retrigger-only character re-requests when open and does nothing when closed', () => {
  const h = harness({
    status: {
      ...READY_STATUS,
      signatureHelpTriggerCharacters: ['('],
      signatureHelpRetriggerCharacters: [','],
    },
  });
  assert.strictEqual(h.tooltips.signatureActionFor(h.state, ',', true), 'retrigger');
  assert.strictEqual(h.tooltips.signatureActionFor(h.state, ',', false), null);
});

check('automatic triggers are refused when the session is not ready', () => {
  const h = harness({ status: { ...READY_STATUS, state: 'starting' } });
  assert.strictEqual(h.tooltips.signatureActionFor(h.state, '(', false), null);
});

// --- explicit actions ---------------------------------------------------------

check('the configured signature-help shortcut requests at the caret with no trigger', async () => {
  const h = harness({ respond: () => SIGNATURE_RESPONSE });
  h.view.dispatch({ selection: { anchor: 21 } });
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-signature-help'));
  await sleep(10);
  assert.strictEqual(h.requests.length, 1);
  assert.strictEqual(h.requests[0].kind, 'signatureHelp');
  assert.strictEqual(h.requests[0].trigger, null, 'manual invocation names no trigger character');
  assert.strictEqual(h.requests[0].position.character, 21);
});

check('manual signature help works even where no trigger character sits', async () => {
  const h = harness({
    respond: () => SIGNATURE_RESPONSE,
    status: { ...READY_STATUS, signatureHelpTriggerCharacters: [] },
  });
  await h.tooltips.showSignatureHelp(h.view, 3);
  assert.strictEqual(h.phase(), 'visible');
});

check('the command palette hover action requests hover at the caret', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE });
  h.view.dispatch({ selection: { anchor: 16 } });
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-show-hover'));
  await sleep(10);
  assert.strictEqual(h.requests.length, 1);
  assert.strictEqual(h.requests[0].kind, 'hover');
  assert.strictEqual(h.requests[0].position.character, 16);
});

check('dispose removes the window listeners', async () => {
  const h = harness({ respond: () => HOVER_RESPONSE });
  h.tooltips.dispose();
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-show-hover'));
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-signature-help'));
  await sleep(10);
  assert.strictEqual(h.requests.length, 0);
});

// --- Markdown -------------------------------------------------------------------

check('Markdown is normalized to text and fenced code segments', () => {
  const h = harness();
  const segments = h.tooltips.markdownSegments([
    { markdown: true, value: '## Title\nSome `code` and **bold**.\n```ts\nconst a = 1;\n```\nAfter.' },
  ]);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(segments)), [
    { type: 'text', value: 'Title\nSome code and bold.' },
    { type: 'code', value: 'const a = 1;' },
    { type: 'text', value: 'After.' },
  ]);
});

check('a plaintext block is passed through untouched', () => {
  const h = harness();
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.tooltips.markdownSegments([
      { markdown: false, value: '## not a heading **here**' },
    ]))),
    [{ type: 'text', value: '## not a heading **here**' }],
  );
});

check('an unterminated code fence still yields its content', () => {
  const h = harness();
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(h.tooltips.markdownSegments([
      { markdown: true, value: '```\nlet x = 1;' },
    ]))),
    [{ type: 'code', value: 'let x = 1;' }],
  );
});

check('empty and whitespace-only blocks produce no segments', () => {
  const h = harness();
  assert.deepStrictEqual(JSON.parse(JSON.stringify(h.tooltips.markdownSegments([
    { markdown: true, value: '   \n\n' },
    { markdown: false, value: '' },
    null,
  ]))), []);
});

check('server HTML is rendered as text, never as markup', () => {
  const h = harness();
  const evil = '<img src=x onerror="alert(1)"><script>steal()</script>';
  const node = h.tooltips.renderHover({
    segments: h.tooltips.markdownSegments([{ markdown: true, value: evil }]),
  });
  const all = flatten(node);
  assert.ok(all.every((child) => child.innerHTML === undefined), 'nothing set innerHTML');
  assert.ok(
    all.some((child) => String(child.textContent).indexOf('onerror') >= 0),
    'the payload survives as visible text',
  );
  assert.ok(
    !all.some((child) => child.tagName === 'IMG' || child.tagName === 'SCRIPT'),
    'and produced no elements of its own',
  );
});

check('a code segment renders as a pre with the source verbatim', () => {
  const h = harness();
  const node = h.tooltips.renderHover({
    segments: [{ type: 'code', value: 'fn main() {}' }],
  });
  const pre = flatten(node).find((child) => child.tagName === 'PRE');
  assert.ok(pre, 'code is rendered in a pre');
  assert.strictEqual(pre.textContent, 'fn main() {}');
});

check('signature documentation is rendered as text too', async () => {
  const h = harness({ respond: () => SIGNATURE_RESPONSE });
  await h.tooltips.showSignatureHelp(h.view);
  const node = h.tooltips.renderSignature(h.snapshot().payload);
  assert.ok(textOf(node).indexOf('The number to format.') < 0
    || textOf(node).indexOf('Formats a number.') >= 0);
  assert.ok(flatten(node).every((child) => child.innerHTML === undefined));
});

// --- the REAL half --------------------------------------------------------------

check('the overlay lives in a real StateField that the showTooltip facet reads', async () => {
  const h = harness({ real: true, respond: () => HOVER_RESPONSE });
  await h.tooltips.showHover(h.view);
  const tooltips = h.view.state.facet(REAL.view.showTooltip).filter(Boolean);
  assert.strictEqual(tooltips.length, 1, 'exactly one tooltip is shown');
  assert.strictEqual(tooltips[0].pos, 14);
  assert.strictEqual(tooltips[0].end, 20);
  assert.strictEqual(typeof tooltips[0].create, 'function');
});

check('a real document change clears the overlay through the field itself', async () => {
  const h = harness({ real: true, respond: () => HOVER_RESPONSE });
  await h.tooltips.showHover(h.view);
  assert.strictEqual(h.phase(), 'visible');
  h.view.dispatch({ changes: { from: 0, insert: 'x' }, userEvent: 'input.type' });
  assert.strictEqual(h.phase(), 'closed', 'the field drops the value on docChanged');
  assert.strictEqual(h.view.state.facet(REAL.view.showTooltip).filter(Boolean).length, 0);
});

// Signature help is the one overlay an edit must NOT close: the whole point
// of a retrigger character is that typing `,` inside a call updates the open
// tooltip. So an edit closes hover, cancels any pending request, and carries
// an open signature overlay forward with its anchor mapped through the change.
check('an edit carries an open signature overlay forward with a mapped anchor', async () => {
  const h = harness({ respond: () => SIGNATURE_RESPONSE, text: 'format()' });
  await h.tooltips.showSignatureHelp(h.view, 7);
  const anchor = () => JSON.parse(JSON.stringify(h.snapshot().anchor));
  assert.deepStrictEqual(anchor(), { from: 7, to: 7 });
  h.view.dispatch({ changes: { from: 0, insert: 'const x = ' }, userEvent: 'input.type' });
  assert.strictEqual(h.phase(), 'visible');
  assert.deepStrictEqual(anchor(), { from: 17, to: 17 }, 'anchor mapped, not stale');
});

check('an edit cancels an in-flight request', async () => {
  let resolveIt = null;
  const h = harness({ respond: () => new Promise((resolve) => { resolveIt = resolve; }) });
  const promise = h.tooltips.showHover(h.view);
  h.tooltips.noteDocumentChange(h.view, {
    docChanged: true, state: h.view.state, transactions: [],
  });
  assert.strictEqual(h.phase(), 'closed');
  resolveIt(HOVER_RESPONSE);
  await promise;
  assert.strictEqual(h.phase(), 'closed');
});

check('a caret move clears a hover but leaves signature help alone', async () => {
  const h = harness({
    real: true,
    respond: (kind) => (kind === 'hover' ? HOVER_RESPONSE : SIGNATURE_RESPONSE),
  });
  await h.tooltips.showHover(h.view);
  h.view.dispatch({ selection: { anchor: 3 } });
  assert.strictEqual(h.phase(), 'closed');
  await h.tooltips.showSignatureHelp(h.view);
  h.view.dispatch({ selection: { anchor: 5 } });
  assert.strictEqual(h.phase(), 'visible', 'moving between arguments must not close signature help');
});

check('a real input transaction fires the automatic signature trigger', async () => {
  const h = harness({ real: true, respond: () => SIGNATURE_RESPONSE, text: 'format' });
  const update = h.view.state.update({
    changes: { from: 6, insert: '(' },
    selection: { anchor: 7 },
    userEvent: 'input.type',
  });
  h.view.state = update.state;
  h.tooltips.noteDocumentChange(h.view, {
    docChanged: true, state: update.state, transactions: [update],
  });
  await sleep(10);
  assert.strictEqual(h.requests.length, 1);
  assert.strictEqual(h.requests[0].kind, 'signatureHelp');
  assert.strictEqual(h.requests[0].trigger, '(');
});

check('a programmatic change fires no automatic trigger', async () => {
  const h = harness({ real: true, respond: () => SIGNATURE_RESPONSE, text: 'format' });
  const update = h.view.state.update({
    changes: { from: 6, insert: '(' },
    selection: { anchor: 7 },
  });
  h.view.state = update.state;
  h.tooltips.noteDocumentChange(h.view, {
    docChanged: true, state: update.state, transactions: [update],
  });
  await sleep(10);
  assert.strictEqual(
    h.requests.length, 0,
    'automatic signature help follows edits, not every document mutation',
  );
});

check('the Escape handler is raised above vim, which is a plugin and not a keymap', () => {
  const h = harness({ real: true });
  const state = REAL.state.EditorState.create({
    doc: 'x',
    extensions: [REAL.vim.vim(), h.tooltips.extensions()],
  });
  const owners = keydownOwners(state);
  assert.ok(owners.length >= 2, 'both vim and the overlay handler registered a keydown');
  const vimOnly = keydownOwners(REAL.state.EditorState.create({
    doc: 'x', extensions: [REAL.vim.vim()],
  }));
  const vimId = vimOnly[vimOnly.length - 1];
  assert.ok(
    owners.indexOf(vimId) > 0,
    'the overlay handler runs before vim, so Escape closes the tooltip first',
  );
});

check('extensions() is stable, so mounting twice does not install two fields', () => {
  const h = harness({ real: true });
  const state = REAL.state.EditorState.create({
    doc: 'x',
    extensions: [h.tooltips.extensions(), h.tooltips.extensions()],
  });
  assert.strictEqual(state.facet(REAL.view.showTooltip).filter(Boolean).length, 0);
  assert.ok(state, 'two mounts of the same field must not throw');
});

check('extensions() is empty when the bundle lacks the tooltip exports', () => {
  const { sandbox } = load([TOOLTIPS], null, (box) => {
    box.CM6 = { EditorState: REAL.state.EditorState };
    return { CM: box.CM6, setPopup: () => {} };
  });
  assert.strictEqual(sandbox.termlabLspTooltips.extensions().length, 0);
});

// --- wiring and hygiene -----------------------------------------------------------

check('vendor-entry re-exports the tooltip APIs the module needs', () => {
  const source = fs.readFileSync(VENDOR_ENTRY, 'utf8');
  assert.ok(/\bshowTooltip\b/.test(source), 'showTooltip drives the overlay');
  assert.ok(/\bStateField\b/.test(source), 'StateField holds it');
});

check('index.html loads lsp-tooltips.js before editor-pane.js', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(name);
  assert.ok(at('app/features/editor/lsp-tooltips.js') > 0, 'the module is loaded at all');
  assert.ok(
    at('app/features/editor/lsp-tooltips.js') < at('app/features/editor/editor-pane.js'),
    'editor-pane reads termlabLspTooltips at view-construction time',
  );
});

check('editor-pane mounts the tooltip extensions defensively', () => {
  const source = fs.readFileSync(path.join(MODULES, 'editor-pane.js'), 'utf8');
  assert.ok(/termlabLspTooltips/.test(source));
  assert.ok(
    /typeof global\.termlabLspTooltips\.extensions === 'function'/.test(source),
    'a stale vendor bundle must cost tooltips and nothing else',
  );
});

check('the compose runtime configures the controller with the lookups only it has', () => {
  const source = fs.readFileSync(COMPOSE, 'utf8');
  assert.ok(/termlabLspTooltips/.test(source));
  const at = source.indexOf('termlabLspTooltips');
  const block = source.slice(at, at + 700);
  assert.ok(/paneForView/.test(block));
  assert.ok(/currentPane/.test(block));
});

check('the command palette offers Show Hover, since it has no default chord', () => {
  const source = fs.readFileSync(PALETTE, 'utf8');
  assert.ok(/termlab:editor-show-hover/.test(source), 'it dispatches the application event');
  assert.ok(/Show Hover/.test(source), 'under a findable title');
});

check('the module reaches the backend only through editor-service', () => {
  const source = fs.readFileSync(TOOLTIPS, 'utf8');
  assert.ok(!/\binvoke\(/.test(source), 'never calls invoke() directly');
  assert.ok(!/listenOnCurrentWindow|__TAURI__/.test(source));
  assert.ok(/requestFeature/.test(source), 'it uses the flush/version barrier');
});

check('the module adds no outer version check around the barrier', () => {
  const source = fs.readFileSync(TOOLTIPS, 'utf8');
  assert.ok(
    !/version !== .*barrier|barrier\.version/.test(source),
    'the barrier owns the version guard; a second one outside it rejects good results',
  );
});

check('the module registers no window or document key handlers', () => {
  const source = fs.readFileSync(TOOLTIPS, 'utf8');
  assert.ok(!/addEventListener\(\s*['"]key(down|up|press)['"]/.test(source));
  assert.ok(!/\bdocument\.addEventListener\b/.test(source));
});

check('the module never uses alert or confirm', () => {
  const source = fs.readFileSync(TOOLTIPS, 'utf8');
  assert.ok(!/\balert\(|\bconfirm\(/.test(source));
});

check('the module never sets innerHTML', () => {
  const source = fs.readFileSync(TOOLTIPS, 'utf8');
  assert.ok(!/innerHTML/.test(source), 'server text goes in as textContent, always');
});

check('the tooltip modules use no regex lookbehind', () => {
  // A regex literal is validated when the FILE is parsed, so one lookbehind
  // would stop the module loading at all on an older WKWebView — and because
  // every call site guards on the module being present, the failure would be
  // silent: hover would simply never appear.
  for (const file of [TOOLTIPS, COMPLETION]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(
      !/\(\?<[=!]/.test(source),
      `${file} uses a lookbehind — it costs the whole file on an older WKWebView`,
    );
  }
});

check('the tooltip module contains no control bytes', () => {
  const bytes = fs.readFileSync(TOOLTIPS);
  for (let i = 0; i < bytes.length; i += 1) {
    const byte = bytes[i];
    assert.ok(
      byte >= 0x20 || byte === 0x0a || byte === 0x09,
      `control byte 0x${byte.toString(16)} at offset ${i} — git treats the file as binary`,
    );
  }
});

check('the overlay classes it renders are styled with tokens', () => {
  const css = fs.readFileSync(EDITOR_CSS, 'utf8');
  const names = [
    'tl-hover', 'tl-hover__text', 'tl-hover__code',
    'tl-signature', 'tl-signature__label', 'tl-signature__param--active',
    'tl-signature__count', 'tl-signature__doc',
  ];
  for (const name of names) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  const block = css.slice(css.indexOf('.tl-hover'));
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(block), 'tokens only, no hex colours');
});

// --- the payload the frontend reads ------------------------------------------------

check('LspStatus carries the trigger characters both surfaces read', () => {
  const source = fs.readFileSync(LSP_TYPES, 'utf8');
  const at = source.indexOf('pub(crate) struct LspStatus');
  assert.ok(at > 0, 'LspStatus exists');
  const block = source.slice(at, source.indexOf('}', at));
  for (const field of [
    'completion_trigger_characters',
    'signature_help_trigger_characters',
    'signature_help_retrigger_characters',
  ]) {
    assert.ok(block.includes(field), `LspStatus must carry ${field}`);
  }
});

check('lsp-completion already reads the completion trigger characters off the status', () => {
  const source = fs.readFileSync(COMPLETION, 'utf8');
  assert.ok(/status\.completionTriggerCharacters/.test(source));
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
  console.log(`lsp tooltips: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`lsp tooltips: all ${ran} checks passed`);
}
