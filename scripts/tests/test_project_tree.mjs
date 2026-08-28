// Run: node scripts/tests/test_project_tree.mjs
//
// The project tree — the lazy, keyboard-navigable file tree that project
// windows render in the Files panel: pure sort/join helpers, lazy per-
// directory listing and caching, keyboard navigation, the missing-root
// state, and the context-menu hook.
//
// No jsdom (see test_tl_dialog.mjs for the precedent). Deliberately does NOT
// define `sandbox.global` — see test_problems_panel.mjs's note. The DOM
// stand-in below is copied from test_problems_panel.mjs (makeElement,
// selectorParts, dataName, and the load() sandbox builder) with one
// addition: an element-level `dispatchEvent`, since this suite dispatches
// events directly on row/list elements rather than through `_fire`.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const TREE = path.join(APP, 'panels/project-tree.js');
const CSS = path.join(ROOT, 'styles/design-system/components/project-tree.css');
const INDEX_HTML = path.join(ROOT, 'index.html');

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
    // Bubbles, because the tree delegates click/keydown/contextmenu handling
    // to the list and a stub that fired only on the target would let a
    // per-row-handler regression pass while the real DOM behaved differently.
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
    // Not part of the copied stand-in: this suite dispatches events directly
    // on elements (`row.dispatchEvent({ type: 'click', ... })`) rather than
    // through `_fire`, so `dispatchEvent` is the entry point tests use.
    dispatchEvent(event) {
      return this._fire(event && event.type, event);
    },
    _listeners(name) { return (listeners.get(name) || []).slice(); },
    _listenerCount(name) { return (listeners.get(name) || []).length; },
  };
  return element;
}

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

function treeHarness(options) {
  const opts = options || {};
  const { sandbox, body, documentStub } = load([TREE]);
  const listed = [];
  const dirs = opts.dirs || {};
  const invoke = async (cmd, args) => {
    if (cmd !== 'local_list_dir') throw new Error('unexpected command ' + cmd);
    listed.push(args.path);
    if (!(args.path in dirs)) throw new Error('permission denied');
    return dirs[args.path];
  };
  const opened = [];
  const errors = [];
  const handle = sandbox.termlabProjectTree.create({
    invoke,
    root: opts.root || '/repo',
    showHidden: opts.showHidden === true,
    onOpenFile: (p) => { opened.push(p); },
    onContextMenu: () => {},
    toastError: (title, msg) => { errors.push(msg); },
  });
  body.appendChild(handle.element);
  return { sandbox, handle, listed, opened, errors, body, documentStub };
}

const entry = (name, isDir) => ({ name, is_dir: isDir, size: 0, modified: null, permissions: null });

check('sortEntries: dirs first, alphabetical, case-insensitive', () => {
  const { sandbox } = treeHarness({ dirs: { '/repo': [] } });
  const names = sandbox.termlabProjectTree.sortEntries([
    entry('zeta.rs', false), entry('Beta', true), entry('alpha.rs', false), entry('apple', true),
  ], true).map((e) => e.name);
  deepEq(names, ['apple', 'Beta', 'alpha.rs', 'zeta.rs']);
});

check('sortEntries honours the hidden-files convention', () => {
  const { sandbox } = treeHarness({ dirs: { '/repo': [] } });
  const visible = sandbox.termlabProjectTree.sortEntries(
    [entry('.git', true), entry('src', true), entry('.env', false), entry('a.rs', false)], false,
  ).map((e) => e.name);
  deepEq(visible, ['src', 'a.rs'], 'dotfiles are hidden by default');
  const all = sandbox.termlabProjectTree.sortEntries(
    [entry('.git', true), entry('src', true)], true,
  ).map((e) => e.name);
  deepEq(all, ['.git', 'src']);
});

check('the tree lists the root once and nothing below it until expanded', async () => {
  const h = treeHarness({
    dirs: {
      '/repo': [entry('src', true), entry('README.md', false)],
      '/repo/src': [entry('main.rs', false)],
    },
  });
  await h.handle.refreshAll();
  deepEq(h.listed, ['/repo'], 'lazy: only the root is listed');
  const paths = h.handle.rows().map((n) => n.path);
  deepEq(paths, ['/repo/src', '/repo/README.md']);
  await h.handle.expand('/repo/src');
  deepEq(h.listed, ['/repo', '/repo/src'], 'a directory lists when first expanded');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/src', '/repo/src/main.rs', '/repo/README.md']);
});

check('collapsing keeps the listing and re-expanding does not re-list', async () => {
  const h = treeHarness({
    dirs: { '/repo': [entry('src', true)], '/repo/src': [entry('main.rs', false)] },
  });
  await h.handle.refreshAll();
  await h.handle.expand('/repo/src');
  h.handle.collapse('/repo/src');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/src']);
  await h.handle.expand('/repo/src');
  deepEq(h.listed, ['/repo', '/repo/src'], 'the cached listing is reused');
});

check('an unreadable directory toasts and collapses, and the rest keeps working', async () => {
  const h = treeHarness({
    dirs: { '/repo': [entry('secret', true), entry('src', true)], '/repo/src': [entry('a.rs', false)] },
  });
  await h.handle.refreshAll();
  await h.handle.expand('/repo/secret');
  assert.strictEqual(h.errors.length, 1, 'the failure is reported');
  assert.ok(h.errors[0].includes('/repo/secret'), 'the toast names the directory');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/secret', '/repo/src'], 'the row stays, collapsed');
  await h.handle.expand('/repo/src');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/secret', '/repo/src', '/repo/src/a.rs']);
});

check('Enter and click open a file through the injected opener', async () => {
  const h = treeHarness({ dirs: { '/repo': [entry('a.rs', false)] } });
  await h.handle.refreshAll();
  const row = h.handle.element.querySelector('[data-tree-path="/repo/a.rs"]');
  assert.ok(row, 'the file row is rendered');
  row.dispatchEvent({ type: 'click', target: row });
  deepEq(h.opened, ['/repo/a.rs']);
});

check('arrows move, Right expands, Left collapses', async () => {
  const h = treeHarness({
    dirs: { '/repo': [entry('src', true), entry('a.rs', false)], '/repo/src': [entry('m.rs', false)] },
  });
  await h.handle.refreshAll();
  const list = h.handle.element.querySelector('.tl-project-tree__list');
  const key = (k) => list.dispatchEvent({ type: 'keydown', key: k, preventDefault() {}, target: list });
  key('ArrowDown');
  assert.strictEqual(h.handle.activePath(), '/repo/src');
  key('ArrowRight');
  await h.handle.settled();
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/src', '/repo/src/m.rs', '/repo/a.rs']);
  key('ArrowLeft');
  deepEq(h.handle.rows().map((n) => n.path), ['/repo/src', '/repo/a.rs']);
  key('ArrowDown');
  assert.strictEqual(h.handle.activePath(), '/repo/a.rs');
  key('Enter');
  deepEq(h.opened, ['/repo/a.rs']);
});

check('a right-click hands the node to the context-menu hook', async () => {
  const seen = [];
  const { sandbox, body } = load([TREE]);
  const handle = sandbox.termlabProjectTree.create({
    invoke: async () => [entry('a.rs', false)],
    root: '/repo',
    showHidden: false,
    onOpenFile: () => {},
    onContextMenu: (event, node) => { seen.push(node); },
    toastError: () => {},
  });
  body.appendChild(handle.element);
  await handle.refreshAll();
  const row = handle.element.querySelector('[data-tree-path="/repo/a.rs"]');
  row.dispatchEvent({ type: 'contextmenu', target: row, preventDefault() {}, clientX: 1, clientY: 2 });
  deepEq(seen, [{ path: '/repo/a.rs', name: 'a.rs', isDir: false, parentPath: '/repo' }]);
});

check('setMissing renders the vanished-root state with a reopen action', async () => {
  const h = treeHarness({ dirs: { '/repo': [entry('a.rs', false)] } });
  await h.handle.refreshAll();
  h.handle.setMissing(true);
  const missing = h.handle.element.querySelector('.tl-project-tree__missing');
  assert.ok(missing, 'the missing state renders');
  assert.ok(missing.textContent.includes('missing'), 'it says what is wrong');
  assert.ok(h.handle.element.querySelector('[data-tree-action="reopen"]'), 'and offers a way out');
  h.handle.setMissing(false);
  assert.strictEqual(h.handle.element.querySelector('.tl-project-tree__missing'), null);
});

check('project-tree.css styles every class the tree renders, with tokens only', () => {
  const css = fs.readFileSync(CSS, 'utf8');
  for (const name of [
    'tl-project-tree', 'tl-project-tree__toolbar', 'tl-project-tree__list',
    'tl-project-tree__row', 'tl-project-tree__twisty', 'tl-project-tree__label',
    'tl-project-tree__missing',
  ]) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(css), 'design-system components use tokens only');
  assert.ok(/focus-visible/.test(css), 'tree rows need a strong focus state');
});

check('the tree module uses no regex lookbehind and no control bytes', () => {
  for (const file of [TREE, CSS]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(!/\(\?<[=!]/.test(source), `${file} uses a lookbehind`);
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
        `${file}: control byte at offset ${i}`);
    }
  }
});

check('index.html loads the tree module and its stylesheet', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  assert.ok(html.indexOf('app/panels/project-tree.js') > 0);
  assert.ok(html.indexOf('styles/design-system/components/project-tree.css') > 0);
  assert.ok(html.indexOf('app/panels/project-tree.js') < html.indexOf('app/panels/files-panel.js'),
    'files-panel consumes the tree, so the tree must load first');
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
  console.log(`project tree: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`project tree: all ${ran} checks passed`);
}
