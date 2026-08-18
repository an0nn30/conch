// Run: node scripts/tests/test_editor_save_inflight.mjs
//
// Two saves for the same pane must never be in flight at once. If they are,
// they can resolve out of order: the newer write clears `dirty`, then the
// older write lands last and leaves stale bytes on disk while the pane reads
// clean. The close guards in this same file read `pane.dirty`, so that lie is
// what makes them close a tab over unsaved text — the exact failure the guards
// exist to prevent.
//
// Companion to test_editor_save_race.mjs, which pins the other half of the
// contract: an edit typed *during* a save keeps the pane dirty.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const SERVICE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/editor/editor-service.js',
);

// Same stand-in for the real EditorView as test_editor_save_race.mjs, and for
// the same reason: `dirty` is a plain boolean and the update listener stops
// firing once it is set (editor-pane.js:43-48, 84-87).
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
    change(nextDoc) {
      if (nextDoc === doc) return;
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
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const pane = { kind: 'editor', filePath: '/tmp/scratch-1.txt', dirty: false, remote: null };
  pane.view = makeView(initialDoc, (d) => { pane.dirty = d; });

  // Every write is parked until the test releases it, so overlapping calls
  // really do overlap.
  const writes = [];
  const toasts = [];

  sandbox.CM6 = {};
  sandbox.toast = { error: (title, body) => { toasts.push([title, body]); } };
  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        if (command !== 'editor_write_file') return Promise.resolve(null);
        const entry = { path: args.path, contents: args.contents };
        writes.push(entry);
        return new Promise((resolve, reject) => {
          entry.resolve = () => resolve(null);
          entry.reject = (e) => reject(e);
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

  return { pane, writes, toasts, service: sandbox.termlabEditorService };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

// --- 1. A second save for the same pane starts no second write -------------
{
  const h = makeHarness('hello');
  h.pane.view.type(' world');

  const first = h.service.savePane(h.pane);
  await tick();
  assert.strictEqual(h.writes.length, 1, 'the first save issued its write');

  // The double cmd+s / menu-click-while-pending case.
  const second = h.service.savePane(h.pane);
  await tick();
  assert.strictEqual(
    h.writes.length,
    1,
    'the second save must join the pending write, not start an overlapping one',
  );

  let secondSettled = false;
  second.then(() => { secondSettled = true; });
  await tick();
  assert.strictEqual(secondSettled, false, 'and it must not resolve before the write does');

  h.writes[0].resolve();
  await first;
  await second;
  assert.strictEqual(h.writes.length, 1, 'still exactly one write for the two calls');
  assert.strictEqual(h.pane.dirty, false, 'the pane ends clean');
}

// --- 2. saveActiveEditor is covered by the same guard ----------------------
// The keyboard path is where a double cmd+s actually comes from.
{
  const h = makeHarness('hello');
  h.pane.view.type(' world');

  const a = h.service.saveActiveEditor();
  const b = h.service.saveActiveEditor();
  await tick();
  assert.strictEqual(h.writes.length, 1, 'a double cmd+s issues one write');

  h.writes[0].resolve();
  await Promise.all([a, b]);
  assert.strictEqual(h.pane.dirty, false);
  assert.strictEqual(h.toasts.length, 0);
}

// --- 3. The joiner still gets the newer text written ----------------------
// Joining is not the same as skipping: if the in-flight write snapshotted the
// document before the joiner's text existed, the joiner owes one more write.
{
  const h = makeHarness('hello');
  h.pane.view.type(' world');

  const first = h.service.savePane(h.pane);
  await tick();
  assert.strictEqual(h.writes[0].contents, 'hello world');

  h.pane.view.type('!');            // races the in-flight write
  const second = h.service.savePane(h.pane);
  await tick();
  assert.strictEqual(h.writes.length, 1, 'no overlapping write while the first is pending');

  h.writes[0].resolve();
  await first;
  await tick();

  assert.strictEqual(h.writes.length, 2, 'the joiner writes the newer text once it is free to');
  assert.strictEqual(h.writes[1].contents, 'hello world!');
  h.writes[1].resolve();
  await second;
  assert.strictEqual(h.pane.dirty, false, 'and only then is the pane clean');
}

// --- 4. The retry is bounded ---------------------------------------------
// Typing through every write must not spin forever issuing writes.
{
  const h = makeHarness('hello');
  h.pane.view.type('a');

  const first = h.service.savePane(h.pane);
  await tick();
  h.pane.view.type('b');
  const second = h.service.savePane(h.pane);
  await tick();

  h.writes[0].resolve();
  await first;
  await tick();
  assert.strictEqual(h.writes.length, 2);

  h.pane.view.type('c');   // and again, during the retry's write
  h.writes[1].resolve();
  await second;
  await tick();

  assert.strictEqual(h.writes.length, 2, 'the retry does not retry itself');
  assert.strictEqual(h.pane.dirty, true, 'the pane is honestly still dirty');
}

// --- 5. A failure reaches the joiner too ----------------------------------
// Silently resolving a joined save would tell a close guard the file is safe.
{
  const h = makeHarness('hello');
  h.pane.view.type(' world');

  const first = h.service.savePane(h.pane);
  await tick();
  const second = h.service.savePane(h.pane);
  await tick();

  h.writes[0].reject(new Error('permission denied'));

  await assert.rejects(() => first, /permission denied/, 'the owner sees the failure');
  await assert.rejects(() => second, /permission denied/, 'the joiner sees it as well');
  assert.strictEqual(h.pane.dirty, true, 'and the pane stays dirty');
}

// --- 6. The guard is released, not sticky --------------------------------
{
  const h = makeHarness('hello');
  h.pane.view.type(' world');
  const first = h.service.savePane(h.pane);
  await tick();
  h.writes[0].resolve();
  await first;

  h.pane.view.type('!');
  const later = h.service.savePane(h.pane);
  await tick();
  assert.strictEqual(h.writes.length, 2, 'a later save is not blocked by the finished one');
  h.writes[1].resolve();
  await later;
  assert.strictEqual(h.pane.dirty, false);
}

console.log('editor save in-flight guard: all assertions passed');
