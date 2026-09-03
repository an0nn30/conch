// Run: node scripts/tests/test_preview_toolbar.mjs
//
// The visible route into Editor / Split / Preview.
//
// The preview shipped reachable only by cmd+shift+y, which is the same as not
// shipping it for anyone who does not already know it exists. This pins the
// segmented control that fixes that: that it exists exactly when a preview
// does, that it says which mode the pane is in, that a click lands in the same
// code path the keystroke does, and that closing the pane leaves nothing
// behind.
//
// The modules under test are features/editor/preview/preview-toolbar.js and
// the mount/teardown seam in features/editor/editor-pane.js. The parser and
// the frame are stubbed at their factory globals, exactly as in
// test_preview_wiring.mjs.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const FILES = {
  languageMap: path.join(ROOT, 'app/features/editor/language-map.js'),
  previewMode: path.join(ROOT, 'app/features/editor/preview/preview-mode.js'),
  images: path.join(ROOT, 'app/features/editor/preview/image-resolver.js'),
  controller: path.join(ROOT, 'app/features/editor/preview/preview-controller.js'),
  toolbar: path.join(ROOT, 'app/features/editor/preview/preview-toolbar.js'),
  editorPane: path.join(ROOT, 'app/features/editor/editor-pane.js'),
};

// --- a minimal element ------------------------------------------------------
//
// Richer than test_preview_wiring.mjs's: the toolbar sets attributes and binds
// listeners, and a leaked listener is one of the things this file is here to
// catch, so both are recorded rather than swallowed.
function makeEl(tag) {
  const classes = new Set();
  const el = {
    tagName: String(tag).toUpperCase(),
    children: [],
    parentNode: null,
    attrs: {},
    listeners: {},
    innerHTML: '',
    classList: {
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      contains: (c) => classes.has(c),
    },
    classNames: () => [...classes],
    appendChild(child) { child.parentNode = this; this.children.push(child); return child; },
    removeChild(child) {
      this.children = this.children.filter((c) => c !== child);
      child.parentNode = null;
      return child;
    },
    setAttribute(name, value) { this.attrs[name] = String(value); },
    getAttribute(name) {
      return Object.prototype.hasOwnProperty.call(this.attrs, name) ? this.attrs[name] : null;
    },
    hasAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attrs, name); },
    removeAttribute(name) { delete this.attrs[name]; },
    addEventListener(type, fn) { (this.listeners[type] || (this.listeners[type] = [])).push(fn); },
    removeEventListener(type, fn) {
      this.listeners[type] = (this.listeners[type] || []).filter((f) => f !== fn);
    },
    listenerCount() {
      return Object.keys(this.listeners)
        .reduce((n, type) => n + this.listeners[type].length, 0);
    },
  };
  Object.defineProperty(el, 'className', {
    get: () => [...classes].join(' '),
    set: (value) => {
      classes.clear();
      String(value).split(/\s+/).filter(Boolean).forEach((c) => classes.add(c));
    },
  });
  return el;
}

let defaultsPrevented = 0;
function fire(el, type) {
  const event = { type, preventDefault() { defaultsPrevented += 1; } };
  (el.listeners[type] || []).slice().forEach((fn) => fn(event));
}

// --- CM6 stand-in, only what editor-pane.js touches -------------------------
function makeCM6() {
  class Compartment {
    of(ext) { return { compartment: true, contents: ext }; }
    reconfigure(ext) { return { reconfigure: true, contents: ext }; }
  }
  function EditorView(config) {
    this.config = config;
    this.state = config.state;
    this.dispatch = () => {};
    this.destroy = () => { this.destroyed = true; };
  }
  EditorView.updateListener = { of: (fn) => ({ ext: 'updateListener', fn }) };
  EditorView.theme = (spec) => ({ ext: 'theme', spec });
  const tagged = (name) => () => ({ ext: name });
  return {
    Compartment,
    EditorView,
    EditorState: { create: (spec) => ({ spec, doc: { toString: () => spec.doc } }) },
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
}

function makeHarness(options = {}) {
  const record = { contents: [], destroyed: 0 };
  const sandbox = {
    console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap, WeakSet,
    Array, Object, String, Number,
    document: { createElement: makeEl },
    requestAnimationFrame: (fn) => setTimeout(fn, 0),
    cancelAnimationFrame: (handle) => clearTimeout(handle),
  };
  sandbox.window = sandbox;
  sandbox.CM6 = makeCM6();
  sandbox.termlabServices = { tauriClient: { invoke: async () => null } };

  vm.createContext(sandbox);
  for (const key of ['languageMap', 'previewMode', 'images', 'controller', 'toolbar', 'editorPane']) {
    vm.runInContext(fs.readFileSync(FILES[key], 'utf8'), sandbox, { filename: FILES[key] });
  }

  if (options.withBundle !== false) {
    sandbox.MDLib = {};
    sandbox.termlabMarkdownRenderer = {
      createRenderer: () => ({ render: (md) => `<p>${md}</p>` }),
    };
  }
  sandbox.termlabPreviewFrame = {
    createFrame(host) {
      const element = makeEl('iframe');
      element.contentDocument = null;
      host.appendChild(element);
      return {
        element,
        setContent(html) { record.contents.push(html); },
        scrollToLine() {},
        destroy() { record.destroyed += 1; },
      };
    },
  };

  const container = makeEl('div');
  container.className = 'terminal-pane';
  const editorHost = makeEl('div');
  editorHost.className = 'editor-pane-host';
  container.appendChild(editorHost);

  const pane = sandbox.termlabEditorPane;
  const view = pane.createEditorView(editorHost, { doc: '# hello', filename: 'notes.md' });
  view.dom = editorHost.appendChild(makeEl('div'));
  view.scrollDOM = { scrollTop: 0, addEventListener() {}, removeEventListener() {} };

  const toolbarEl = () => container.children.find(
    (c) => c.classList && c.classList.contains('md-preview-toolbar'),
  ) || null;
  const buttons = () => {
    const el = toolbarEl();
    return el ? el.children : [];
  };
  const buttonFor = (mode) => buttons().find((b) => b.getAttribute('data-preview-mode') === mode);
  const pressed = () => buttons()
    .filter((b) => b.getAttribute('aria-pressed') === 'true')
    .map((b) => b.getAttribute('data-preview-mode'));

  return { sandbox, pane, view, container, record, toolbarEl, buttons, buttonFor, pressed };
}

const results = [];
function check(name, fn) { results.push({ name, fn }); }

// --- the module on its own --------------------------------------------------
check('createToolbar refuses to build without the callbacks it needs', () => {
  const h = makeHarness();
  const api = h.sandbox.termlabPreviewToolbar;
  const host = makeEl('div');
  assert.strictEqual(typeof api.createToolbar, 'function');
  assert.strictEqual(api.createToolbar(null, { onSelect() {}, readMode: () => 'editor' }), null);
  assert.strictEqual(api.createToolbar(host, { readMode: () => 'editor' }), null, 'no onSelect');
  assert.strictEqual(api.createToolbar(host, { onSelect() {} }), null, 'no readMode');
  assert.deepStrictEqual(host.children, [], 'a refused toolbar leaves no element behind');
});

check('the module exposes exactly one global and it is the factory', () => {
  const h = makeHarness();
  assert.deepStrictEqual(
    Object.keys(h.sandbox.termlabPreviewToolbar), ['createToolbar'],
    'the toolbar module exposes createToolbar and nothing else',
  );
});

check('the three buttons are real, named, focusable buttons', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.md' });
  const btns = h.buttons();
  assert.strictEqual(btns.length, 3, 'Editor, Split and Preview');
  for (const btn of btns) {
    assert.strictEqual(btn.tagName, 'BUTTON', 'a real button, not a styled div');
    assert.strictEqual(btn.getAttribute('type'), 'button');
    assert.ok(btn.getAttribute('title'), 'every button carries a tooltip');
    assert.ok(btn.getAttribute('aria-label'), 'and an accessible name');
    assert.ok(
      btn.classList.contains('tl-icon-btn'),
      'the shared icon-button chrome, not a bespoke one',
    );
  }
  assert.deepStrictEqual(
    btns.map((b) => b.getAttribute('data-preview-mode')),
    ['editor', 'split', 'preview'],
    'in the order the shortcut cycles through them',
  );
});

// --- who gets one -----------------------------------------------------------
check('a non-markdown pane gets no toolbar', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'deploy.py', docPath: '/a/deploy.py' });
  assert.strictEqual(h.toolbarEl(), null, 'nothing to offer, so nothing is shown');
});

check('a markdown pane gets one, visible in editor mode too', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.md', docPath: '/a/notes.md' });
  const el = h.toolbarEl();
  assert.ok(el, 'the control is mounted for a markdown pane');
  // Editor mode is where a user who has never heard of the preview sits. A
  // control that only appears once the preview is already showing would not
  // fix the discoverability problem it exists for.
  assert.notStrictEqual(el.parentNode, null, 'and is in the pane, not detached');
  assert.strictEqual(
    el.getAttribute('hidden'), null,
    'not hidden until hover — hidden-until-hover is the bug being fixed',
  );
});

check('a missing markdown bundle leaves no toolbar behind an inert control', () => {
  const h = makeHarness({ withBundle: false });
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  assert.strictEqual(h.toolbarEl(), null, 'no controller, so no control');
});

// --- it says which mode the pane is in --------------------------------------
check('the mounted mode is the pressed button', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  assert.deepStrictEqual(h.pressed(), ['split']);
});

check('the shortcut moves the pressed button with it', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.md' });
  assert.deepStrictEqual(h.pressed(), ['editor']);
  h.pane.togglePreview(h.view);
  assert.deepStrictEqual(h.pressed(), ['split'], 'cmd+shift+y must not desync the buttons');
  h.pane.togglePreview(h.view);
  assert.deepStrictEqual(h.pressed(), ['preview']);
  h.pane.togglePreview(h.view);
  assert.deepStrictEqual(h.pressed(), ['editor']);
});

check('a re-described pane (Save As within markdown) keeps its mode marked', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'preview', { filename: 'notes.md', docPath: '/a/notes.md' });
  h.pane.setPreviewMode(h.view, 'preview', { filename: 'other.md', docPath: '/b/other.md' });
  assert.deepStrictEqual(h.pressed(), ['preview']);
});

// --- clicking ---------------------------------------------------------------
check('clicking a button drives the same mode change the shortcut does', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.md' });
  fire(h.buttonFor('preview'), 'click');
  assert.strictEqual(h.pane.previewMode(h.view), 'preview', 'the controller moved');
  assert.ok(
    h.container.classNames().includes('md-mode-preview'),
    'and so did the layout class — one code path, not two',
  );
  assert.deepStrictEqual(h.pressed(), ['preview']);
});

check('clicking the active button is a no-op, not a cycle', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  const before = h.record.contents.length;
  fire(h.buttonFor('split'), 'click');
  assert.strictEqual(h.pane.previewMode(h.view), 'split', 'the pane stays where it is');
  assert.strictEqual(h.record.contents.length, before, 'and nothing is re-rendered');
});

check('a click does not take focus off the editor', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.md' });
  const before = defaultsPrevented;
  fire(h.buttonFor('split'), 'mousedown');
  assert.ok(
    defaultsPrevented > before,
    'mousedown default is cancelled so the caret stays in the document',
  );
});

// --- teardown ---------------------------------------------------------------
check('Save As away from markdown removes the toolbar and its listeners', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md', docPath: '/a/notes.md' });
  const el = h.toolbarEl();
  const btns = el.children.slice();
  assert.ok(btns.every((b) => b.listenerCount() > 0), 'listeners were bound in the first place');

  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.txt', docPath: '/a/notes.txt' });
  assert.strictEqual(h.toolbarEl(), null, 'the element is gone from the pane');
  assert.strictEqual(el.parentNode, null, 'and is detached');
  assert.deepStrictEqual(
    btns.map((b) => b.listenerCount()), [0, 0, 0],
    'a listener left bound per closed pane is a leak',
  );
});

check('destroying the view tears the toolbar down too', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'preview', { filename: 'notes.md' });
  const el = h.toolbarEl();
  h.pane.destroyEditorView(h.view);
  assert.strictEqual(h.toolbarEl(), null);
  assert.strictEqual(el.parentNode, null);
});

check('a torn-down toolbar cannot still drive the pane', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.md' });
  const stale = h.buttonFor('preview');
  h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.txt' });
  fire(stale, 'click');
  assert.strictEqual(h.pane.previewMode(h.view), null, 'the pane has no preview and gains none');
});

check('re-entering markdown mounts exactly one toolbar', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.txt' });
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  const mounted = h.container.children.filter(
    (c) => c.classList && c.classList.contains('md-preview-toolbar'),
  );
  assert.strictEqual(mounted.length, 1, 'no orphan from the previous mount');
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
  console.log(`preview toolbar: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`preview toolbar: all ${results.length} checks passed`);
}
