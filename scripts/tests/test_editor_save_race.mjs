// Run: node scripts/tests/test_editor_save_race.mjs
//
// A save snapshots the document, then awaits the write. A keystroke that
// lands inside that window is on screen but not in the file, so the dirty
// flag must survive the save — otherwise the close guards built on
// pane.dirty discard those characters without a prompt.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const SERVICE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/editor/editor-service.js',
);

// A faithful stand-in for the EditorView that editor-pane.js builds. It
// reproduces the two behaviours the service depends on, exactly as the real
// one implements them (editor-pane.js:43-48, 84-87):
//   - `dirty` is a plain boolean, NOT a comparison against the document;
//   - the update listener early-returns once `dirty` is already true, so a
//     second change fires no callback.
// Weakening either would hide the bug this test exists to catch.
function makeView(initialDoc, onDirtyChange) {
  let doc = initialDoc;
  let dirty = false;
  return {
    get state() {
      return { doc: { toString: () => doc } };
    },
    termlabResetDirty() {
      dirty = false;
      onDirtyChange(false);
    },
    isDirty: () => dirty,
    // One document-changing transaction, as the update listener sees it.
    change(nextDoc) {
      if (nextDoc === doc) return; // not a docChanged update
      doc = nextDoc;
      if (dirty) return;
      dirty = true;
      onDirtyChange(true);
    },
    type(text) {
      this.change(doc + text);
    },
  };
}

function makeHarness(initialDoc) {
  // Only `console` is injected; the context keeps its own intrinsics.
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const pane = { kind: 'editor', filePath: '/tmp/scratch-1.txt', dirty: false, remote: null };
  pane.view = makeView(initialDoc, (d) => { pane.dirty = d; });

  const writes = [];
  const toasts = [];
  let pendingResolve = null;
  let pendingReject = null;

  sandbox.CM6 = {};
  sandbox.toast = { error: (title, body) => { toasts.push([title, body]); } };
  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        if (command !== 'editor_write_file') return Promise.resolve(null);
        writes.push({ path: args.path, contents: args.contents });
        // Resolves only when the test says so, so the test owns the window
        // between the snapshot and the write completing.
        return new Promise((resolve, reject) => {
          pendingResolve = () => resolve(null);
          pendingReject = (e) => reject(e);
        });
      },
    },
  };
  sandbox.__termlabPaneAccess = {
    currentPane: () => pane,
    allPanes: () => new Map([[1, pane]]),
    setFocusedPane: () => {},
    activateTab: () => {},
  };

  vm.runInContext(fs.readFileSync(SERVICE_PATH, 'utf8'), sandbox, { filename: SERVICE_PATH });

  return {
    pane,
    writes,
    toasts,
    service: sandbox.termlabEditorService,
    finishWrite: () => pendingResolve(),
    failWrite: (e) => pendingReject(e),
  };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

// --- 1. An edit during an in-flight write must leave the pane dirty --------
{
  const h = makeHarness('hello');
  h.pane.view.type(' world');
  assert.strictEqual(h.pane.dirty, true, 'typing marks the pane dirty');

  const saving = h.service.saveActiveEditor();
  await tick();

  assert.strictEqual(h.writes.length, 1, 'the write was issued');
  assert.strictEqual(h.writes[0].contents, 'hello world', 'the write carries the snapshot');

  // The keystroke that races the write.
  h.pane.view.type('!');

  h.finishWrite();
  await saving;

  assert.strictEqual(
    h.pane.view.state.doc.toString(),
    'hello world!',
    'the raced keystroke is in the document',
  );
  assert.strictEqual(h.writes[0].contents, 'hello world', 'but it was never written to disk');
  assert.strictEqual(h.pane.view.isDirty(), true, 'so the view must still be dirty');
  assert.strictEqual(h.pane.dirty, true, 'and the flag the close guards read must be true');
  assert.strictEqual(h.toasts.length, 0, 'a successful write raises no toast');

  // Not permanently stuck: saving again writes the newer text and clears.
  const saving2 = h.service.saveActiveEditor();
  await tick();
  assert.strictEqual(h.writes[1].contents, 'hello world!', 'the second save writes the newer text');
  h.finishWrite();
  await saving2;
  assert.strictEqual(h.pane.dirty, false, 'and now the pane is clean');
}

// --- 2. The ordinary case still clears the flag ---------------------------
// Guards against "fixing" the race by disabling the reset outright.
{
  const h = makeHarness('hello');
  h.pane.view.type(' world');
  assert.strictEqual(h.pane.dirty, true, 'precondition: dirty before the save');

  const saving = h.service.saveActiveEditor();
  await tick();
  h.finishWrite();
  await saving;

  assert.strictEqual(h.writes.length, 1);
  assert.strictEqual(h.writes[0].contents, 'hello world');
  assert.strictEqual(h.pane.view.isDirty(), false, 'an undisturbed save clears the view flag');
  assert.strictEqual(h.pane.dirty, false, 'an undisturbed save clears the pane flag');
}

// --- 3. An edit undone during the write still counts as saved -------------
// The test is "does the buffer match the bytes that were written", not "did
// anything happen at all", so an edit and its undo must not strand the tab
// with a dirty marker that no save can clear.
{
  const h = makeHarness('hello');
  h.pane.view.type(' world');
  const saving = h.service.saveActiveEditor();
  await tick();
  h.pane.view.type('X');
  h.pane.view.change('hello world'); // undo, back to the written text
  h.finishWrite();
  await saving;

  assert.strictEqual(h.pane.view.state.doc.toString(), 'hello world');
  assert.strictEqual(h.pane.dirty, false, 'a buffer matching the write is clean');
}

// --- 4. A failed write must not clear the flag ----------------------------
{
  const h = makeHarness('hello');
  h.pane.view.type(' world');
  const saving = h.service.saveActiveEditor();
  await tick();
  h.failWrite(new Error('Could not write /tmp/scratch-1.txt: permission denied'));
  await saving;

  assert.strictEqual(h.pane.dirty, true, 'a failed write leaves the pane dirty');
  assert.strictEqual(h.toasts.length, 1, 'and raises a toast');
  assert.strictEqual(h.toasts[0][0], 'Save Failed');
}

console.log('editor save race: all assertions passed');
