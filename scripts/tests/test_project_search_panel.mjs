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
  const { sandbox, body, documentStub } = load([PANEL]);
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
    list: () => panelEl.querySelector('.tl-project-search__list'),
    activeElement: () => documentStub.activeElement,
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
// IPC always does), and so do all the checks below that publish before
// awaiting anything: `project_search`'s reply and the events its walker
// thread emits travel independent channels with NO ordering guarantee (the
// thread is spawned before the command returns its id — see
// search.rs:project_search) so a real batch, even a terminal one, can and
// does arrive before the invoke that started it has resolved in this window.
// Publishing immediately after the Enter dispatch, before any await,
// deliberately exercises that early-arrival path; the flush afterward is
// what lets the id-confirmation microtask (and the buffered replay it
// triggers) actually run before assertions read the DOM.
const flush = () => Promise.resolve();

check('batches render grouped by file and the terminal event reports the count', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'one'), hit('b.rs', 3, 'three')], done: false, capped: false });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 9, 'nine')], done: true, capped: false });
  await flush();
  const labels = h.rows().map((r) => r.getAttribute('data-search-row'));
  deepEq(labels, ['file', 'match', 'match', 'file', 'match']);
  assert.ok(h.status().includes('3'), `the terminal state counts the matches, got ${h.status()}`);
});

check('the capped terminal event says so, with a data-state distinct from a plain done', async () => {
  const h = panelHarness();
  h.input().value = 'e';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'e')], done: true, capped: true });
  await flush();
  assert.ok(h.status().includes('first 1000'), `expected the cap wording, got ${h.status()}`);
  const statusNode = h.panelEl.querySelector('.tl-project-search__status');
  assert.strictEqual(statusNode.getAttribute('data-state'), 'capped');

  // The un-capped case renders a DIFFERENT data-state for the exact same
  // terminal ('done') condition, so a capped result is not just worded
  // differently — it is a visually distinct state a user skimming (not
  // reading) the status line can still tell apart. Deleting the capped
  // branch of renderStatus's setAttribute call must fail this, even though
  // the wording assertion above would not notice.
  const h2 = panelHarness();
  h2.input().value = 'needle';
  h2.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h2.input() });
  h2.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'needle')], done: true, capped: false });
  await flush();
  const status2 = h2.panelEl.querySelector('.tl-project-search__status');
  assert.strictEqual(status2.getAttribute('data-state'), 'done');
  assert.notStrictEqual(status2.getAttribute('data-state'), statusNode.getAttribute('data-state'));
});

check('no results is its own state, not an empty list', async () => {
  const h = panelHarness();
  h.input().value = 'zzz';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-1', matches: [], done: true, capped: false });
  await flush();
  assert.ok(h.status().includes('No results'), `got ${h.status()}`);
});

check('a superseded search id is ignored', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-0', matches: [hit('stale.rs', 1, 'stale')], done: true, capped: false });
  // Flushed so this genuinely proves id-mismatch rejection against the
  // CONFIRMED current id ('search-1') via the early-buffer replay path, not
  // an accident of activeSearchId still being unset.
  await flush();
  deepEq(h.rows(), [], 'results from a cancelled query must never land');
});

check('activating a match opens the file at the line through the editor service', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 4, 'needle')], done: true, capped: false });
  await flush();
  const match = h.rows().find((r) => r.getAttribute('data-search-row') === 'match');
  match.dispatchEvent({ type: 'click', target: match });
  deepEq(h.opens, [[
    '/repo/a.rs',
    { start: { line: 3, character: 0 }, end: { line: 3, character: 0 } },
    { focus: true },
  ]], 'LSP positions are 0-based; a 1-based search line converts once, here');
});

// --- keyboard navigation (Task 9 fix round 1) --------------------------------
//
// The Problems panel's roving-tabindex convention: arrows/Home/End move it,
// Enter activates whatever row currently holds it (not event.target), a
// click uses .closest() to resolve the row. Search tracks the active row by a
// stable key rather than an index — see the file header on
// project-search-panel.js for why an index would silently point at the wrong
// row once a streaming batch inserts rows in the middle of the flat list.

// `done` defaults to true (the ordinary case for these checks); the
// identity-preservation check below passes false so it can publish a SECOND
// batch of the same still-running search afterward.
async function twoFileHarness(done) {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({
    searchId: 'search-1',
    matches: [hit('a.rs', 1, 'one'), hit('c.rs', 5, 'five')],
    done: done === undefined ? true : done,
    capped: false,
  });
  await flush();
  return h; // rows: [file a.rs, match a.rs:1, file c.rs, match c.rs:5]
}

check('the results list has a roving tabindex, defaulting to the first row', async () => {
  const h = await twoFileHarness();
  deepEq(h.rows().map((r) => r.tabIndex), [0, -1, -1, -1]);
});

check('ArrowDown/ArrowUp move the roving tabindex and focus', async () => {
  const h = await twoFileHarness();
  h.list().dispatchEvent({ type: 'keydown', key: 'ArrowDown', preventDefault() {} });
  deepEq(h.rows().map((r) => r.tabIndex), [-1, 0, -1, -1]);
  assert.strictEqual(h.activeElement(), h.rows()[1], 'the newly-active row actually receives focus');

  h.list().dispatchEvent({ type: 'keydown', key: 'ArrowUp', preventDefault() {} });
  deepEq(h.rows().map((r) => r.tabIndex), [0, -1, -1, -1]);
  assert.strictEqual(h.activeElement(), h.rows()[0]);
});

check('ArrowUp at the top and ArrowDown at the bottom stay put rather than wrapping', async () => {
  const h = await twoFileHarness();
  h.list().dispatchEvent({ type: 'keydown', key: 'ArrowUp', preventDefault() {} });
  deepEq(h.rows().map((r) => r.tabIndex), [0, -1, -1, -1]);
  for (let i = 0; i < 3; i += 1) {
    h.list().dispatchEvent({ type: 'keydown', key: 'ArrowDown', preventDefault() {} });
  }
  deepEq(h.rows().map((r) => r.tabIndex), [-1, -1, -1, 0]);
  h.list().dispatchEvent({ type: 'keydown', key: 'ArrowDown', preventDefault() {} });
  deepEq(h.rows().map((r) => r.tabIndex), [-1, -1, -1, 0], 'the last row stays active, it does not wrap');
});

check('Home and End jump to the first and last row', async () => {
  const h = await twoFileHarness();
  h.list().dispatchEvent({ type: 'keydown', key: 'End', preventDefault() {} });
  deepEq(h.rows().map((r) => r.tabIndex), [-1, -1, -1, 0]);
  h.list().dispatchEvent({ type: 'keydown', key: 'Home', preventDefault() {} });
  deepEq(h.rows().map((r) => r.tabIndex), [0, -1, -1, -1]);
});

check('Enter activates the row holding the roving tabindex, not event.target', async () => {
  const h = await twoFileHarness();
  h.list().dispatchEvent({ type: 'keydown', key: 'ArrowDown', preventDefault() {} }); // -> match a.rs:1
  // event.target deliberately points somewhere else, to prove Enter reads
  // the tracked active row rather than trusting the DOM event's target.
  h.list().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.list() });
  deepEq(h.opens, [[
    '/repo/a.rs',
    { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } },
    { focus: true },
  ]]);
});

check('Enter on a file row (no match) is inert, not a crash', async () => {
  const h = await twoFileHarness();
  h.list().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.list() });
  deepEq(h.opens, [], 'the first row is a file row; Enter on it opens nothing');
});

check('the active row survives a re-render from the next streamed batch, by identity not index', async () => {
  const h = await twoFileHarness(false); // still "searching" — rows: [file a.rs, match a.rs:1, file c.rs, match c.rs:5]
  h.list().dispatchEvent({ type: 'keydown', key: 'ArrowDown', preventDefault() {} });
  h.list().dispatchEvent({ type: 'keydown', key: 'ArrowDown', preventDefault() {} });
  const activeBefore = h.rows()[2];
  assert.strictEqual(activeBefore.getAttribute('data-search-row'), 'file');
  assert.ok(activeBefore.textContent.includes('c.rs'), activeBefore.textContent);

  // The SAME search's next batch adds another hit to a.rs's group, which
  // sits ahead of c.rs's rows in the flat list — everything from c.rs onward
  // shifts down by one index. An index-based "active row" would now point at
  // the newly-inserted row instead of the one the user was actually on.
  h.publish({
    searchId: 'search-1',
    matches: [hit('a.rs', 2, 'two')],
    done: true,
    capped: false,
  });

  const rows = h.rows();
  deepEq(rows.map((r) => r.getAttribute('data-search-row')), ['file', 'match', 'match', 'file', 'match']);
  const activeAfter = rows.find((r) => r.tabIndex === 0);
  assert.ok(activeAfter, 'exactly one row is still in the tab order');
  assert.strictEqual(activeAfter.getAttribute('data-search-row'), 'file');
  assert.ok(activeAfter.textContent.includes('c.rs'), `expected the c.rs row still active, got ${activeAfter.textContent}`);
  assert.strictEqual(rows.indexOf(activeAfter), 3, 'its INDEX did shift — identity is what survived');
});

// --- early-arrival buffering (Task 9 fix round 1) ----------------------------
//
// Both shapes the review called out, covered directly: a lone terminal batch
// that beats the invoke's resolution, and a batch that arrives early followed
// by more that arrive normally (after confirmation).

check('a terminal batch arriving before the search id resolves still renders', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  // Published synchronously — no await has happened yet, so the mock
  // invoke's promise cannot possibly have resolved. This is the exact shape
  // search.rs's own thread-spawned-before-the-reply hazard produces.
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'one')], done: true, capped: false });
  assert.strictEqual(h.rows().length, 0, 'nothing renders yet — the id is still unconfirmed');
  assert.ok(h.status().includes('Searching'), 'and the status has not jumped to done early either');
  await flush();
  const labels = h.rows().map((r) => r.getAttribute('data-search-row'));
  deepEq(labels, ['file', 'match']);
  assert.ok(h.status().includes('1'), `expected the buffered batch to have landed, got ${h.status()}`);
});

check('a batch that arrives early is followed by more after resolution; both apply', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'one')], done: false, capped: false }); // early
  await flush(); // confirms the id; the early batch replays now
  h.publish({ searchId: 'search-1', matches: [hit('b.rs', 3, 'three')], done: true, capped: false }); // ordinary
  const labels = h.rows().map((r) => r.getAttribute('data-search-row'));
  deepEq(labels, ['file', 'match', 'file', 'match']);
  assert.ok(h.status().includes('2'), `expected both batches counted, got ${h.status()}`);
});

check('clearing the box without pressing Enter cancels a running search', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  await flush();
  h.publish({ searchId: 'search-1', matches: [hit('a.rs', 1, 'one')], done: false, capped: false });
  assert.ok(h.rows().length > 0, 'a batch is on screen before the clear');

  h.input().value = '';
  h.input().dispatchEvent({ type: 'input', target: h.input() });

  assert.ok(
    h.invoked.some(([cmd]) => cmd === 'project_search_cancel'),
    'clearing the box must cancel the backend walk, not just blank the panel',
  );
  assert.strictEqual(h.rows().length, 0, 'the panel goes back to nothing shown');
  assert.ok(h.status().includes('Search'), `expected the idle state, got ${h.status()}`);

  // A late results event for the search that was just cancelled must not
  // resurrect it: activeSearchId is null now, so this is a plain id
  // mismatch, same as any other superseded batch.
  h.publish({ searchId: 'search-1', matches: [hit('late.rs', 1, 'late')], done: true, capped: false });
  await flush();
  assert.strictEqual(h.rows().length, 0, 'a stale reply after a cancel must not repopulate the list');
});

check('cancelling before the in-flight invoke resolves stops that resolution from resurrecting the search', async () => {
  const h = panelHarness();
  h.input().value = 'needle';
  h.input().dispatchEvent({ type: 'keydown', key: 'Enter', preventDefault() {}, target: h.input() });
  // Cleared immediately — the mock invoke's promise has not resolved yet, so
  // this exercises the searchGeneration guard specifically, not the simpler
  // activeSearchId-is-null case the check above already covers.
  h.input().value = '';
  h.input().dispatchEvent({ type: 'input', target: h.input() });
  await flush(); // lets the now-superseded invoke() resolution run
  assert.strictEqual(h.rows().length, 0);
  assert.ok(h.status().includes('Search'), `expected the idle state to have stuck, got ${h.status()}`);

  // And a batch tagged with the id that resolution WOULD have confirmed must
  // still not land — the search it belonged to no longer exists.
  h.publish({ searchId: 'search-1', matches: [hit('late.rs', 1, 'late')], done: true, capped: false });
  await flush();
  assert.strictEqual(h.rows().length, 0);
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
