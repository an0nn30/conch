// Run: node scripts/tests/test_editor_untitled.mjs
//
// Untitled buffers — a pane with NO file on disk — and `savePane` as the one
// place that knows what to do about it.
//
// The claim under test is a routing claim, so every fixture here discriminates
// on CALL COUNTS rather than on outcomes: "the dialog opened" and "the dialog
// did not open" are both assertions, and a diversion that fires for every pane
// passes an outcome-only test just as happily as the right one.
//
// Three properties, in the order they can lose a user's work:
//
//   1. A pathless pane never writes anywhere. savePane diverts to the Save As
//      chooser instead, and the chooser's own saveAs does the write and the
//      rebind — so savePane must NOT then save again.
//   2. Cancelling that chooser is not a failure. savePane rejects with
//      `SaveCancelled` so nothing treats it as saved, and NO catch-site turns
//      it into an error toast (a red flash for pressing Escape).
//   3. Cancelling is not a save either. The close guards abort, `:wq` does not
//      close, and the pane is left exactly as untitled and as dirty as it was.
//
// Real modules off disk: editor-service.js, tab-label.js, vim-mode.js. Only
// the invoke-backed IO and the two dialogs are stubbed, with the real shapes
// (same stub layer as test_editor_save_as.mjs, whose harness pattern this
// follows):
//   editor_write_file    Result<(), String>     -> resolves null
//   editor_temp_cleanup  Result<(), String>     -> resolves null
//   openForSave(pane)    -> the chooser's result object, or null when the
//                           user cancelled; never rejects
//   confirmSave(name)    -> 'save' | 'discard' | 'cancel'
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');
const SERVICE_PATH = path.join(APP, 'features/editor/editor-service.js');
const TAB_MANAGER_PATH = path.join(APP, 'tab-manager.js');
const VIM_MODE_PATH = path.join(APP, 'features/editor/vim-mode.js');

let failures = 0;
let ran = 0;
function check(name, fn) {
  ran++;
  try {
    fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}
async function checkAsync(name, fn) {
  ran++;
  try {
    await fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = async (n = 6) => { for (let i = 0; i < n; i++) await tick(); };

// Every await on a service promise goes through this: a promise that never
// settles is one of the failures these tests exist to catch, and an unguarded
// await on one wedges node with no message instead of failing.
const TIMEOUT_MS = 2000;
function settles(promise, what) {
  let timer = null;
  const bomb = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`timed out after ${TIMEOUT_MS}ms waiting for ${what}`)), TIMEOUT_MS);
  });
  return Promise.race([promise, bomb]).finally(() => { if (timer) clearTimeout(timer); });
}

// Resolves to the rejection, or fails if the promise resolved.
async function rejection(promise, what) {
  try {
    await settles(promise, what);
  } catch (error) {
    return error;
  }
  throw new Error(`${what} resolved, but it had to reject`);
}

// Same stand-in for the real EditorView as test_editor_save_as.mjs: `dirty` is
// a plain boolean and the update listener stops firing once it is set.
function makeView(initialDoc, onDirtyChange) {
  let doc = initialDoc;
  let dirty = false;
  return {
    get state() { return { doc: { toString: () => doc } }; },
    termlabResetDirty() { dirty = false; onDirtyChange(false); },
    change(nextDoc) {
      if (nextDoc === doc) return;
      doc = nextDoc;
      if (dirty) return;
      dirty = true;
      onDirtyChange(true);
    },
    type(text) { this.change(doc + text); },
  };
}

function load(sandbox, relPath) {
  vm.runInContext(fs.readFileSync(path.join(APP, relPath), 'utf8'), sandbox, { filename: relPath });
}

// ---------------------------------------------------------------------------
// The harness — real editor-service + real tab-label, everything else recorded
// ---------------------------------------------------------------------------
//
// options:
//   noBundle     no global.CM6, i.e. the vendor bundle never got built
//   dialog       (pane) => result | Promise<result>; `null` means cancelled.
//                Defaults to cancelling, so a test that forgets to say what
//                the user did gets the conservative answer.
//   confirmSave  (name) => 'save' | 'discard' | 'cancel'
//   parkWrites   editor_write_file hangs until the recorded entry is resolved
//   failWrite    editor_write_file rejects with this
function makeHarness(options = {}) {
  const sandbox = { console, setTimeout, clearTimeout };
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const toasts = [];
  const calls = [];
  const writes = [];
  const cleanups = [];
  const created = [];
  const dialogCalls = [];
  const confirmCalls = [];
  const activations = [];
  const labelCalls = [];
  const panes = new Map();
  const tabs = new Map();
  let nextPaneId = 1;

  if (!options.noBundle) sandbox.CM6 = {};
  sandbox.toast = {
    error: (title, body) => { toasts.push({ kind: 'error', title, body }); },
    success: (title, body) => { toasts.push({ kind: 'success', title, body }); },
    warn: (title, body) => { toasts.push({ kind: 'warn', title, body }); },
    info: (title, body) => { toasts.push({ kind: 'info', title, body }); },
  };

  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        calls.push({ command, args });
        switch (command) {
          case 'editor_write_file': {
            const entry = { path: args.path, contents: args.contents };
            writes.push(entry);
            if (!options.parkWrites) {
              return options.failWrite ? Promise.reject(options.failWrite) : Promise.resolve(null);
            }
            return new Promise((resolve, reject) => {
              entry.resolve = () => resolve(null);
              entry.reject = (e) => reject(e);
            });
          }
          case 'editor_read_file':
            // The real command rejects with a String for a path that is not
            // there, and `null` is certainly not there.
            if (!args.path) return Promise.reject('No such file or directory');
            return Promise.resolve('on-disk contents');
          case 'editor_temp_cleanup':
            cleanups.push(args.path);
            return Promise.resolve(null);
          default:
            return Promise.resolve(null);
        }
      },
      listen(eventName, handler) {
        return Promise.resolve(() => { void eventName; void handler; });
      },
    },
  };

  let focusedPaneId = null;
  sandbox.__termlabPaneAccess = {
    currentPane: () => (focusedPaneId == null ? null : panes.get(focusedPaneId) || null),
    allPanes: () => panes,
    setFocusedPane: (paneId) => { focusedPaneId = paneId; },
    activateTab: (tabId) => { activations.push(tabId); },
    setTabLabel: (tabId, label, tooltip) => {
      labelCalls.push({ tabId, label, tooltip });
      const tab = tabs.get(tabId);
      if (!tab) return false;
      tab.label = label;
      tab.tooltip = tooltip;
      return true;
    },
  };

  // Real module: the Untitled label formula is the thing being applied.
  load(sandbox, 'features/editor/tab-label.js');

  // Stands in for tab-manager's createEditorTab. It builds the pane literal
  // the same way and then labels it through the same tab-label module the real
  // one uses — the one structural fact this mirrors (that `untitledSeq` is
  // carried from the options onto the pane) is pinned against the real file by
  // the SOURCE CANARY below, so the mirror cannot drift silently.
  sandbox.__termlabCreateEditorTab = (opts) => {
    created.push(opts);
    const paneId = nextPaneId++;
    const pane = {
      paneId,
      tabId: 100 + paneId,
      kind: 'editor',
      filePath: opts.filePath || null,
      remote: opts.remote || null,
      untitledSeq: opts.untitledSeq || null,
      dirty: false,
    };
    pane.view = makeView(typeof opts.contents === 'string' ? opts.contents : '', (d) => { pane.dirty = d; });
    panes.set(paneId, pane);
    const { label, tooltip } = sandbox.termlabEditorTabLabel.editorTabLabel(pane);
    tabs.set(pane.tabId, { label, tooltip });
    focusedPaneId = paneId;
    return pane;
  };

  sandbox.termlabEditorPane = {
    setLanguage: () => {},
  };

  sandbox.termlabFileDialog = {
    openForSave: (pane) => {
      dialogCalls.push(pane);
      const fn = options.dialog || (() => null);
      return Promise.resolve(fn(pane));
    },
  };

  sandbox.termlabDialogService = {
    confirmSave: (name) => {
      confirmCalls.push(name);
      const fn = options.confirmSave || (() => 'cancel');
      return Promise.resolve(fn(name));
    },
  };

  vm.runInContext(fs.readFileSync(SERVICE_PATH, 'utf8'), sandbox, { filename: SERVICE_PATH });

  const service = sandbox.termlabEditorService;

  // A pane that already has a home, built directly: the "titled" half of every
  // both-directions fixture.
  function addTitledPane(filePath, doc) {
    const paneId = nextPaneId++;
    const pane = {
      paneId,
      tabId: 100 + paneId,
      kind: 'editor',
      filePath,
      remote: null,
      untitledSeq: null,
      dirty: false,
    };
    pane.view = makeView(typeof doc === 'string' ? doc : 'body text', (d) => { pane.dirty = d; });
    panes.set(paneId, pane);
    const seed = sandbox.termlabEditorTabLabel.editorTabLabel(pane);
    tabs.set(pane.tabId, { label: seed.label, tooltip: seed.tooltip });
    focusedPaneId = paneId;
    return pane;
  }

  return {
    sandbox,
    service,
    panes,
    tabs,
    tabOf: (pane) => tabs.get(pane.tabId),
    labelOf: (pane) => sandbox.termlabEditorTabLabel.editorTabLabel(pane),
    toasts,
    errorToasts: () => toasts.filter((t) => t.kind === 'error'),
    calls,
    writes,
    cleanups,
    created,
    dialogCalls,
    confirmCalls,
    activations,
    labelCalls,
    addTitledPane,
    closePane: (pane) => { panes.delete(pane.paneId); },
    focus: (pane) => { focusedPaneId = pane.paneId; },
  };
}

// The result object a real openForSave resolves with (file-dialog.js returns
// the chooser's `choice`). Nothing in editor-service reads its fields — only
// null-vs-not is load-bearing — so the shape is reproduced rather than mocked
// down to a bare `true`, which would hide a caller that started reading it.
const choiceFor = (p) => ({ scope: { kind: 'local', id: 'local' }, path: p, entry: null });

// A dialog stub that does what the REAL openForSave does: route the target
// through the service's own saveAs (which owns the write and the rebind), then
// resolve with the chooser result.
function savingDialog(service, targetPath) {
  return async (pane) => {
    await service.saveAs(pane, { scope: 'local', path: targetPath });
    return choiceFor(targetPath);
  };
}

const TARGET = '/home/dev/notes/gamma.md';

// ---------------------------------------------------------------------------
// 1. Naming — the per-window counter
// ---------------------------------------------------------------------------

console.log('editor untitled: naming');

await checkAsync('the first buffer is "Untitled" and the next ones are numbered', async () => {
  const h = makeHarness();
  h.service.openUntitled();
  h.service.openUntitled();
  h.service.openUntitled();
  await settle();

  assert.strictEqual(h.created.length, 3, 'three tabs were created');
  assert.deepStrictEqual(
    h.created.map((o) => o.untitledSeq),
    [1, 2, 3],
    'each buffer carries its own sequence number',
  );
  assert.deepStrictEqual(
    h.created.map((o) => o.filePath),
    [null, null, null],
    'and none of them has a path — nothing was written to disk',
  );
  assert.strictEqual(
    h.calls.filter((c) => c.command === 'editor_write_file').length,
    0,
    'creating an untitled buffer writes no file',
  );

  const labels = [...h.panes.values()].map((p) => h.labelOf(p));
  assert.deepStrictEqual(
    labels.map((l) => l.label),
    ['Untitled', 'Untitled-2', 'Untitled-3'],
    'the first is unnumbered, exactly as Notepad names them',
  );
  assert.deepStrictEqual(
    labels.map((l) => l.tooltip),
    ['Unsaved', 'Unsaved', 'Unsaved'],
    'and the tooltip says there is nowhere to hover-reveal',
  );
  assert.deepStrictEqual(
    [...h.panes.values()].map((p) => p.dirty),
    [false, false, false],
    'a buffer nobody has typed in is not modified',
  );
});

await checkAsync('numbers are not reused after a tab closes', async () => {
  const h = makeHarness();
  h.service.openUntitled();
  h.service.openUntitled();
  await settle();
  const [first, second] = [...h.panes.values()];
  assert.strictEqual(h.labelOf(second).label, 'Untitled-2', 'precondition');

  h.closePane(first);
  h.closePane(second);
  h.service.openUntitled();
  await settle();

  const survivor = [...h.panes.values()][0];
  assert.strictEqual(
    h.labelOf(survivor).label,
    'Untitled-3',
    'the counter only goes up: reusing "Untitled" for a third buffer would let a '
    + 'user think a tab they closed came back',
  );
});

await checkAsync('a missing editor bundle creates nothing and says so', async () => {
  const h = makeHarness({ noBundle: true });
  h.service.openUntitled();
  await settle();
  assert.strictEqual(h.created.length, 0, 'no tab');
  assert.strictEqual(h.errorToasts().length, 1, 'one toast explaining why');
});

// A pathless pane with NO sequence number is not an untitled buffer — it is a
// defensive call — and must keep the old lowercase fallback rather than
// claiming to be Untitled. (test_editor_tab_label.mjs pins that fallback; this
// says the new branch is keyed tightly enough not to swallow it.)
check('a pathless pane without a sequence number keeps the old fallback', () => {
  const h = makeHarness();
  const { editorTabLabel } = h.sandbox.termlabEditorTabLabel;
  // deepEqual, not deepStrictEqual: the object is built inside the vm context
  // and so has a different Object.prototype than this file's.
  assert.deepEqual(
    editorTabLabel({ filePath: null, remote: null }),
    { label: 'untitled', tooltip: '' },
  );
});

// The harness's createEditorTab stub mirrors tab-manager's pane literal. This
// reads the real file as TEXT and requires the one field the mirror depends on
// to be there — otherwise every naming assertion above would keep passing
// against a tab-manager that dropped `untitledSeq` on the floor and shipped
// tabs labelled "untitled" forever.
check('SOURCE CANARY: tab-manager carries untitledSeq onto the pane', () => {
  const src = fs.readFileSync(TAB_MANAGER_PATH, 'utf8');
  assert.match(
    src,
    /untitledSeq:\s*opts\.untitledSeq/,
    'createEditorTab must copy opts.untitledSeq onto the pane it builds — the '
    + 'label formula reads it from the pane',
  );
  assert.match(
    src,
    /filePath:\s*opts\.filePath\s*\|\|\s*null/,
    'and must still accept a null filePath rather than inventing one',
  );
});

// ---------------------------------------------------------------------------
// 2. The diversion — both directions, by call count
// ---------------------------------------------------------------------------

console.log('editor untitled: the savePane diversion');

await checkAsync('saving an untitled pane opens the chooser and writes nothing itself', async () => {
  const h = makeHarness({ dialog: null });
  h.service.openUntitled();
  await settle();
  const pane = [...h.panes.values()][0];
  pane.view.type('hello');

  const saving = h.service.savePane(pane);
  await rejection(saving, 'savePane on a cancelled untitled pane');

  assert.strictEqual(h.dialogCalls.length, 1, 'the Save As chooser opened exactly once');
  assert.strictEqual(h.dialogCalls[0], pane, 'for this pane');
  assert.strictEqual(h.writes.length, 0, 'and savePane wrote nothing to a path it does not have');
});

await checkAsync('saving a titled pane does NOT open the chooser', async () => {
  const h = makeHarness({ dialog: null });
  const pane = h.addTitledPane('/home/dev/notes/existing.md', 'body');
  pane.view.type('!');

  await settles(h.service.savePane(pane), 'savePane on a titled pane');

  assert.strictEqual(
    h.dialogCalls.length,
    0,
    'a file that already has a home is saved in place — a chooser here would '
    + 'ask the user where to put a file that is already somewhere',
  );
  assert.strictEqual(h.writes.length, 1, 'exactly one write');
  assert.strictEqual(h.writes[0].path, '/home/dev/notes/existing.md');
  assert.strictEqual(h.writes[0].contents, 'body!');
  assert.strictEqual(pane.dirty, false, 'and the pane is clean afterwards');
});

await checkAsync('a successful chooser rebinds the pane and saves ONCE', async () => {
  const h = makeHarness();
  h.sandbox.termlabFileDialog.openForSave = (pane) => {
    h.dialogCalls.push(pane);
    return savingDialog(h.service, TARGET)(pane);
  };
  h.service.openUntitled();
  await settle();
  const pane = [...h.panes.values()][0];
  pane.view.type('first draft');

  await settles(h.service.savePane(pane), 'savePane through a successful chooser');

  assert.strictEqual(h.dialogCalls.length, 1, 'one chooser');
  assert.strictEqual(
    h.writes.length,
    1,
    'ONE write: the chooser\'s saveAs already wrote the file, so savePane '
    + 'must not fall through and write it a second time',
  );
  assert.strictEqual(h.writes[0].path, TARGET, 'at the chosen path');
  assert.strictEqual(h.writes[0].contents, 'first draft', 'with the buffer contents');
  assert.strictEqual(pane.filePath, TARGET, 'the pane is rebound');
  assert.strictEqual(pane.remote, null, 'to a local file');
  assert.strictEqual(pane.dirty, false, 'and is no longer modified');
  assert.strictEqual(h.tabOf(pane).label, 'gamma.md', 'the tab stops saying Untitled');
  assert.strictEqual(h.tabOf(pane).tooltip, TARGET, 'and points at the real path');
  assert.strictEqual(h.errorToasts().length, 0, 'nothing went wrong, so nothing was reported');
});

await checkAsync('an ordinary save after the rebind takes the normal path', async () => {
  const h = makeHarness();
  h.sandbox.termlabFileDialog.openForSave = (pane) => {
    h.dialogCalls.push(pane);
    return savingDialog(h.service, TARGET)(pane);
  };
  h.service.openUntitled();
  await settle();
  const pane = [...h.panes.values()][0];
  pane.view.type('first draft');
  await settles(h.service.savePane(pane), 'the first save');

  pane.view.type(' plus more');
  await settles(h.service.savePane(pane), 'the second save');

  assert.strictEqual(h.dialogCalls.length, 1, 'the chooser is asked once and only once');
  assert.strictEqual(h.writes.length, 2, 'the second save wrote straight through');
  assert.strictEqual(h.writes[1].path, TARGET, 'to the path the first one bound');
});

await checkAsync('a second save while the chooser\'s save is still in flight joins it', async () => {
  // The pane is still pathless while its Save As uploads/writes, so a naive
  // "no filePath -> open the chooser" would put a SECOND chooser on screen on
  // top of the first.
  const h = makeHarness({ parkWrites: true });
  h.sandbox.termlabFileDialog.openForSave = (pane) => {
    h.dialogCalls.push(pane);
    return savingDialog(h.service, TARGET)(pane);
  };
  h.service.openUntitled();
  await settle();
  const pane = [...h.panes.values()][0];
  pane.view.type('draft');

  const first = h.service.savePane(pane);
  await settle();
  assert.strictEqual(h.writes.length, 1, 'precondition: the write is parked in flight');
  assert.strictEqual(pane.filePath, null, 'precondition: not rebound yet');

  const second = h.service.savePane(pane);
  await settle();
  assert.strictEqual(h.dialogCalls.length, 1, 'the second save did not open a second chooser');

  h.writes[0].resolve();
  await settles(Promise.all([first, second]), 'both saves');

  assert.strictEqual(h.writes.length, 1, 'and it did not write a second copy');
  assert.strictEqual(pane.filePath, TARGET, 'both are satisfied by the one rebind');
});

// ---------------------------------------------------------------------------
// 3. Cancel is not a failure
// ---------------------------------------------------------------------------

console.log('editor untitled: cancel');

await checkAsync('cancelling rejects with SaveCancelled and leaves the pane untouched', async () => {
  const h = makeHarness({ dialog: () => null });
  h.service.openUntitled();
  await settle();
  const pane = [...h.panes.values()][0];
  pane.view.type('unsaved work');

  const error = await rejection(h.service.savePane(pane), 'a cancelled save');
  assert.strictEqual(error.name, 'SaveCancelled', 'the sentinel name every catch-site keys on');
  assert.strictEqual(h.errorToasts().length, 0, 'pressing Escape is not an error');
  assert.strictEqual(pane.filePath, null, 'the pane still has no home');
  assert.strictEqual(pane.untitledSeq, 1, 'and is still that untitled buffer');
  assert.strictEqual(pane.dirty, true, 'with its unsaved work still unsaved');
  assert.strictEqual(h.writes.length, 0, 'nothing was written anywhere');
});

await checkAsync('⌘S on a cancelled untitled pane shows NO toast', async () => {
  const h = makeHarness({ dialog: () => null });
  h.service.openUntitled();
  await settle();
  const pane = [...h.panes.values()][0];
  pane.view.type('unsaved work');
  h.focus(pane);

  await settles(h.service.saveActiveEditor(), 'saveActiveEditor on a cancelled untitled pane');

  assert.strictEqual(h.toasts.length, 0, 'no toast of any kind for a deliberate cancel');
  assert.strictEqual(pane.filePath, null, 'and the pane is unchanged');
  assert.strictEqual(pane.dirty, true);
});

await checkAsync('⌘S on a REAL save failure still shows exactly one toast', async () => {
  // The other direction of the suppression: a name check that swallowed
  // everything would leave a failed write silent, which is the worse bug.
  const h = makeHarness({ failWrite: new Error('permission denied') });
  const pane = h.addTitledPane('/home/dev/notes/existing.md', 'body');
  pane.view.type('!');
  h.focus(pane);

  await settles(h.service.saveActiveEditor(), 'saveActiveEditor over a failing write');

  const errors = h.errorToasts();
  assert.strictEqual(errors.length, 1, 'a failure is still reported');
  assert.strictEqual(errors[0].title, 'Save Failed');
  assert.match(String(errors[0].body), /permission denied/);
  assert.strictEqual(pane.dirty, true, 'and the pane stays dirty');
});

// ---------------------------------------------------------------------------
// 4. Two untitled panes are never "the same file"
// ---------------------------------------------------------------------------

console.log('editor untitled: identity');

// focusExistingEditor is internal, so it is driven through openLocalFile —
// the same route ⌘O takes.
await checkAsync('an open never resolves to an untitled buffer', async () => {
  const h = makeHarness();
  h.service.openUntitled();
  h.service.openUntitled();
  await settle();
  const titled = h.addTitledPane('/home/dev/notes/real.md', 'body');
  const tabsBefore = h.panes.size;

  // A pathless open must not be answered with a pathless PANE. Without the
  // null guard, `pane.filePath === filePath` is true for the first untitled
  // buffer and the user who asked for a file gets handed their scratch
  // buffer instead — silently, with no error and no file.
  await settles(h.service.openLocalFile(null), 'openLocalFile(null)');
  assert.strictEqual(h.activations.length, 0, 'nothing was focused');
  assert.strictEqual(h.panes.size, tabsBefore, 'and nothing was opened');
  assert.strictEqual(h.errorToasts().length, 1, 'the open failed, and said so');

  // A real path still resolves to the tab that holds it.
  await settles(h.service.openLocalFile('/home/dev/notes/real.md'), 'openLocalFile on an open file');
  assert.deepStrictEqual(h.activations, [titled.tabId], 'the existing tab is focused');
  assert.strictEqual(h.panes.size, tabsBefore, 'and no second view of it is made');

  // And a path nobody holds still opens, with two untitled panes in the map.
  await settles(h.service.openLocalFile('/home/dev/notes/other.md'), 'openLocalFile on a new file');
  assert.strictEqual(h.panes.size, tabsBefore + 1, 'a new tab');
  assert.strictEqual(h.activations.length, 1, 'and no extra activation');
});

await checkAsync('one untitled pane does not block another from saving anywhere', async () => {
  const h = makeHarness();
  h.service.openUntitled();
  h.service.openUntitled();
  await settle();
  const [first] = [...h.panes.values()];

  await settles(h.service.saveAs(first, { scope: 'local', path: TARGET }), 'Save As on the first untitled pane');

  assert.strictEqual(first.filePath, TARGET, 'it rebound');
  assert.strictEqual(h.errorToasts().length, 0, 'the other untitled pane was not read as holding the path');
});

// ---------------------------------------------------------------------------
// 5. The close guards
// ---------------------------------------------------------------------------

console.log('editor untitled: close guards');

await checkAsync('Save on an untitled pane, cancelled at the chooser, aborts the close', async () => {
  const h = makeHarness({ dialog: () => null, confirmSave: () => 'save' });
  h.service.openUntitled();
  h.service.openUntitled();
  await settle();
  const [pane] = [...h.panes.values()];
  pane.view.type('work in progress');

  const ok = await settles(h.service.confirmDirtyPanes([pane]), 'confirmDirtyPanes');

  assert.strictEqual(ok, false, 'the close is abandoned — a cancelled save is not consent to lose the buffer');
  assert.strictEqual(h.dialogCalls.length, 1, 'the chooser was opened once');
  assert.deepStrictEqual(h.confirmCalls, ['Untitled'], 'the prompt names the buffer the way its tab does');
  assert.strictEqual(h.errorToasts().length, 0, 'and cancelling it is not an error');
  assert.strictEqual(pane.filePath, null, 'the pane is untouched: still untitled');
  assert.strictEqual(pane.dirty, true, 'still dirty');
  assert.strictEqual(h.writes.length, 0, 'and nothing was written');
});

await checkAsync('Save on an untitled pane that succeeds lets the close through', async () => {
  const h = makeHarness({ confirmSave: () => 'save' });
  h.sandbox.termlabFileDialog.openForSave = (pane) => {
    h.dialogCalls.push(pane);
    return savingDialog(h.service, TARGET)(pane);
  };
  h.service.openUntitled();
  await settle();
  const [pane] = [...h.panes.values()];
  pane.view.type('work in progress');

  const ok = await settles(h.service.confirmDirtyPanes([pane]), 'confirmDirtyPanes');

  assert.strictEqual(ok, true, 'saved, so the close proceeds');
  assert.strictEqual(pane.filePath, TARGET, 'and the buffer now has a home');
  assert.strictEqual(pane.dirty, false);
  assert.strictEqual(h.errorToasts().length, 0);
});

await checkAsync('a genuinely failed save inside the close guard still toasts and aborts', async () => {
  const h = makeHarness({ confirmSave: () => 'save', failWrite: new Error('disk full') });
  const pane = h.addTitledPane('/home/dev/notes/existing.md', 'body');
  pane.view.type('!');

  const ok = await settles(h.service.confirmDirtyPanes([pane]), 'confirmDirtyPanes over a failing write');

  assert.strictEqual(ok, false, 'still refuses to close');
  const errors = h.errorToasts();
  assert.strictEqual(errors.length, 1, 'and still says why — suppression is for cancels only');
  assert.strictEqual(errors[0].title, 'Save Failed');
});

await checkAsync('a Save As that FAILS at the chooser reports ONCE and still aborts', async () => {
  // openForSave resolves null for a failed saveAs as well as for a cancel,
  // having already toasted the failure itself. So the sentinel is raised for
  // something that really did go wrong — and the requirement is that it is
  // reported exactly once (not zero times, and not twice).
  const h = makeHarness({ confirmSave: () => 'save', failWrite: new Error('disk full') });
  h.sandbox.termlabFileDialog.openForSave = async (pane) => {
    h.dialogCalls.push(pane);
    try {
      await h.service.saveAs(pane, { scope: 'local', path: TARGET });
    } catch (_) {
      return null; // exactly what the real openForSave does
    }
    return choiceFor(TARGET);
  };
  h.service.openUntitled();
  await settle();
  const [pane] = [...h.panes.values()];
  pane.view.type('work in progress');

  const ok = await settles(h.service.confirmDirtyPanes([pane]), 'confirmDirtyPanes over a failed Save As');

  assert.strictEqual(ok, false, 'the close is abandoned');
  const errors = h.errorToasts();
  assert.strictEqual(errors.length, 1, 'one report, from saveAs — the close guard must not say it again');
  assert.strictEqual(errors[0].title, 'Save As Failed');
  assert.strictEqual(pane.filePath, null, 'and the buffer is still untitled');
  assert.strictEqual(pane.dirty, true, 'and still dirty');
});

await checkAsync('a clean untitled buffer is never asked about', async () => {
  const h = makeHarness({ confirmSave: () => 'cancel' });
  h.service.openUntitled();
  await settle();
  const [pane] = [...h.panes.values()];

  const ok = await settles(h.service.confirmDirtyPanes([pane]), 'confirmDirtyPanes on a clean buffer');

  assert.strictEqual(ok, true, 'an untouched buffer closes silently, exactly as Notepad does');
  assert.strictEqual(h.confirmCalls.length, 0, 'nothing was asked');
  assert.strictEqual(h.dialogCalls.length, 0, 'and no chooser appeared');
});

// ---------------------------------------------------------------------------
// 6. vim — :w and :wq over a cancelled chooser
// ---------------------------------------------------------------------------
//
// Real vim-mode.js with a recorded `savePane`, because the sentinel has to
// survive the ex-command layer too: `:wq` closing the tab after a cancelled
// chooser would discard the buffer outright.

console.log('editor untitled: vim');

function makeVimHarness(saveOutcome) {
  const sandbox = { console, setTimeout, clearTimeout, Promise };
  sandbox.window = sandbox;
  const defined = [];
  sandbox.CM6 = { Vim: { defineEx(name, prefix, func) { defined.push({ name, prefix, func }); } } };
  const toasts = [];
  sandbox.toast = {
    error: (title, body) => { toasts.push({ title, body }); },
    success() {}, warn() {}, info() {},
  };
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(VIM_MODE_PATH, 'utf8'), sandbox, { filename: VIM_MODE_PATH });

  const pane = { paneId: 1, tabId: 101, kind: 'editor', view: {}, filePath: null, untitledSeq: 1, dirty: true };
  const closes = [];
  const saves = [];
  sandbox.termlabVimMode.registerExCommands({
    savePane: (p) => { saves.push(p); return saveOutcome(); },
    closeTab: (tabId) => { closes.push(tabId); return Promise.resolve(); },
    currentPane: () => pane,
  });
  return {
    pane, closes, saves, toasts,
    run: (name) => defined.find((e) => e.name === name).func({}, { commandName: name }),
  };
}

const cancelled = () => {
  const error = new Error('save cancelled');
  error.name = 'SaveCancelled';
  return Promise.reject(error);
};

await checkAsync(':w on an untitled pane, cancelled, is silent', async () => {
  const h = makeVimHarness(cancelled);
  h.run('write');
  await settle();
  assert.strictEqual(h.saves.length, 1, 'the save was attempted');
  assert.strictEqual(h.toasts.length, 0, 'and cancelling it said nothing');
});

await checkAsync(':wq on an untitled pane, cancelled, does not close the tab', async () => {
  const h = makeVimHarness(cancelled);
  h.run('wq');
  await settle();
  assert.strictEqual(h.saves.length, 1, 'the save was attempted');
  assert.deepStrictEqual(h.closes, [], 'the tab stays: closing it would discard the buffer');
  assert.strictEqual(h.toasts.length, 0, 'and nothing red flashed');
});

await checkAsync(':wq over a real save failure also refuses to close, and DOES report', async () => {
  const h = makeVimHarness(() => Promise.reject(new Error('disk full')));
  h.run('wq');
  await settle();
  assert.deepStrictEqual(h.closes, [], 'still no close');
  assert.strictEqual(h.toasts.length, 1, 'but a real failure is still reported');
  assert.match(String(h.toasts[0].title), /Save Failed/);
});

await checkAsync(':wq on a successful save closes the tab', async () => {
  const h = makeVimHarness(() => Promise.resolve());
  h.run('wq');
  await settle();
  assert.deepStrictEqual(h.closes, [101], 'saved, so it closes');
  assert.strictEqual(h.toasts.length, 0);
});

// ---------------------------------------------------------------------------

if (failures) {
  console.error(`\neditor untitled: ${failures} of ${ran} checks FAILED`);
  process.exit(1);
}
console.log(`editor untitled: all ${ran} checks passed`);
