// Run: node scripts/tests/test_editor_close_guards.mjs
//
// The unsaved-changes prompt, end to end through the real modules: the real
// app/ui/tl-dialog.js builds the dialog, the real core/dialog-service.js's
// confirmSave() wires the three buttons, and the real
// features/editor/editor-service.js's confirmDirtyPanes()/confirmAllDirty()
// drive them. Nothing about the prompt itself is stubbed — the test clicks the
// actual <button> elements tl-dialog created, and presses Escape through the
// same keyboard-router contract tl-dialog registers against.
//
// Only the two things that genuinely cannot exist in Node are stubbed: the DOM
// (no jsdom in this repo — same minimal element factory as
// test_tl_dialog.mjs) and the Tauri `invoke` bridge, which returns exactly
// what editor_write_file returns in Rust: a resolved promise carrying `null`
// on success (editor_fs.rs's `Result<(), String>` serialises to null), and a
// rejected promise carrying the error *string* on failure.
//
// What this pins: Cancel aborts, a failed save aborts, Escape counts as
// Cancel, Don't Save proceeds without writing, and a clean pane is never
// asked about at all.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const TL_DIALOG = path.join(ROOT, 'app/ui/tl-dialog.js');
const DIALOG_SERVICE = path.join(ROOT, 'app/core/dialog-service.js');
const EDITOR_SERVICE = path.join(ROOT, 'app/features/editor/editor-service.js');

// --- minimal DOM ----------------------------------------------------------
function makeElement(tag, doc) {
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    disabled: false,
    tabIndex: 0,
    isConnected: false,
    innerHTML: '',
    appendChild(child) {
      this.children.push(child);
      child.parentNode = this;
      child.isConnected = this.isConnected;
      return child;
    },
    removeChild(child) {
      const idx = this.children.indexOf(child);
      if (idx >= 0) this.children.splice(idx, 1);
      child.parentNode = null;
      child.isConnected = false;
      return child;
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    setAttribute(name, value) { attrs.set(name, String(value)); },
    getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
    removeAttribute(name) { attrs.delete(name); },
    hasAttribute(name) { return attrs.has(name); },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    removeEventListener(name, fn) {
      const arr = listeners.get(name) || [];
      const idx = arr.indexOf(fn);
      if (idx >= 0) arr.splice(idx, 1);
    },
    click() {
      for (const fn of (listeners.get('click') || []).slice()) fn({ target: this });
    },
    dispatch(name, event) {
      for (const fn of (listeners.get(name) || []).slice()) fn(event);
    },
    querySelectorAll() { return []; },
    querySelector() { return null; },
    contains(node) {
      let n = node;
      while (n) { if (n === this) return true; n = n.parentNode; }
      return false;
    },
    focus() { doc.activeElement = this; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 100, bottom: 30, width: 100, height: 30 }; },
    classList: {
      _set: new Set(),
      add(c) { this._set.add(c); },
      remove(c) { this._set.delete(c); },
      contains(c) { return this._set.has(c); },
    },
  };
  Object.defineProperty(el, 'className', {
    get() { return Array.from(el.classList._set).join(' '); },
    set(v) { el.classList._set = new Set(String(v).split(' ').filter(Boolean)); },
  });
  // Assigning textContent updates innerHTML with the text escaped, as a real
  // element does. dialog-service.js's escHtml() falls back to exactly that
  // round trip when window.utils is absent, so without it the file name would
  // silently come out empty and the prompt's whole point would go untested.
  let text = '';
  Object.defineProperty(el, 'textContent', {
    get() { return text; },
    set(v) {
      text = String(v == null ? '' : v);
      el.innerHTML = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    },
  });
  return el;
}

// Mirrors core/keyboard-router.js's dispatch order (highest priority first,
// registration order breaking ties, first handler returning true wins), which
// is what tl-dialog's Escape handling is written against.
function makeRouterStub() {
  let nextId = 1;
  const handlers = new Map();
  return {
    register(options) {
      const id = nextId++;
      handlers.set(id, {
        order: id,
        priority: options && typeof options.priority === 'number' ? options.priority : 0,
        isActive: options && typeof options.isActive === 'function' ? options.isActive : null,
        onKeyDown: options && typeof options.onKeyDown === 'function' ? options.onKeyDown : null,
      });
      return () => handlers.delete(id);
    },
    dispatchEscape() {
      const sorted = Array.from(handlers.values()).sort((a, b) => {
        if (a.priority !== b.priority) return b.priority - a.priority;
        return a.order - b.order;
      });
      for (const entry of sorted) {
        if (entry.isActive && !entry.isActive()) continue;
        if (!entry.onKeyDown) continue;
        if (entry.onKeyDown({ key: 'Escape' }) === true) return true;
      }
      return false;
    },
  };
}

// Same EditorView stand-in as test_editor_save_race.mjs: `dirty` is a plain
// boolean, and the update listener stops firing once it is set
// (editor-pane.js:43-48, 84-87).
function makeView(initialDoc, onDirtyChange) {
  let doc = initialDoc;
  let dirty = false;
  return {
    get state() { return { doc: { toString: () => doc } }; },
    termlabResetDirty() { dirty = false; onDirtyChange(false); },
    isDirty: () => dirty,
    type(text) {
      doc += text;
      if (dirty) return;
      dirty = true;
      onDirtyChange(true);
    },
  };
}

function makeHarness(options = {}) {
  const sandbox = { console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap };
  sandbox.window = sandbox;
  const document = {
    activeElement: null,
    createElement: (tag) => makeElement(tag, document),
    addEventListener() {},
    removeEventListener() {},
  };
  document.body = makeElement('body', document);
  document.body.isConnected = true;
  sandbox.document = document;
  sandbox.requestAnimationFrame = (fn) => fn();
  sandbox.termlabKeyboardRouter = makeRouterStub();
  sandbox.innerWidth = 1024;
  sandbox.innerHeight = 768;
  sandbox.CM6 = {};

  const toasts = [];
  sandbox.toast = {
    error: (title, body) => { toasts.push([title, body]); },
    warn: () => {}, info: () => {}, success: () => {},
  };

  // The write bridge. Success resolves with null and failure rejects with the
  // error string, which is what `editor_write_file` (editor_fs.rs, returning
  // `Result<(), String>`) actually produces over the Tauri IPC.
  const writes = [];
  let failWritesWith = null;
  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        if (command !== 'editor_write_file') return Promise.resolve(null);
        writes.push({ path: args.path, contents: args.contents });
        if (failWritesWith) return Promise.reject(failWritesWith);
        return Promise.resolve(null);
      },
    },
  };

  const panes = new Map();
  sandbox.__termlabPaneAccess = {
    currentPane: () => panes.values().next().value || null,
    allPanes: () => panes,
    setFocusedPane: () => {},
    activateTab: () => {},
  };

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(TL_DIALOG, 'utf8'), sandbox, { filename: TL_DIALOG });
  vm.runInContext(fs.readFileSync(DIALOG_SERVICE, 'utf8'), sandbox, { filename: DIALOG_SERVICE });
  if (!options.noDialogService) {
    // loaded above; nothing to do
  } else {
    delete sandbox.termlabDialogService;
  }
  vm.runInContext(fs.readFileSync(EDITOR_SERVICE, 'utf8'), sandbox, { filename: EDITOR_SERVICE });

  let nextPaneId = 1;
  function addPane(filePath, text) {
    const paneId = nextPaneId++;
    const pane = { paneId, tabId: 1, kind: 'editor', filePath, dirty: false, remote: null };
    pane.view = makeView('', (d) => { pane.dirty = d; });
    if (text) pane.view.type(text);
    panes.set(paneId, pane);
    return pane;
  }

  function findButton(label) {
    let found = null;
    const walk = (node) => {
      for (const child of node.children) {
        if (!found && child.tagName === 'BUTTON' && child.textContent === label) found = child;
        walk(child);
      }
    };
    walk(document.body);
    return found;
  }

  return {
    sandbox,
    document,
    panes,
    writes,
    toasts,
    addPane,
    findButton,
    dialogCount: () => sandbox.tlDialog.count(),
    pressEscape: () => sandbox.termlabKeyboardRouter.dispatchEscape(),
    failWrites: (message) => { failWritesWith = message; },
    service: sandbox.termlabEditorService,
  };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

const results = [];
function check(name, fn) {
  results.push({ name, fn });
}

// --- confirmSave: the three answers come from the real buttons -------------
check('confirmSave resolves "save" when the Save button is clicked', async () => {
  const h = makeHarness();
  const p = h.sandbox.termlabDialogService.confirmSave('notes.txt');
  await tick();
  assert.strictEqual(h.dialogCount(), 1, 'a dialog is open');
  const btn = h.findButton('Save');
  assert.ok(btn, 'the Save button exists');
  btn.click();
  assert.strictEqual(await p, 'save');
  assert.strictEqual(h.dialogCount(), 0, 'and the dialog closed');
});

check('confirmSave resolves "discard" for Don\'t Save', async () => {
  const h = makeHarness();
  const p = h.sandbox.termlabDialogService.confirmSave('notes.txt');
  await tick();
  h.findButton("Don't Save").click();
  assert.strictEqual(await p, 'discard');
});

check('confirmSave resolves "cancel" for Cancel', async () => {
  const h = makeHarness();
  const p = h.sandbox.termlabDialogService.confirmSave('notes.txt');
  await tick();
  h.findButton('Cancel').click();
  assert.strictEqual(await p, 'cancel');
});

check('confirmSave resolves "cancel" for Escape', async () => {
  const h = makeHarness();
  const p = h.sandbox.termlabDialogService.confirmSave('notes.txt');
  await tick();
  assert.strictEqual(h.pressEscape(), true, 'the dialog consumed Escape');
  assert.strictEqual(await p, 'cancel', 'a stray keystroke must not discard work');
  assert.strictEqual(h.dialogCount(), 0);
});

check('confirmSave names the file being closed', async () => {
  const h = makeHarness();
  const p = h.sandbox.termlabDialogService.confirmSave('scratch-3.txt');
  await tick();
  let body = null;
  const walk = (node) => {
    for (const c of node.children) {
      if (typeof c.innerHTML === 'string' && c.innerHTML.includes('scratch-3.txt')) body = c;
      walk(c);
    }
  };
  walk(h.document.body);
  assert.ok(body, 'the prompt says which file it is about');
  h.findButton('Cancel').click();
  await p;
});

// --- confirmDirtyPanes ----------------------------------------------------
check('a clean pane is never asked about', async () => {
  const h = makeHarness();
  const pane = h.addPane('/s/clean.txt', '');
  assert.strictEqual(pane.dirty, false);
  const ok = await h.service.confirmDirtyPanes([pane]);
  assert.strictEqual(ok, true, 'closing proceeds');
  assert.strictEqual(h.dialogCount(), 0, 'and no dialog was ever opened');
  assert.strictEqual(h.writes.length, 0);
});

check('an empty pane list proceeds with no prompt', async () => {
  const h = makeHarness();
  assert.strictEqual(await h.service.confirmDirtyPanes([]), true);
  assert.strictEqual(h.dialogCount(), 0);
});

check('Cancel aborts and leaves the pane dirty', async () => {
  const h = makeHarness();
  const pane = h.addPane('/s/a.txt', 'unsaved words');
  const p = h.service.confirmDirtyPanes([pane]);
  await tick();
  h.findButton('Cancel').click();
  assert.strictEqual(await p, false, 'the close is refused');
  assert.strictEqual(pane.dirty, true, 'and the work is still there, still dirty');
  assert.strictEqual(h.writes.length, 0, 'nothing was written');
});

check('Escape aborts, exactly like Cancel', async () => {
  const h = makeHarness();
  const pane = h.addPane('/s/a.txt', 'unsaved words');
  const p = h.service.confirmDirtyPanes([pane]);
  await tick();
  h.pressEscape();
  assert.strictEqual(await p, false);
  assert.strictEqual(pane.dirty, true);
});

check("Don't Save proceeds without writing", async () => {
  const h = makeHarness();
  const pane = h.addPane('/s/a.txt', 'throwaway');
  const p = h.service.confirmDirtyPanes([pane]);
  await tick();
  h.findButton("Don't Save").click();
  assert.strictEqual(await p, true, 'the close proceeds');
  assert.strictEqual(h.writes.length, 0, 'and the file on disk is untouched');
});

check('Save writes the text and then proceeds', async () => {
  const h = makeHarness();
  const pane = h.addPane('/s/a.txt', 'keep me');
  const p = h.service.confirmDirtyPanes([pane]);
  await tick();
  h.findButton('Save').click();
  assert.strictEqual(await p, true);
  assert.deepStrictEqual(h.writes, [{ path: '/s/a.txt', contents: 'keep me' }]);
  assert.strictEqual(pane.dirty, false, 'the pane is clean afterwards');
});

check('a failed save aborts the close and raises a toast', async () => {
  const h = makeHarness();
  const pane = h.addPane('/s/a.txt', 'keep me');
  h.failWrites('Could not write /s/a.txt: Permission denied (os error 13)');
  const p = h.service.confirmDirtyPanes([pane]);
  await tick();
  h.findButton('Save').click();
  assert.strictEqual(await p, false, 'a failed save must not be consent to lose the file');
  assert.strictEqual(pane.dirty, true, 'the pane is still dirty');
  assert.strictEqual(h.toasts.length, 1);
  assert.strictEqual(h.toasts[0][0], 'Save Failed');
  assert.match(h.toasts[0][1], /Permission denied/);
});

check('each dirty pane is asked about in turn', async () => {
  const h = makeHarness();
  const a = h.addPane('/s/a.txt', 'aaa');
  const b = h.addPane('/s/b.txt', 'bbb');
  const p = h.service.confirmDirtyPanes([a, b]);
  await tick();
  assert.strictEqual(h.dialogCount(), 1, 'one prompt at a time');
  h.findButton("Don't Save").click();
  await tick();
  assert.strictEqual(h.dialogCount(), 1, 'the second pane gets its own prompt');
  h.findButton("Don't Save").click();
  assert.strictEqual(await p, true);
});

check('Cancel on the second prompt aborts the whole close', async () => {
  const h = makeHarness();
  const a = h.addPane('/s/a.txt', 'aaa');
  const b = h.addPane('/s/b.txt', 'bbb');
  const p = h.service.confirmDirtyPanes([a, b]);
  await tick();
  h.findButton('Save').click();      // first: save it
  await tick();
  h.findButton('Cancel').click();    // second: change of mind
  assert.strictEqual(await p, false, 'the whole close is abandoned, not just this tab');
  assert.strictEqual(b.dirty, true, 'the second file keeps its unsaved text');
  assert.strictEqual(h.writes.length, 1, 'only the first was written');
});

check('no dialog service means no consent', async () => {
  const h = makeHarness({ noDialogService: true });
  const pane = h.addPane('/s/a.txt', 'aaa');
  assert.strictEqual(await h.service.confirmDirtyPanes([pane]), false);
  assert.strictEqual(h.toasts.length, 1, 'and the user is told why nothing closed');
});

// --- confirmAllDirty ------------------------------------------------------
check('confirmAllDirty finds every dirty editor pane in the window', async () => {
  const h = makeHarness();
  h.addPane('/s/clean.txt', '');
  const a = h.addPane('/s/a.txt', 'aaa');
  const b = h.addPane('/s/b.txt', 'bbb');
  // A terminal pane in the same window must be ignored entirely.
  h.panes.set(99, { paneId: 99, kind: 'terminal', dirty: true });

  const p = h.service.confirmAllDirty();
  await tick();
  h.findButton("Don't Save").click();
  await tick();
  h.findButton("Don't Save").click();
  await tick();
  assert.strictEqual(h.dialogCount(), 0, 'exactly two prompts, one per dirty editor');
  assert.strictEqual(await p, true);
  assert.strictEqual(a.dirty, true);
  assert.strictEqual(b.dirty, true);
});

check('confirmAllDirty with nothing dirty never prompts', async () => {
  const h = makeHarness();
  h.addPane('/s/clean.txt', '');
  assert.strictEqual(await h.service.confirmAllDirty(), true);
  assert.strictEqual(h.dialogCount(), 0);
});

check('confirmAllDirty stops at the first Cancel', async () => {
  const h = makeHarness();
  h.addPane('/s/a.txt', 'aaa');
  h.addPane('/s/b.txt', 'bbb');
  const p = h.service.confirmAllDirty();
  await tick();
  h.findButton('Cancel').click();
  assert.strictEqual(await p, false);
  assert.strictEqual(h.dialogCount(), 0, 'the second pane is never asked about');
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
  console.log(`editor close guards: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`editor close guards: all ${results.length} checks passed`);
}
