// Run: node scripts/tests/test_preview_wiring.mjs
//
// The seam between the editor pane and the preview modules: who gets a
// preview, where it hangs in the DOM, what the mode toggle does, and what the
// update listener drives.
//
// The real modules under test are features/editor/editor-pane.js,
// features/editor/preview/preview-controller.js, preview-mode.js,
// image-resolver.js and language-map.js. The parser and the frame are stubbed
// at their factory globals — that is the seam the controller resolves them
// through — because markdown-it needs a DOM and the frame's own contract is
// already pinned by test_markdown_render.mjs and test_preview_frame.mjs.
//
// What this pins:
//   - a preview is offered for markdown and for nothing else;
//   - a missing MDLib bundle leaves the pane in editor mode with no host
//     element and no throw (a fresh checkout run with plain `cargo run`);
//   - Save As away from .md tears the preview down;
//   - the mode is a class on the pane container, so the EditorView is never
//     re-parented;
//   - typing schedules a debounced render and scrolling drives the frame;
//   - images are resolved after injection, on the frame's load event, and are
//     generation-checked.
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
  editorPane: path.join(ROOT, 'app/features/editor/editor-pane.js'),
};

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// --- a minimal element ------------------------------------------------------
function makeEl(tag) {
  const classes = new Set();
  const el = {
    tagName: String(tag).toUpperCase(),
    children: [],
    parentNode: null,
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
    EditorState: {
      create: (spec) => ({ spec, doc: { toString: () => spec.doc } }),
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
}

// --- the harness ------------------------------------------------------------
//
// `options.withBundle: false` removes the markdown renderer factory, standing
// in for the gitignored vendor bundle being absent.
function makeHarness(options = {}) {
  const record = {
    frames: [], contents: [], scrolls: [], destroyed: 0, linkHandlers: [], invokes: [],
  };

  const sandbox = {
    console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap, Array, Object, String, Number,
    document: { createElement: makeEl },
  };
  sandbox.window = sandbox;
  sandbox.CM6 = makeCM6();

  // The Tauri seam the image resolver reads through.
  sandbox.termlabServices = {
    tauriClient: {
      invoke: async (command, args) => {
        record.invokes.push([command, args]);
        if (command === 'editor_read_image_base64') return 'QUJD';
        return null;
      },
    },
  };

  vm.createContext(sandbox);
  for (const key of ['languageMap', 'previewMode', 'images', 'controller', 'editorPane']) {
    vm.runInContext(fs.readFileSync(FILES[key], 'utf8'), sandbox, { filename: FILES[key] });
  }

  // Stubbed AFTER load: both are resolved off globals at call time, which is
  // exactly the injection point the controller was written to have.
  if (options.withBundle !== false) {
    sandbox.MDLib = {};
    sandbox.termlabMarkdownRenderer = {
      createRenderer: () => ({ render: (md) => `<p data-src-line="0">${md}</p>` }),
    };
  }
  sandbox.termlabPreviewFrame = {
    createFrame(host, deps) {
      const element = {
        parentNode: null,
        contentDocument: null,
        listeners: {},
        addEventListener(type, fn) { (this.listeners[type] || (this.listeners[type] = [])).push(fn); },
      };
      host.appendChild(element);
      const frame = {
        element,
        setContent(html) { record.contents.push(html); },
        scrollToLine(line) { record.scrolls.push(line); },
        destroy() { record.destroyed += 1; },
      };
      record.frames.push(frame);
      record.linkHandlers.push(deps && deps.onLinkClick);
      return frame;
    },
  };

  // The pane DOM the tab manager builds: container > editor host > the view.
  const container = makeEl('div');
  container.className = 'terminal-pane';
  const editorHost = makeEl('div');
  editorHost.className = 'editor-pane-host';
  container.appendChild(editorHost);

  const pane = sandbox.termlabEditorPane;
  const view = pane.createEditorView(editorHost, { doc: '# hello', filename: 'notes.md' });
  // The real EditorView mounts itself into its parent; the stub does not.
  view.dom = editorHost.appendChild(makeEl('div'));
  view.scrollDOM = { scrollTop: 0 };

  const listeners = view.state.spec.extensions.filter((e) => e && e.ext === 'updateListener');
  return {
    sandbox, pane, view, container, editorHost, record,
    // The dirty watcher is first, the preview watcher second.
    previewWatcher: listeners[listeners.length - 1].fn,
    hosts: () => container.children.filter((c) => c.className === 'md-preview-host'),
  };
}

const results = [];
function check(name, fn) { results.push({ name, fn }); }

// --- who gets a preview -----------------------------------------------------
check('a non-markdown file gets no preview, no host element and no class', () => {
  const h = makeHarness();
  const applied = h.pane.setPreviewMode(h.view, 'split', { filename: 'deploy.py', docPath: '/a/deploy.py' });
  assert.strictEqual(applied, null, 'no mode is applied');
  assert.strictEqual(h.pane.previewMode(h.view), null);
  assert.deepStrictEqual(h.hosts(), [], 'nothing was mounted');
  assert.strictEqual(h.record.frames.length, 0, 'no frame was created');
  assert.strictEqual(
    h.container.classNames().includes('md-mode-split'), false,
    'the pane must behave exactly as it does today',
  );
});

check('a markdown file mounts one host and takes the mode class', () => {
  const h = makeHarness();
  const applied = h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md', docPath: '/a/notes.md' });
  assert.strictEqual(applied, 'split');
  assert.strictEqual(h.hosts().length, 1, 'exactly one preview host');
  assert.ok(h.container.classNames().includes('md-mode-split'));
  assert.strictEqual(h.record.frames.length, 1);
  assert.deepStrictEqual(h.record.contents, ['<p data-src-line="0"># hello</p>']);
  // The layout is a class change, never a re-parent: the view is still where
  // it was mounted, which is what keeps undo history across a toggle.
  assert.strictEqual(h.view.dom.parentNode, h.editorHost);
});

check('editor mode mounts nothing until the preview is actually shown', () => {
  const h = makeHarness();
  assert.strictEqual(h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.md' }), 'editor');
  assert.strictEqual(h.record.frames.length, 0, 'no frame while the preview is hidden');
  assert.ok(h.container.classNames().includes('md-mode-editor'));
  assert.strictEqual(h.pane.previewMode(h.view), 'editor', 'but the pane HAS a preview to toggle');
});

check('an unknown mode degrades to editor rather than throwing', () => {
  const h = makeHarness();
  assert.strictEqual(h.pane.setPreviewMode(h.view, 'splitt', { filename: 'notes.md' }), 'editor');
});

// --- the missing vendor bundle ---------------------------------------------
check('no MDLib: editor mode, inert toggle, no host left behind, no throw', () => {
  const h = makeHarness({ withBundle: false });
  assert.strictEqual(h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' }), null);
  assert.deepStrictEqual(h.hosts(), [], 'the host element is removed again, not orphaned');
  assert.strictEqual(h.pane.togglePreview(h.view), null, 'the toggle is inert');
  assert.strictEqual(
    h.container.classNames().includes('md-mode-split'), false,
    'and the layout never changed',
  );
});

// --- Save As ----------------------------------------------------------------
check('notes.md saved as notes.txt drops out of preview', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'preview', { filename: 'notes.md', docPath: '/a/notes.md' });
  assert.strictEqual(h.hosts().length, 1);

  h.pane.setPreviewMode(h.view, 'preview', { filename: 'notes.txt', docPath: '/a/notes.txt' });
  assert.strictEqual(h.pane.previewMode(h.view), null, 'the preview is gone entirely');
  assert.deepStrictEqual(h.hosts(), [], 'and so is its host element');
  assert.strictEqual(h.record.destroyed, 1, 'the frame was destroyed');
  assert.ok(h.container.classNames().includes('md-mode-editor'));
  assert.strictEqual(h.container.classNames().includes('md-mode-preview'), false);
});

check('a rename between two markdown names keeps the mode the user chose', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'preview', { filename: 'notes.md', docPath: '/a/notes.md' });
  const applied = h.pane.setPreviewMode(h.view, 'preview', { filename: 'other.md', docPath: '/b/other.md' });
  assert.strictEqual(applied, 'preview');
  assert.strictEqual(h.hosts().length, 1, 'the same host, not a second one');
});

// --- the toggle -------------------------------------------------------------
check('togglePreview cycles editor -> split -> preview -> editor', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.md' });
  assert.strictEqual(h.pane.togglePreview(h.view), 'split');
  assert.ok(h.container.classNames().includes('md-mode-split'));
  assert.strictEqual(h.pane.togglePreview(h.view), 'preview');
  assert.ok(h.container.classNames().includes('md-mode-preview'));
  assert.strictEqual(h.container.classNames().includes('md-mode-split'), false, 'one class at a time');
  assert.strictEqual(h.pane.togglePreview(h.view), 'editor');
});

check('togglePreview on a pane with no preview reports null, so the key can fall through', () => {
  const h = makeHarness();
  assert.strictEqual(h.pane.togglePreview(h.view), null);
});

// --- the update listener ----------------------------------------------------
check('typing schedules a debounced re-render', async () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  assert.strictEqual(h.record.contents.length, 1, 'the mode change rendered once');

  h.previewWatcher({ view: h.view, docChanged: true });
  h.previewWatcher({ view: h.view, docChanged: true });
  h.previewWatcher({ view: h.view, docChanged: true });
  assert.strictEqual(h.record.contents.length, 1, 'nothing renders while the keys are still coming');
  await sleep(220);
  assert.strictEqual(h.record.contents.length, 2, 'three keystrokes, one render');
});

check('typing in editor mode renders nothing at all', async () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'editor', { filename: 'notes.md' });
  h.previewWatcher({ view: h.view, docChanged: true });
  await sleep(220);
  assert.strictEqual(h.record.contents.length, 0);
  assert.strictEqual(h.record.frames.length, 0);
});

check('scrolling the editor drives the preview, and only that direction', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  h.view.scrollDOM.scrollTop = 120;
  h.view.lineBlockAtHeight = () => ({ from: 42 });
  h.view.state.doc.lineAt = () => ({ number: 7 });

  h.previewWatcher({ view: h.view, geometryChanged: true });
  // 0-based, matching the data-src-line values markdown-it produces.
  assert.deepStrictEqual(h.record.scrolls, [6]);
});

check('a view whose geometry cannot be read skips the sync instead of throwing', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  h.view.lineBlockAtHeight = () => { throw new Error('mid-layout'); };
  h.previewWatcher({ view: h.view, geometryChanged: true });
  assert.deepStrictEqual(h.record.scrolls, [], 'nothing was scrolled, nothing was thrown');
});

check('the watcher is inert for a pane with no preview', () => {
  const h = makeHarness();
  h.previewWatcher({ view: h.view, docChanged: true, geometryChanged: true });
  assert.strictEqual(h.record.frames.length, 0);
});

// --- images -----------------------------------------------------------------
check('images resolve on the frame load event, after the text is injected', async () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md', docPath: '/home/u/docs/notes.md' });
  const frame = h.record.frames[0];

  const img = {
    attrs: { src: 'shot.png' },
    getAttribute(k) { return this.attrs[k] ?? null; },
    setAttribute(k, v) { this.attrs[k] = v; },
  };
  const inline = {
    attrs: { src: 'data:image/png;base64,AAA' },
    getAttribute(k) { return this.attrs[k] ?? null; },
    setAttribute(k, v) { this.attrs[k] = v; },
  };
  frame.element.contentDocument = { querySelectorAll: () => [img, inline] };

  // srcdoc parses asynchronously, so resolution hangs off `load`, not off the
  // setContent call — which is also what makes the text readable first.
  frame.element.listeners.load.forEach((fn) => fn());
  await sleep(20);

  assert.strictEqual(img.attrs.src, 'data:image/png;base64,QUJD', 'swapped for a data: URI');
  assert.strictEqual(inline.attrs.src, 'data:image/png;base64,AAA', 'an inline image is left alone');
  // Flattened to strings: the args object is built inside the vm realm, so a
  // structural compare would fail on prototype identity alone.
  assert.deepStrictEqual(
    h.record.invokes.map(([command, args]) => `${command} ${args.path}`),
    ['editor_read_image_base64 /home/u/docs/shot.png'],
    'resolved against the document\'s own directory, exactly once',
  );
});

check('a render that lands mid-fetch supersedes the one in flight', async () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md', docPath: '/home/u/docs/notes.md' });
  const frame = h.record.frames[0];
  const img = {
    attrs: { src: 'shot.png' },
    getAttribute(k) { return this.attrs[k] ?? null; },
    setAttribute(k, v) { this.attrs[k] = v; },
  };
  frame.element.contentDocument = { querySelectorAll: () => [img] };

  frame.element.listeners.load.forEach((fn) => fn());
  // Re-render before the fetch above settles: the generation moves on, so its
  // result must be dropped rather than painted into the frame that replaced it.
  h.pane.setPreviewMode(h.view, 'preview', { filename: 'notes.md', docPath: '/home/u/docs/notes.md' });
  await sleep(20);
  assert.strictEqual(img.attrs.src, 'shot.png', 'the superseded result never landed');
});

// --- teardown ---------------------------------------------------------------
check('destroyEditorView tears the preview down with the view', () => {
  const h = makeHarness();
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  h.pane.destroyEditorView(h.view);
  assert.strictEqual(h.record.destroyed, 1);
  assert.deepStrictEqual(h.hosts(), []);
  assert.strictEqual(h.pane.previewMode(h.view), null);
  assert.strictEqual(h.view.destroyed, true, 'and the view itself still gets destroyed');
});

check('refreshTheme re-renders an open preview', () => {
  const h = makeHarness();
  h.sandbox.termlabEditorTheme = { buildTheme: () => [] };
  h.pane.setPreviewMode(h.view, 'split', { filename: 'notes.md' });
  const before = h.record.contents.length;
  h.pane.refreshTheme(h.view);
  assert.strictEqual(h.record.contents.length, before + 1, 'the frame re-snapshots the palette');
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
  console.log(`preview wiring: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`preview wiring: all ${results.length} checks passed`);
}
