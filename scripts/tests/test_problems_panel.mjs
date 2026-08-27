// Run: node scripts/tests/test_problems_panel.mjs
//
// The workspace Problems tool window: its pure model (grouping, filtering,
// counts, ordering, states), the live store that projects Rust's revisioned
// diagnostic snapshot, F8/Shift-F8 navigation, and the panel view.
//
// No jsdom in this repo (see test_tl_dialog.mjs for the precedent), so the DOM
// is stubbed to the surface these modules actually touch — the same idiom
// test_transfer_center.mjs uses. Deliberately does NOT define `sandbox.global`:
// tool-window-manager.js binds `exports`, never `global`, and a harness that
// helpfully supplies one would hide a bare `global.` regression (see
// test_dock_highlight.mjs's source guard).
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const PROBLEMS = path.join(APP, 'features/problems');
const MODEL = path.join(PROBLEMS, 'problems-model.js');
const STORE = path.join(PROBLEMS, 'problems-store.js');
const NAVIGATION = path.join(PROBLEMS, 'problems-navigation.js');
const PANEL = path.join(APP, 'panels/problems-panel.js');
const LSP_URI = path.join(APP, 'features/editor/lsp-uri.js');
const TOOL_WINDOW_RUNTIME = path.join(APP, 'tool-window-runtime.js');
const TOOL_WINDOW_MANAGER = path.join(APP, 'layout/tool-window-manager.js');
const SHORTCUT_RUNTIME = path.join(APP, 'shortcut-runtime.js');
const INDEX_HTML = path.join(ROOT, 'index.html');
const CSS = path.join(ROOT, 'styles/design-system/components/problems.css');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

// Values built inside the vm context belong to another realm, so an Array
// from there is not `Array` here and deepStrictEqual refuses it. Every
// structural comparison in this file goes through one JSON round trip first.
const plain = (value) => JSON.parse(JSON.stringify(value === undefined ? null : value));
function deepEq(actual, expected, message) {
  assert.deepStrictEqual(plain(actual), plain(expected), message);
}

// --- DOM stand-in -------------------------------------------------------------
let focusDocument = null;

function dataName(attr) {
  return attr.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
}

function selectorParts(selector) {
  const match = /^([a-zA-Z]*)(?:#([\w-]+))?(?:\.([\w-]+))?(?:\[([\w-]+)(?:="([^"]*)")?\])?$/.exec(selector);
  if (!match) throw new Error(`unsupported selector in test DOM: ${selector}`);
  return { tag: match[1], id: match[2], className: match[3], attr: match[4], attrValue: match[5] };
}

function makeElement(tag) {
  const listeners = new Map();
  const attributes = new Map();
  let ownText = '';
  let className = '';
  const element = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    parentNode: null,
    dataset: {},
    style: {},
    id: '',
    hidden: false,
    disabled: false,
    type: '',
    value: '',
    title: '',
    tabIndex: -1,
    _focusCount: 0,
    get isConnected() {
      let current = this;
      while (current && current.parentNode) current = current.parentNode;
      return !!current && current.tagName === 'BODY';
    },
    get className() { return className; },
    set className(value) { className = String(value || ''); },
    classList: {
      add(...names) {
        const values = new Set(className.split(/\s+/).filter(Boolean));
        names.forEach((name) => values.add(name));
        className = Array.from(values).join(' ');
      },
      remove(...names) {
        const removed = new Set(names);
        className = className.split(/\s+/).filter((name) => name && !removed.has(name)).join(' ');
      },
      toggle(name, force) {
        const exists = this.contains(name);
        const next = force === undefined ? !exists : !!force;
        if (next) this.add(name); else this.remove(name);
        return next;
      },
      contains(name) { return className.split(/\s+/).includes(name); },
    },
    get textContent() {
      return ownText + this.children.map((child) => child.textContent || '').join('');
    },
    set textContent(value) {
      for (const child of this.children) child.parentNode = null;
      this.children = [];
      ownText = String(value == null ? '' : value);
    },
    appendChild(child) {
      if (child.parentNode) child.parentNode.removeChild(child);
      this.children.push(child);
      child.parentNode = this;
      return child;
    },
    removeChild(child) {
      const index = this.children.indexOf(child);
      if (index >= 0) this.children.splice(index, 1);
      child.parentNode = null;
      return child;
    },
    replaceChildren(...children) {
      for (const child of this.children) child.parentNode = null;
      this.children = [];
      ownText = '';
      children.forEach((child) => this.appendChild(child));
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    focus() {
      this._focusCount += 1;
      if (focusDocument) focusDocument.activeElement = this;
    },
    scrollIntoView() {},
    setAttribute(name, value) {
      const stringValue = String(value);
      attributes.set(name, stringValue);
      if (name === 'id') this.id = stringValue;
      else if (name === 'class') this.className = stringValue;
      else if (name === 'tabindex') this.tabIndex = Number(stringValue);
      else if (name.startsWith('data-')) this.dataset[dataName(name)] = stringValue;
    },
    removeAttribute(name) { attributes.delete(name); },
    getAttribute(name) {
      if (name === 'id' && this.id) return this.id;
      if (name === 'class' && this.className) return this.className;
      if (name.startsWith('data-')) {
        const value = this.dataset[dataName(name)];
        return value === undefined ? null : String(value);
      }
      return attributes.has(name) ? attributes.get(name) : null;
    },
    matches(selector) {
      const parts = selectorParts(selector);
      if (parts.tag && this.tagName !== parts.tag.toUpperCase()) return false;
      if (parts.id && this.id !== parts.id) return false;
      if (parts.className && !this.classList.contains(parts.className)) return false;
      if (parts.attr) {
        const value = this.getAttribute(parts.attr);
        if (value === null) return false;
        if (parts.attrValue !== undefined && value !== parts.attrValue) return false;
      }
      return true;
    },
    closest(selector) {
      let current = this;
      while (current) {
        if (current.matches(selector)) return current;
        current = current.parentNode;
      }
      return null;
    },
    querySelectorAll(selector) {
      const found = [];
      const visit = (node) => {
        for (const child of node.children) {
          if (child.matches(selector)) found.push(child);
          visit(child);
        }
      };
      visit(this);
      return found;
    },
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; },
    addEventListener(name, handler) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(handler);
    },
    removeEventListener(name, handler) {
      const values = listeners.get(name) || [];
      const index = values.indexOf(handler);
      if (index >= 0) values.splice(index, 1);
    },
    // Bubbles, because the panel delegates its click handling to the list and
    // a stub that fired only on the target would let a per-row-handler
    // regression pass while the real DOM behaved differently.
    _fire(name, event = {}) {
      let stopped = false;
      const record = {
        target: this,
        preventDefault() { record.defaultPrevented = true; },
        stopPropagation() { stopped = true; },
        defaultPrevented: false,
        ...event,
      };
      let node = this;
      while (node && !stopped) {
        for (const handler of node._listeners(name)) {
          handler(record);
          if (stopped) break;
        }
        node = node.parentNode;
      }
      return record;
    },
    _listeners(name) { return (listeners.get(name) || []).slice(); },
    _listenerCount(name) { return (listeners.get(name) || []).length; },
  };
  return element;
}

// --- fixtures -----------------------------------------------------------------

const range = (sl, sc, el, ec) => ({
  start: { line: sl, character: sc },
  end: { line: el, character: ec },
});

const diag = (overrides) => ({
  id: 'd-1',
  uri: 'file:///repo/src/main.ts',
  range: range(0, 0, 0, 4),
  severity: 'error',
  code: null,
  source: null,
  message: 'Something',
  relatedInformation: [],
  ...(overrides || {}),
});

const session = (overrides) => ({
  revision: 1,
  documentId: null,
  sessionId: 'session-ts',
  adapterId: 'typescript',
  projectRootUri: 'file:///repo',
  state: 'ready',
  message: null,
  capabilities: {
    completion: true, hover: true, signatureHelp: true, definition: true, diagnostics: true,
  },
  errorCount: 0,
  warningCount: 0,
  ...(overrides || {}),
});

function counts(items) {
  const out = { errors: 0, warnings: 0, information: 0, hints: 0 };
  for (const item of items) {
    if (item.severity === 'error') out.errors += 1;
    else if (item.severity === 'warning') out.warnings += 1;
    else if (item.severity === 'information') out.information += 1;
    else out.hints += 1;
  }
  return out;
}

const snapshot = (items, revision) => ({
  revision: revision === undefined ? 1 : revision,
  items: items || [],
  counts: counts(items || []),
});

const diagnosticsEvent = (items, overrides) => ({
  revision: 1,
  sessionId: 'session-ts',
  documentId: null,
  uri: null,
  snapshot: snapshot(items, (overrides && overrides.revision) || 1),
  ...(overrides || {}),
});

// --- sandbox ------------------------------------------------------------------

function load(files, extra) {
  const body = makeElement('body');
  const documentStub = {
    body,
    activeElement: body,
    createElement: (tag) => makeElement(tag),
    getElementById: (id) => body.querySelector(`#${id}`),
    querySelector: (selector) => body.querySelector(selector),
    querySelectorAll: (selector) => body.querySelectorAll(selector),
    addEventListener() {},
    removeEventListener() {},
  };
  focusDocument = documentStub;
  const windowListeners = new Map();
  const sandbox = {
    console, setTimeout, clearTimeout, queueMicrotask, Promise, Map, Set, WeakMap,
    Array, Object, JSON, Date, RegExp, String, Number, Boolean, Math, Error,
    decodeURIComponent, encodeURIComponent, Intl,
    document: documentStub,
  };
  sandbox.window = sandbox;
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
  sandbox.utils = {
    esc: (value) => String(value === null || value === undefined ? '' : value),
  };
  sandbox.toast = { error() {}, success() {}, info() {}, warn() {} };
  Object.assign(sandbox, extra || {});
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return { sandbox, body, documentStub, windowListeners };
}

function modelOnly() {
  const { sandbox } = load([LSP_URI, MODEL]);
  return sandbox.termlabProblemsModel;
}

// A store wired to a fake bridge, plus optional navigation and panel.
function storeHarness(options = {}) {
  const opts = options || {};
  const files = [LSP_URI, MODEL, STORE];
  if (opts.navigation) files.push(NAVIGATION);
  if (opts.panel) files.push(PANEL);
  const { sandbox, body, documentStub, windowListeners } = load(files);

  let diagnosticsListener = null;
  let statusListener = null;
  const bridge = {
    subscribeDiagnostics(fn) {
      diagnosticsListener = fn;
      return () => { diagnosticsListener = null; };
    },
    subscribeStatus(fn) {
      statusListener = fn;
      return () => { statusListener = null; };
    },
    problemsSnapshot: () => Promise.resolve(
      opts.initialSnapshot === undefined ? snapshot([], 0) : opts.initialSnapshot,
    ),
    statusSnapshot: () => Promise.resolve(opts.initialSessions || []),
  };
  sandbox.termlabLspBridge = bridge;
  const store = sandbox.termlabProblemsStore;
  store.configure({});

  return {
    sandbox,
    body,
    documentStub,
    windowListeners,
    store,
    navigation: sandbox.termlabProblemsNavigation,
    panelModule: sandbox.problemsPanel,
    publish: (payload) => diagnosticsListener && diagnosticsListener(payload),
    publishStatus: (payload) => statusListener && statusListener(payload),
  };
}

// ===========================================================================
// Model: grouping
// ===========================================================================

check('items are grouped by project root, then by file', () => {
  const model = modelOnly();
  const built = model.build({
    items: [
      diag({ id: 'a', uri: 'file:///repo/src/main.ts' }),
      diag({ id: 'b', uri: 'file:///repo/src/other.ts' }),
      diag({ id: 'c', uri: 'file:///other-project/lib.rs' }),
    ],
    sessions: [
      session({ sessionId: 's1', adapterId: 'typescript', projectRootUri: 'file:///repo' }),
      session({ sessionId: 's2', adapterId: 'rust', projectRootUri: 'file:///other-project' }),
    ],
  });
  deepEq(built.groups.map((g) => g.root), ['/other-project', '/repo']);
  const repo = built.groups.find((g) => g.root === '/repo');
  deepEq(repo.files.map((f) => f.relativePath), ['src/main.ts', 'src/other.ts']);
});

check('the deepest matching root wins for nested projects', () => {
  const model = modelOnly();
  assert.strictEqual(
    model.rootForPath('/repo/crates/inner/src/a.rs', ['/repo', '/repo/crates/inner']),
    '/repo/crates/inner',
  );
});

check('root matching uses path components, so /repo does not swallow /repository', () => {
  const model = modelOnly();
  assert.strictEqual(model.rootForPath('/repository/src/a.ts', ['/repo']), null);
  assert.strictEqual(model.rootForPath('/repo-old/a.ts', ['/repo']), null);
  assert.strictEqual(model.rootForPath('/repo/a.ts', ['/repo']), '/repo');
  assert.strictEqual(model.rootForPath('/repo', ['/repo']), '/repo');
});

check('a file under no active project root lands in its own group, last', () => {
  const model = modelOnly();
  const built = model.build({
    items: [
      diag({ id: 'a', uri: 'file:///repo/src/main.ts' }),
      diag({ id: 'b', uri: 'file:///tmp/scratch.ts' }),
    ],
    sessions: [session({ projectRootUri: 'file:///repo' })],
  });
  assert.strictEqual(built.groups.length, 2);
  assert.strictEqual(built.groups[0].root, '/repo');
  assert.strictEqual(built.groups[1].root, null);
  deepEq(built.groups[1].files.map((f) => f.relativePath), ['/tmp/scratch.ts']);
});

check('a group is labelled by its root basename and carries its adapters', () => {
  const model = modelOnly();
  const built = model.build({
    items: [diag({ uri: 'file:///Users/dev/my-app/src/a.ts' })],
    sessions: [
      session({ sessionId: 's1', adapterId: 'typescript', projectRootUri: 'file:///Users/dev/my-app' }),
      session({ sessionId: 's2', adapterId: 'eslint', projectRootUri: 'file:///Users/dev/my-app' }),
    ],
  });
  assert.strictEqual(built.groups[0].label, 'my-app');
  assert.strictEqual(built.groups[0].root, '/Users/dev/my-app');
  deepEq(built.groups[0].adapters, ['eslint', 'typescript']);
});

check('a ready project with nothing to report is dropped; a struggling one is not', () => {
  const model = modelOnly();
  const ready = model.build({
    items: [],
    sessions: [session({ state: 'ready', projectRootUri: 'file:///repo' })],
  });
  deepEq(ready.groups, []);

  const failed = model.build({
    items: [],
    sessions: [session({ state: 'failed', projectRootUri: 'file:///repo', message: 'exited' })],
  });
  assert.strictEqual(failed.groups.length, 1, 'the failure needs somewhere to be explained');
  assert.strictEqual(failed.groups[0].state, 'failed');
  assert.strictEqual(failed.groups[0].message, 'exited');
});

check('a project group disappears when its last session stops', () => {
  const model = modelOnly();
  const before = model.build({
    items: [diag({ uri: 'file:///repo/a.ts' })],
    sessions: [session({ projectRootUri: 'file:///repo' })],
  });
  assert.strictEqual(before.groups[0].root, '/repo');
  // The session is gone AND Rust cleared its records: nothing is left to show.
  const after = model.build({ items: [], sessions: [] });
  deepEq(after.groups, []);
});

// ===========================================================================
// Model: ordering
// ===========================================================================

check('items in a file sort by severity, then line, then column — stably', () => {
  const model = modelOnly();
  const built = model.build({
    items: [
      diag({ id: 'hint', severity: 'hint', range: range(0, 0, 0, 1), message: 'h' }),
      diag({ id: 'err-late', severity: 'error', range: range(5, 2, 5, 3), message: 'e2' }),
      diag({ id: 'warn', severity: 'warning', range: range(0, 0, 0, 1), message: 'w' }),
      diag({ id: 'err-early', severity: 'error', range: range(5, 1, 5, 2), message: 'e1' }),
      diag({ id: 'info', severity: 'information', range: range(0, 0, 0, 1), message: 'i' }),
    ],
    sessions: [session()],
  });
  deepEq(
    built.flat.map((item) => item.id),
    ['err-early', 'err-late', 'warn', 'info', 'hint'],
  );
});

check('two diagnostics at the identical position keep a total, repeatable order', () => {
  const model = modelOnly();
  const items = [
    diag({ id: 'b', source: 'zeta', message: 'b' }),
    diag({ id: 'a', source: 'alpha', message: 'a' }),
  ];
  const first = model.build({ items, sessions: [session()] }).flat.map((i) => i.id);
  const second = model.build({ items: items.slice().reverse(), sessions: [session()] })
    .flat.map((i) => i.id);
  deepEq(first, second, 'input order must not leak into the list');
});

check('groups sort by root path and files sort by relative path', () => {
  const model = modelOnly();
  const built = model.build({
    items: [
      diag({ id: '1', uri: 'file:///repo/z/last.ts' }),
      diag({ id: '2', uri: 'file:///repo/a/first.ts' }),
      diag({ id: '3', uri: 'file:///alpha/x.ts' }),
    ],
    sessions: [
      session({ sessionId: 's1', projectRootUri: 'file:///repo' }),
      session({ sessionId: 's2', projectRootUri: 'file:///alpha' }),
    ],
  });
  deepEq(built.groups.map((g) => g.root), ['/alpha', '/repo']);
  deepEq(
    built.groups[1].files.map((f) => f.relativePath),
    ['a/first.ts', 'z/last.ts'],
  );
});

// ===========================================================================
// Model: counts and filters
// ===========================================================================

check('counts are reported at the global, project and file levels', () => {
  const model = modelOnly();
  const built = model.build({
    items: [
      diag({ id: 'a', uri: 'file:///repo/a.ts', severity: 'error' }),
      diag({ id: 'b', uri: 'file:///repo/a.ts', severity: 'warning' }),
      diag({ id: 'c', uri: 'file:///repo/b.ts', severity: 'hint' }),
      diag({ id: 'd', uri: 'file:///other/c.ts', severity: 'error' }),
    ],
    sessions: [
      session({ sessionId: 's1', projectRootUri: 'file:///repo' }),
      session({ sessionId: 's2', projectRootUri: 'file:///other' }),
    ],
  });
  assert.strictEqual(built.counts.error, 2);
  assert.strictEqual(built.counts.total, 4);
  const repo = built.groups.find((g) => g.root === '/repo');
  assert.strictEqual(repo.counts.error, 1);
  assert.strictEqual(repo.counts.warning, 1);
  assert.strictEqual(repo.counts.hint, 1);
  assert.strictEqual(repo.counts.total, 3);
  const fileA = repo.files.find((f) => f.relativePath === 'a.ts');
  assert.strictEqual(fileA.counts.total, 2);
  assert.strictEqual(fileA.counts.error, 1);
});

check('a severity filter removes those rows but leaves the unfiltered totals intact', () => {
  const model = modelOnly();
  const items = [
    diag({ id: 'a', severity: 'error' }),
    diag({ id: 'b', severity: 'warning' }),
    diag({ id: 'c', severity: 'hint' }),
  ];
  const built = model.build({
    items,
    sessions: [session()],
    filters: { severities: { error: true, warning: false, information: false, hint: false } },
  });
  deepEq(built.flat.map((i) => i.id), ['a']);
  assert.strictEqual(built.counts.total, 1, 'counts describe what is shown');
  assert.strictEqual(built.totals.total, 3, 'totals describe what exists — the toggles read these');
  assert.strictEqual(built.totals.warning, 1);
});

check('the text filter matches message, path, source and code, case-insensitively', () => {
  const model = modelOnly();
  const items = [
    diag({ id: 'msg', message: 'Unexpected TOKEN' }),
    diag({ id: 'path', uri: 'file:///repo/src/widget.ts', message: 'x' }),
    diag({ id: 'source', source: 'rust-analyzer', message: 'x' }),
    diag({ id: 'code', code: 'E0432', message: 'x' }),
  ];
  const only = (text) => model.build({
    items, sessions: [session()], filters: { text },
  }).flat.map((i) => i.id);
  deepEq(only('token'), ['msg']);
  deepEq(only('widget'), ['path']);
  deepEq(only('analyzer'), ['source']);
  deepEq(only('e0432'), ['code']);
  assert.strictEqual(only('   ').length, 4, 'a blank filter filters nothing');
});

check('a filter that hides everything empties the groups too', () => {
  const model = modelOnly();
  const built = model.build({
    items: [diag({ severity: 'hint' })],
    sessions: [session({ state: 'ready' })],
    filters: { severities: { error: true, warning: true, information: true, hint: false } },
  });
  deepEq(built.flat, []);
  deepEq(built.groups, []);
});

// ===========================================================================
// Model: positions and states
// ===========================================================================

check('a normalized item carries 1-based line and column for display', () => {
  const model = modelOnly();
  const item = model.build({
    items: [diag({ range: range(11, 4, 11, 9) })],
    sessions: [session()],
  }).flat[0];
  assert.strictEqual(item.line, 12);
  assert.strictEqual(item.column, 5);
  assert.strictEqual(item.path, '/repo/src/main.ts');
});

check('the panel state names why the list is empty', () => {
  const model = modelOnly();
  const state = (options) => model.panelState({
    hydrated: true, sessions: [], itemCount: 0, ...options,
  });
  assert.strictEqual(state({ hydrated: false }), 'loading');
  assert.strictEqual(state({ sessions: [] }), 'disconnected');
  assert.strictEqual(
    state({ sessions: [session({ state: 'failed' })] }), 'failed',
  );
  assert.strictEqual(
    state({ sessions: [session({ state: 'unavailable' })] }), 'failed',
  );
  assert.strictEqual(
    state({ sessions: [session({ state: 'indexing' })] }), 'indexing',
  );
  assert.strictEqual(
    state({ sessions: [session({ state: 'starting' })] }), 'indexing',
  );
  assert.strictEqual(state({ sessions: [session()] }), 'empty');
  assert.strictEqual(state({ sessions: [session()], itemCount: 3 }), 'ready');
});

check('one failed session among healthy ones does not declare the whole window failed', () => {
  const model = modelOnly();
  assert.strictEqual(
    model.panelState({
      hydrated: true,
      sessions: [session({ sessionId: 'a', state: 'failed' }), session({ sessionId: 'b', state: 'ready' })],
      itemCount: 0,
    }),
    'empty',
  );
});

check('stepIndex wraps in both directions and copes with an empty list', () => {
  const model = modelOnly();
  assert.strictEqual(model.stepIndex(0, -1, 1), -1);
  assert.strictEqual(model.stepIndex(3, -1, 1), 0, 'first Next selects the first row');
  assert.strictEqual(model.stepIndex(3, -1, -1), 2, 'first Previous selects the last row');
  assert.strictEqual(model.stepIndex(3, 2, 1), 0, 'Next wraps past the end');
  assert.strictEqual(model.stepIndex(3, 0, -1), 2, 'Previous wraps past the start');
});

// ===========================================================================
// Store: the live projection of Rust's snapshot
// ===========================================================================

check('the store hydrates from the snapshot commands', async () => {
  const h = storeHarness({
    initialSnapshot: snapshot([diag()], 4),
    initialSessions: [session()],
  });
  await h.store.hydrate();
  const state = h.store.getState();
  assert.strictEqual(state.hydrated, true);
  assert.strictEqual(state.revision, 4);
  assert.strictEqual(h.store.orderedItems().length, 1);
});

check('a newer revision REPLACES the store contents', async () => {
  const h = storeHarness({ initialSnapshot: snapshot([diag({ id: 'old' })], 4) });
  await h.store.hydrate();
  h.publish(diagnosticsEvent([diag({ id: 'new', message: 'new' })], { revision: 5 }));
  deepEq(h.store.orderedItems().map((i) => i.id), ['new']);
  assert.strictEqual(h.store.getState().revision, 5);
});

check('a stale revision is discarded', async () => {
  const h = storeHarness({ initialSnapshot: snapshot([diag({ id: 'current' })], 9) });
  await h.store.hydrate();
  h.publish(diagnosticsEvent([], { revision: 8 }));
  deepEq(h.store.orderedItems().map((i) => i.id), ['current']);
  assert.strictEqual(h.store.getState().revision, 9);
});

check('an empty newer publication clears the list', async () => {
  const h = storeHarness({ initialSnapshot: snapshot([diag()], 1) });
  await h.store.hydrate();
  h.publish(diagnosticsEvent([], { revision: 2 }));
  deepEq(h.store.orderedItems(), []);
});

check('subscribers are notified once per accepted publication and not for a stale one', async () => {
  const h = storeHarness({ initialSnapshot: snapshot([], 1) });
  await h.store.hydrate();
  let notifications = 0;
  h.store.subscribe(() => { notifications += 1; });
  h.publish(diagnosticsEvent([diag()], { revision: 2 }));
  assert.strictEqual(notifications, 1);
  h.publish(diagnosticsEvent([diag()], { revision: 1 }));
  assert.strictEqual(notifications, 1, 'a stale event is not a change');
});

check('a session status update re-groups without waiting for a diagnostics event', async () => {
  const h = storeHarness({
    initialSnapshot: snapshot([diag({ uri: 'file:///repo/a.ts' })], 1),
    initialSessions: [],
  });
  await h.store.hydrate();
  assert.strictEqual(h.store.view().groups[0].root, null, 'ungrouped until a root is known');
  h.publishStatus(session({ projectRootUri: 'file:///repo' }));
  assert.strictEqual(h.store.view().groups[0].root, '/repo');
});

check('a newer status revision for the same session replaces the older one', async () => {
  const h = storeHarness({ initialSnapshot: snapshot([], 1) });
  await h.store.hydrate();
  h.publishStatus(session({ revision: 5, state: 'indexing' }));
  h.publishStatus(session({ revision: 4, state: 'ready' }));
  assert.strictEqual(h.store.getState().sessions[0].state, 'indexing', 'the older status is dropped');
  h.publishStatus(session({ revision: 6, state: 'ready' }));
  assert.strictEqual(h.store.getState().sessions[0].state, 'ready');
});

check('filters live in the store so the panel and F8 traverse the same list', async () => {
  const h = storeHarness({
    initialSnapshot: snapshot([
      diag({ id: 'e', severity: 'error' }),
      diag({ id: 'w', severity: 'warning' }),
    ], 1),
  });
  await h.store.hydrate();
  h.store.setSeverityEnabled('warning', false);
  deepEq(h.store.orderedItems().map((i) => i.id), ['e']);
  h.store.setTextFilter('nothing matches this');
  deepEq(h.store.orderedItems(), []);
});

check('the store subscribes through the bridge fan-out, never to Tauri', () => {
  const source = fs.readFileSync(STORE, 'utf8');
  assert.ok(/subscribeDiagnostics/.test(source));
  assert.ok(!/__TAURI__|listenOnCurrentWindow/.test(source));
});

// ===========================================================================
// Navigation: F8 / Shift-F8
// ===========================================================================

function navHarness(options = {}) {
  const opts = options || {};
  const h = storeHarness({
    navigation: true,
    initialSnapshot: opts.snapshot === undefined ? snapshot([
      diag({ id: 'a', uri: 'file:///repo/a.ts', range: range(0, 0, 0, 3), message: 'first' }),
      diag({ id: 'b', uri: 'file:///repo/b.ts', range: range(1, 2, 1, 5), message: 'second' }),
    ], 1) : opts.snapshot,
    initialSessions: [session({ projectRootUri: 'file:///repo' })],
  });
  const opened = [];
  const revealed = [];
  const activated = [];
  const focused = [];
  const panes = new Map();
  for (const filePath of (opts.openPaths || ['/repo/a.ts', '/repo/b.ts'])) {
    const pane = {
      paneId: `pane-${filePath}`,
      tabId: `tab-${filePath}`,
      kind: 'editor',
      filePath,
      remote: null,
      view: {
        state: {
          doc: {
            lines: 10,
            length: 200,
            line: (n) => ({ number: n, from: (n - 1) * 20, to: (n - 1) * 20 + 19 }),
          },
        },
        dispatch: (spec) => revealed.push({ filePath, spec }),
        focus: () => focused.push(filePath),
      },
    };
    panes.set(pane.paneId, pane);
  }
  h.sandbox.CM6 = {
    EditorView: { scrollIntoView: (pos, config) => ({ ext: 'scrollIntoView', pos, config }) },
  };
  h.sandbox.termlabEditorService = {
    openLocalFile: (filePath) => { opened.push(filePath); return Promise.resolve(); },
  };
  // The signal a real window has an editor of its own — the same one
  // files-panel.js's openInEditor gates on. Absent in a panel host.
  if (opts.host !== true) h.sandbox.__termlabCreateEditorTab = () => {};
  const hostActions = [];
  h.sandbox.termlabPanelHostBridge = {
    HOST_ACTION_EVENTS: ['open-in-editor'],
    create: () => ({ publishAction: (event, payload) => hostActions.push({ event, payload }) }),
  };
  h.sandbox.termlabServices = { tauriClient: { invoke: () => Promise.resolve(null) } };
  const announcements = [];
  h.navigation.configure({
    paneAccess: {
      allPanes: () => panes,
      activateTab: (tabId) => activated.push(tabId),
      setFocusedPane: (paneId) => activated.push(paneId),
    },
  });
  return {
    ...h, opened, revealed, activated, focused, panes, announcements, hostActions,
    liveText: () => {
      const region = h.documentStub.body.querySelector('#problems-live-region');
      return region ? region.textContent : null;
    },
  };
}

check('F8 walks the filtered list forward and wraps', async () => {
  const h = navHarness();
  await h.store.hydrate();
  assert.strictEqual((await h.navigation.next()).id, 'a');
  assert.strictEqual((await h.navigation.next()).id, 'b');
  assert.strictEqual((await h.navigation.next()).id, 'a', 'it wraps to the top');
});

check('Shift-F8 walks backward and wraps', async () => {
  const h = navHarness();
  await h.store.hydrate();
  assert.strictEqual((await h.navigation.previous()).id, 'b', 'the first Previous starts at the end');
  assert.strictEqual((await h.navigation.previous()).id, 'a');
  assert.strictEqual((await h.navigation.previous()).id, 'b');
});

check('navigation respects the panel\'s current filters', async () => {
  const h = navHarness({
    snapshot: snapshot([
      diag({ id: 'e', uri: 'file:///repo/a.ts', severity: 'error' }),
      diag({ id: 'w', uri: 'file:///repo/b.ts', severity: 'warning' }),
    ], 1),
  });
  await h.store.hydrate();
  h.store.setSeverityEnabled('warning', false);
  assert.strictEqual((await h.navigation.next()).id, 'e');
  assert.strictEqual((await h.navigation.next()).id, 'e', 'the hidden warning is not in the walk');
});

check('an empty list announces that and navigates nowhere', async () => {
  const h = navHarness({ snapshot: snapshot([], 1) });
  await h.store.hydrate();
  assert.strictEqual(await h.navigation.next(), null);
  assert.ok(/no problems/i.test(h.liveText() || ''), h.liveText());
});

check('a cross-file target focuses the owning tab and selects the range', async () => {
  const h = navHarness();
  await h.store.hydrate();
  await h.navigation.next();
  await h.navigation.next();
  deepEq(h.opened, ['/repo/a.ts', '/repo/b.ts'], 'both go through the editor open flow');
  const last = h.revealed[h.revealed.length - 1];
  assert.strictEqual(last.filePath, '/repo/b.ts');
  // Line 2 starts at offset 20 in the stub doc; columns 2..5.
  deepEq(last.spec.selection, { anchor: 22, head: 25 });
  assert.ok(last.spec.effects, 'and the range is scrolled into view');
});

check('activation goes through editor-service, never around it', () => {
  const source = fs.readFileSync(NAVIGATION, 'utf8');
  assert.ok(/termlabEditorService/.test(source), 'the app-wide ownership protocol lives there');
  assert.ok(/openLocalFile/.test(source));
  assert.ok(!/editor_read_file|reserveDocument/.test(source), 'it must not re-implement the open flow');
});

check('the announcement names the position, the file and the message', async () => {
  const h = navHarness();
  await h.store.hydrate();
  await h.navigation.next();
  const text = h.liveText() || '';
  assert.ok(text.includes('1 of 2'), text);
  assert.ok(text.includes('a.ts'), text);
  assert.ok(text.includes('first'), text);
  assert.ok(/error/i.test(text), text);
});

check('the live region is polite, off-screen and never focused', async () => {
  const h = navHarness();
  await h.store.hydrate();
  await h.navigation.next();
  const region = h.documentStub.body.querySelector('#problems-live-region');
  assert.ok(region, 'the region exists');
  assert.strictEqual(region.getAttribute('aria-live'), 'polite');
  assert.strictEqual(region._focusCount, 0, 'announcing must not steal focus');
  assert.notStrictEqual(h.documentStub.activeElement, region);
});

check('F8 and Shift-F8 arrive as the app\'s own shortcut events, not raw key listeners', () => {
  const shortcuts = fs.readFileSync(SHORTCUT_RUNTIME, 'utf8');
  assert.ok(/editor_next_problem/.test(shortcuts), 'the router already owns the binding');
  assert.ok(/editor-next-problem/.test(shortcuts));
  const source = fs.readFileSync(NAVIGATION, 'utf8');
  assert.ok(/termlab:editor-next-problem/.test(source));
  assert.ok(/termlab:editor-previous-problem/.test(source));
  assert.ok(
    !/addEventListener\(\s*['"]key(down|up|press)['"]/.test(source),
    'a raw key listener would fire in terminal panes too',
  );
  assert.ok(!/\bdocument\.addEventListener\b/.test(source));
});

check('the shortcut events reach navigation and dispose removes them', async () => {
  const h = navHarness();
  await h.store.hydrate();
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-next-problem'));
  await Promise.resolve();
  await Promise.resolve();
  assert.strictEqual(h.store.getSelectedId(), 'a');
  h.navigation.dispose();
  h.sandbox.dispatchEvent(new h.sandbox.CustomEvent('termlab:editor-next-problem'));
  await Promise.resolve();
  await Promise.resolve();
  assert.strictEqual(h.store.getSelectedId(), 'a', 'no further movement after dispose');
});

check('a popped-out window hands the open to its parent, which owns the editor', async () => {
  const h = navHarness({ host: true });
  await h.store.hydrate();
  const item = await h.navigation.next();
  assert.ok(item, 'the walk still advances');
  deepEq(h.opened, [], 'a host has no editor of its own to open into');
  deepEq(h.hostActions, [
    { event: 'open-in-editor', payload: { kind: 'local', path: '/repo/a.ts' } },
  ]);
});

check('a target whose owner is in another window is not treated as an error', async () => {
  const h = navHarness({ openPaths: [] });
  await h.store.hydrate();
  const item = await h.navigation.next();
  assert.ok(item, 'the walk still advances');
  deepEq(h.opened, ['/repo/a.ts'], 'the open flow decides who owns it');
  deepEq(h.revealed, [], 'nothing local to reveal into');
});

// ===========================================================================
// Panel: rendering and interaction
// ===========================================================================

function panelHarness(options = {}) {
  const opts = options || {};
  const h = storeHarness({
    navigation: true,
    panel: true,
    initialSnapshot: opts.snapshot === undefined ? snapshot([
      diag({ id: 'a', uri: 'file:///repo/a.ts', severity: 'error', source: 'typescript', code: '2304', message: 'Cannot find name' }),
      diag({ id: 'b', uri: 'file:///repo/b.ts', severity: 'warning', range: range(3, 1, 3, 4), message: 'Unused' }),
    ], 1) : opts.snapshot,
    initialSessions: opts.sessions === undefined
      ? [session({ projectRootUri: 'file:///repo' })]
      : opts.sessions,
  });
  const activations = [];
  h.sandbox.termlabProblemsNavigation = {
    ...h.sandbox.termlabProblemsNavigation,
    activate: (item, options2) => {
      activations.push({ id: item && item.id, options: options2 });
      return Promise.resolve(true);
    },
  };
  const panelEl = makeElement('div');
  panelEl.id = 'problems-panel';
  h.documentStub.body.appendChild(panelEl);
  const handle = h.panelModule.init({ panelEl });
  return {
    ...h,
    panelEl,
    handle,
    activations,
    rows: () => panelEl.querySelectorAll('[data-problem-row]'),
    itemRows: () => panelEl.querySelectorAll('[data-problem-row="item"]'),
    status: () => panelEl.querySelector('.tl-problems__status'),
  };
}

check('the panel renders a group row, a file row and an item row', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  const kinds = h.rows().map((row) => row.dataset.problemRow);
  deepEq(kinds, ['group', 'file', 'item', 'file', 'item']);
});

check('an item row shows severity, position, source and code', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  const row = h.itemRows()[0];
  const text = row.textContent;
  assert.ok(text.includes('Cannot find name'), text);
  assert.ok(text.includes('1:1'), text);
  assert.ok(text.includes('typescript'), text);
  assert.ok(text.includes('2304'), text);
  assert.strictEqual(row.dataset.severity, 'error');
  assert.ok(/error/i.test(row.getAttribute('aria-label') || ''), row.getAttribute('aria-label'));
});

check('a file row shows the path relative to its project root', async () => {
  const h = panelHarness({
    snapshot: snapshot([diag({ uri: 'file:///repo/src/deep/a.ts' })], 1),
  });
  await h.store.hydrate();
  const fileRow = h.panelEl.querySelectorAll('[data-problem-row="file"]')[0];
  assert.ok(fileRow.textContent.includes('src/deep/a.ts'), fileRow.textContent);
  assert.ok(!fileRow.textContent.includes('/repo/src'), 'the root is the group, not the file');
});

check('the list is a keyboard-navigable tree with levels and a roving tabindex', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  const list = h.panelEl.querySelector('.tl-problems__list');
  assert.strictEqual(list.getAttribute('role'), 'tree');
  const rows = h.rows();
  deepEq(rows.map((r) => r.getAttribute('role')), rows.map(() => 'treeitem'));
  deepEq(rows.map((r) => r.getAttribute('aria-level')), ['1', '2', '3', '2', '3']);
  const focusable = rows.filter((r) => r.tabIndex === 0);
  assert.strictEqual(focusable.length, 1, 'exactly one row is in the tab order');
});

check('arrow keys move the selection and Enter activates the item', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  const list = h.panelEl.querySelector('.tl-problems__list');
  list._fire('keydown', { key: 'ArrowDown' });
  list._fire('keydown', { key: 'ArrowDown' });
  list._fire('keydown', { key: 'Enter' });
  await Promise.resolve();
  deepEq(h.activations.map((a) => a.id), ['a']);
});

check('Enter on a group row collapses it instead of opening anything', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  const list = h.panelEl.querySelector('.tl-problems__list');
  list._fire('keydown', { key: 'Enter' });
  deepEq(h.activations, []);
  const group = h.panelEl.querySelector('[data-problem-row="group"]');
  assert.strictEqual(group.getAttribute('aria-expanded'), 'false');
  deepEq(h.rows().map((r) => r.dataset.problemRow), ['group']);
});

check('clicking an item row activates it through the shared open/focus flow', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  h.itemRows()[1]._fire('click');
  await Promise.resolve();
  deepEq(h.activations.map((a) => a.id), ['b']);
});

check('severity toggles filter the list and show the unfiltered totals', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  const toggle = h.panelEl.querySelector('[data-severity-toggle="warning"]');
  assert.ok(toggle.textContent.includes('1'), toggle.textContent);
  assert.strictEqual(toggle.getAttribute('aria-pressed'), 'true');
  toggle._fire('click');
  deepEq(h.itemRows().map((r) => r.dataset.problemId), ['a']);
  assert.strictEqual(
    h.panelEl.querySelector('[data-severity-toggle="warning"]').getAttribute('aria-pressed'),
    'false',
  );
});

check('the text filter narrows the list', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  const input = h.panelEl.querySelector('.tl-problems__filter');
  input.value = 'unused';
  input._fire('input');
  deepEq(h.itemRows().map((r) => r.dataset.problemId), ['b']);
});

check('a live replacement from a newer revision re-renders the list', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  assert.strictEqual(h.itemRows().length, 2);
  h.publish(diagnosticsEvent(
    [diag({ id: 'c', uri: 'file:///repo/c.ts', message: 'Only one now' })],
    { revision: 2 },
  ));
  deepEq(h.itemRows().map((r) => r.dataset.problemId), ['c']);
});

check('a stale replacement leaves the rendered list alone', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  h.publish(diagnosticsEvent([], { revision: 0 }));
  assert.strictEqual(h.itemRows().length, 2);
});

check('each empty state explains itself', async () => {
  const cases = [
    { sessions: [], snapshot: snapshot([], 1), expect: /no language server/i },
    { sessions: [session({ state: 'indexing' })], snapshot: snapshot([], 1), expect: /indexing/i },
    { sessions: [session({ state: 'failed', message: 'exited (2)' })], snapshot: snapshot([], 1), expect: /failed/i },
    { sessions: [session()], snapshot: snapshot([], 1), expect: /no problems/i },
  ];
  for (const scenario of cases) {
    const h = panelHarness(scenario);
    await h.store.hydrate();
    const status = h.status();
    assert.ok(status, 'a status line is rendered');
    assert.ok(scenario.expect.test(status.textContent), `${scenario.expect} vs ${status.textContent}`);
    assert.strictEqual(h.itemRows().length, 0);
  }
});

check('a failed session names the reason it failed', async () => {
  const h = panelHarness({
    sessions: [session({ state: 'failed', message: 'rust-analyzer exited (101)' })],
    snapshot: snapshot([], 1),
  });
  await h.store.hydrate();
  assert.ok(h.status().textContent.includes('rust-analyzer exited (101)'), h.status().textContent);
});

check('the status line is gone once there are problems to show', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  assert.strictEqual(h.status(), null);
});

check('destroy unsubscribes so a docked/undocked cycle does not double-render', async () => {
  const h = panelHarness();
  await h.store.hydrate();
  h.handle.destroy();
  h.publish(diagnosticsEvent([diag({ id: 'z' })], { revision: 5 }));
  assert.strictEqual(h.itemRows().length, 2, 'the torn-down view stopped listening');
});

check('the panel configures the store itself — a panel host runs no compose runtime', () => {
  const panelSource = fs.readFileSync(PANEL, 'utf8');
  assert.ok(/store\.configure/.test(panelSource), 'otherwise a popped-out window goes stale');
  const storeSource = fs.readFileSync(STORE, 'utf8');
  assert.ok(
    /active\.configure/.test(storeSource),
    'and the bridge has to be told to start listening in that window too',
  );
});

check('the panel builds no HTML strings out of server text', () => {
  const source = fs.readFileSync(PANEL, 'utf8');
  assert.ok(!/innerHTML\s*=\s*[`'"][^`'"]*\$\{/.test(source), 'no interpolated innerHTML');
  assert.ok(!/\balert\(/.test(source), 'toasts, never alert()');
});

// ===========================================================================
// Registration and styling
// ===========================================================================

check('Problems is a built-in bottom-zone tool window', () => {
  const source = fs.readFileSync(TOOL_WINDOW_RUNTIME, 'utf8');
  const at = source.indexOf("register('problems'");
  assert.ok(at > 0, 'it is registered');
  const block = source.slice(at, at + 600);
  assert.ok(/type:\s*'built-in'/.test(block), 'built-in, so it inherits docking and pop-out');
  assert.ok(/defaultZone:\s*'bottom'/.test(block));
  assert.ok(/problemsPanel/.test(block), 'and it mounts the panel module');
});

check('Problems registers last in the bottom zone so it never steals the active tab', () => {
  const source = fs.readFileSync(TOOL_WINDOW_RUNTIME, 'utf8');
  const fileExplorer = source.indexOf("register('file-explorer'");
  const transfers = source.indexOf("register('transfer-center'");
  const problems = source.indexOf("register('problems'");
  assert.ok(fileExplorer > 0 && transfers > 0 && problems > 0);
  assert.ok(
    problems > fileExplorer && problems > transfers,
    'the first registrant activates the zone — Problems must not be it',
  );
});

// The real manager, against the same rule tool-window-runtime relies on.
// Deliberately NO sandbox.global — see the file header.
function loadManager() {
  const body = makeElement('body');
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    document: {
      body,
      createElement: (t) => makeElement(t),
      getElementById: () => null,
      querySelector: () => null,
      querySelectorAll: () => [],
      addEventListener() {},
    },
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(TOOL_WINDOW_MANAGER, 'utf8'), sandbox, {
    filename: TOOL_WINDOW_MANAGER,
  });
  return sandbox.toolWindowManager;
}

check('registering Problems does not auto-activate it in a configured bottom zone', () => {
  const twm = loadManager();
  twm.setPersistedActiveZoneWindows({ 'bottom-left': 'file-explorer' });
  twm.register('file-explorer', { title: 'SFTP', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  twm.register('problems', { title: 'Problems', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  deepEq(twm.getActiveZoneAssignments()['bottom-left'], 'file-explorer');
  assert.strictEqual(twm.isVisible('problems'), false, 'a saved layout keeps its own active window');
});

// The case the registration-order source scan above can only argue about: a
// FRESH profile, where nothing is persisted and register()'s
// "first registrant activates the zone" rule is live. Behavioural, so
// reordering the two register() calls in tool-window-runtime.js fails here
// rather than merely looking different.
check('on a fresh profile the bottom zone opens on SFTP, not on Problems', () => {
  const twm = loadManager();
  // What init() does on a layout with no `bottom_panel_visible` key: the
  // manager's own default is `false`, and register()'s auto-activation is
  // gated on the side being visible, so without this the zone would look
  // "correct" for the wrong reason.
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('file-explorer', { title: 'SFTP', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  twm.register('problems', { title: 'Problems', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  assert.strictEqual(twm.isVisible('problems'), false, 'Problems must not be what a new user finds open');
  assert.strictEqual(twm.isVisible('file-explorer'), true);
});

check('registered first on a fresh profile, Problems WOULD take the zone', () => {
  // Proves the check above is testing the rule and not an unconditional
  // "Problems never activates" — the guarantee comes from the order.
  const twm = loadManager();
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('problems', { title: 'Problems', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  twm.register('file-explorer', { title: 'SFTP', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  assert.strictEqual(twm.isVisible('problems'), true);
});

check('index.html loads the problems modules in dependency order', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(name);
  for (const name of [
    'app/features/problems/problems-model.js',
    'app/features/problems/problems-store.js',
    'app/features/problems/problems-navigation.js',
    'app/panels/problems-panel.js',
    'styles/design-system/components/problems.css',
  ]) {
    assert.ok(at(name) > 0, `${name} is not loaded`);
  }
  assert.ok(at('app/features/editor/lsp-uri.js') < at('app/features/problems/problems-model.js'));
  assert.ok(at('app/features/problems/problems-model.js') < at('app/features/problems/problems-store.js'));
  assert.ok(at('app/features/problems/problems-store.js') < at('app/features/problems/problems-navigation.js'));
  assert.ok(at('app/panels/problems-panel.js') < at('app/tool-window-runtime.js'));
});

check('problems.css styles every class the panel renders, with tokens only', () => {
  const css = fs.readFileSync(CSS, 'utf8');
  for (const name of [
    'tl-problems', 'tl-problems__toolbar', 'tl-problems__filter', 'tl-problems__list',
    'tl-problems__row', 'tl-problems__severity', 'tl-problems__status', 'tl-problems__toggle',
  ]) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(css), 'design-system components use tokens only');
  assert.ok(/focus-visible/.test(css), 'rows and groups need a strong focus state');
});

// Colour cannot be the only carrier of severity: the aria-label covers a
// screen reader, but a colour-blind sighted user reads the dots, and four
// hues is exactly the case that fails. Each severity gets a distinct glyph.
check('severity is signalled by shape as well as colour', () => {
  const css = fs.readFileSync(CSS, 'utf8');
  const glyphs = new Set();
  for (const severity of ['error', 'warning', 'information', 'hint']) {
    const rule = new RegExp(
      `\\.tl-problems__severity\\[data-severity="${severity}"\\]::before\\s*\\{[^}]*content:\\s*"([^"]+)"`,
    );
    const match = rule.exec(css);
    assert.ok(match, `${severity} has no ::before glyph — colour is its only signal`);
    glyphs.add(match[1]);
  }
  assert.strictEqual(glyphs.size, 4, 'four severities need four distinguishable glyphs');
});

check('the problems modules use no regex lookbehind', () => {
  for (const file of [MODEL, STORE, NAVIGATION, PANEL]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(
      !/\(\?<[=!]/.test(source),
      `${file} uses a lookbehind — it costs the whole file on an older WKWebView`,
    );
  }
});

// A raw control byte anywhere in a source file makes git call the whole file
// BINARY: no diff, no blame, no review on a public repo, and grep-based
// tooling skips it silently. This panel shipped one for exactly the reason
// it usually happens — a "collision-proof" sentinel string, invisible in
// every editor. The group key now comes from the model instead.
check('the problems modules contain no control bytes', () => {
  for (const file of [MODEL, STORE, NAVIGATION, PANEL, CSS]) {
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

check('the panel takes the group identity from the model rather than minting one', () => {
  const source = fs.readFileSync(PANEL, 'utf8');
  assert.ok(!/groupKey/.test(source), 'a second spelling of one identity is how the two drift');
  assert.ok(/group\.key/.test(source));
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
  console.log(`problems panel: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`problems panel: all ${ran} checks passed`);
}
