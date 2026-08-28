// Run: node scripts/tests/test_project_search_panel.mjs
//
// The project-wide Search tool window: its pure grouping helper, the panel's
// idle/searching/done/capped/no-results states, streaming-batch rendering,
// stale-search-id discarding, row activation through the editor service, and
// its registration/wiring (tool window, shortcut, menu action, palette).
//
// No jsdom in this repo (see test_tl_dialog.mjs for the precedent), so the DOM
// is stubbed to the surface these modules actually touch — the same idiom
// test_problems_panel.mjs uses. Reused verbatim from there.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const PANEL = path.join(APP, 'panels/project-search-panel.js');
const CSS = path.join(ROOT, 'styles/design-system/components/project-search.css');
const TOOL_WINDOW_MANAGER = path.join(APP, 'layout/tool-window-manager.js');

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

// --- DOM stand-in (verbatim from test_problems_panel.mjs) ---------------------
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
    // The brief's harness dispatches plain event-like objects rather than
    // calling `_fire` directly, so this thin wrapper is the standard DOM
    // entry point onto the same bubbling walk `_fire` already implements.
    dispatchEvent(event) { return this._fire(event.type, event); },
  };
  return element;
}

// --- sandbox --------------------------------------------------------------

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

// --- harness (brief, Step 1) ------------------------------------------------

function panelHarness(options) {
  const opts = options || {};
  const { sandbox, body } = load([PANEL]);
  const invoked = [];
  let listener = null;
  const panelEl = sandbox.document.createElement('div');
  body.appendChild(panelEl);
  const opens = [];
  sandbox.termlabEditorService = {
    openLocalFileAt: (p, range, o) => { opens.push([p, range, o]); return Promise.resolve({ status: 'opened' }); },
  };
  const handle = sandbox.projectSearchPanel.init({
    panelEl,
    invoke: async (cmd, args) => {
      invoked.push([cmd, args]);
      if (cmd === 'project_search') return 'search-1';
      return null;
    },
    listen: (name, fn) => { if (name === 'project-search-results') listener = fn; return () => {}; },
  });
  return {
    sandbox, body, panelEl, handle, invoked, opens,
    publish: (payload) => listener && listener({ payload }),
    input: () => panelEl.querySelector('.tl-project-search__input'),
    status: () => {
      const node = panelEl.querySelector('.tl-project-search__status');
      return node ? node.textContent : null;
    },
    rows: () => Array.from(panelEl.querySelectorAll('[data-search-row]')),
  };
}

const hit = (relativePath, line, preview) => ({
  path: '/repo/' + relativePath, relativePath, line, column: 1, preview,
});

check('groupByFile keeps file order and gathers each file once', () => {
  const { sandbox } = panelHarness();
  const grouped = sandbox.projectSearchPanel.groupByFile([
    hit('a.rs', 1, 'x'), hit('b.rs', 2, 'y'), hit('a.rs', 5, 'z'),
  ]);
  deepEq(grouped.map((g) => [g.relativePath, g.matches.length]), [['a.rs', 2], ['b.rs', 1]]);
});

check('the empty state is explicit before anything is typed', () => {
  const h = panelHarness();
  assert.ok(h.status().includes('Search'), `expected an inviting empty state, got ${h.status()}`);
  deepEq(h.invoked, [], 'nothing is searched until asked');
});

check('submitting runs a search and shows a searching state', () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  deepEq(h.invoked, [['project_search', { query: 'needle', caseSensitive: false }]]);
  assert.ok(h.status().includes('Searching'), `expected a searching state, got ${h.status()}`);
});

check('the case toggle is carried into the query', () => {
  const h = panelHarness();
  const toggle = h.panelEl.querySelector('[data-search-case]');
  toggle.dispatchEvent({ type: 'click', target: toggle });
  h.input().value = 'Needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  deepEq(h.invoked, [['project_search', { query: 'Needle', caseSensitive: true }]]);
});

// The mock `invoke` resolves the new search id asynchronously (real Tauri
// IPC always does), while `runSearch` only records it once that promise
// settles. In the app itself the walker is spawned only after the command
// returns, so the id is always confirmed before any batch can arrive — but a
// synchronous test harness would otherwise race ahead of that microtask and
// see every batch discarded as unrecognized. One microtask flush reproduces
// the real ordering before publishing starts.
const flush = () => Promise.resolve();

check('batches render grouped by file and the terminal event reports the count', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  await flush();
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'one'), hit('b.rs', 3, 'three')], done: false, capped: false });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 9, 'nine')], done: true, capped: false });
  const labels = h.rows().map((r) => r.getAttribute('data-search-row'));
  deepEq(labels, ['file', 'match', 'match', 'file', 'match']);
  assert.ok(h.status().includes('3'), `the terminal state counts the matches, got ${h.status()}`);
});

check('the capped terminal event says so', async () => {
  const h = panelHarness();
  h.input().value = 'e';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  await flush();
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'e')], done: true, capped: true });
  assert.ok(h.status().includes('first 1000'), `expected the cap wording, got ${h.status()}`);
});

check('no results is its own state, not an empty list', async () => {
  const h = panelHarness();
  h.input().value = 'zzz';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  await flush();
  h.publish({ searchId: 'search-1', matches: [], done: true, capped: false });
  assert.ok(h.status().includes('No results'), `got ${h.status()}`);
});

check('a superseded search id is ignored', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  // Flushed first so this genuinely proves id-mismatch rejection against the
  // CONFIRMED current id ('search-1'), not an accident of activeSearchId
  // still being unset.
  await flush();
  h.publish({ searchId: 'search-0', matches: [hit('stale.rs', 1, 'stale')], done: true, capped: false });
  deepEq(h.rows(), [], 'results from a cancelled query must never land');
});

check('activating a match opens the file at the line through the editor service', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  await flush();
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 4, 'needle')], done: true, capped: false });
  const match = h.rows().find((r) => r.getAttribute('data-search-row') === 'match');
  match.dispatchEvent({ type: 'click', target: match });
  deepEq(h.opens, [[
    '/repo/a.rs',
    { start: { line: 3, character: 0 }, end: { line: 3, character: 0 } },
    { focus: true },
  ]], 'LSP positions are 0-based; a 1-based search line converts once, here');
});

check('Search never steals the bottom zone from SFTP on a fresh profile', () => {
  const { sandbox } = load([TOOL_WINDOW_MANAGER]);
  const twm = sandbox.toolWindowManager;
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('file-explorer', { title: 'Project', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  twm.register('problems', { title: 'Problems', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  twm.register('project-search', { title: 'Search', type: 'built-in', defaultZone: 'bottom', renderFn() {} });
  assert.strictEqual(twm.isVisible('project-search'), false, 'Search must not be what a user finds open');
  assert.strictEqual(twm.isVisible('file-explorer'), true);
});

check('the runtime registers Search last, and only for a project window', () => {
  const src = fs.readFileSync(path.join(APP, 'tool-window-runtime.js'), 'utf8');
  assert.ok(src.includes("register('project-search'"), 'the Search tool window is registered');
  const problems = src.indexOf("register('problems'");
  const search = src.indexOf("register('project-search'");
  assert.ok(problems < search, 'Search registers after the existing bottom-zone registrants');
  const guard = src.slice(0, search).lastIndexOf('if (projectRoot)');
  assert.ok(guard > problems, 'the registration is guarded on the window having a project');
});

check('cmd+shift+f is the shipped default and routes to one action', () => {
  const shortcuts = fs.readFileSync(path.join(APP, 'shortcut-runtime.js'), 'utf8');
  assert.ok(shortcuts.includes("search_in_project: 'search-in-project'"), 'the core table carries the action');
  const actions = fs.readFileSync(path.join(APP, 'menu-actions.js'), 'utf8');
  assert.ok(actions.includes("action === 'search-in-project'"), 'the action is handled');
  assert.ok(actions.includes("activate('project-search')"), 'it activates the Search tool window');
  const palette = fs.readFileSync(path.join(APP, 'command-palette-runtime.js'), 'utf8');
  assert.ok(palette.includes("'core:search-in-project'"), 'the palette exposes Search in Project');
  const config = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_core/src/config/termlab.rs'), 'utf8');
  assert.ok(config.includes('search_in_project'), 'the keyboard config has the binding');
  assert.ok(config.includes('"cmd+shift+f"'), 'shipped default is cmd+shift+f');
});

check('project-search.css styles every class the panel renders, with tokens only', () => {
  const css = fs.readFileSync(CSS, 'utf8');
  for (const name of [
    'tl-project-search', 'tl-project-search__toolbar', 'tl-project-search__input',
    'tl-project-search__list', 'tl-project-search__row', 'tl-project-search__status',
    'tl-project-search__where', 'tl-project-search__preview',
  ]) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(css), 'design-system components use tokens only');
  assert.ok(/focus-visible/.test(css), 'rows need a strong focus state');
});

check('the search panel uses no regex lookbehind and no control bytes', () => {
  for (const file of [PANEL, CSS]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(!/\(\?<[=!]/.test(source), `${file} uses a lookbehind`);
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
        `${file}: control byte at offset ${i}`);
    }
  }
});

check('index.html loads the search panel and its stylesheet before the runtime', () => {
  const html = fs.readFileSync(path.join(ROOT, 'index.html'), 'utf8');
  assert.ok(html.indexOf('app/panels/project-search-panel.js') > 0);
  assert.ok(html.indexOf('styles/design-system/components/project-search.css') > 0);
  assert.ok(html.indexOf('app/panels/project-search-panel.js') < html.indexOf('app/tool-window-runtime.js'));
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
  console.log(`project search panel: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`project search panel: all ${ran} checks passed`);
}
