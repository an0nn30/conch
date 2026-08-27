// Run: node scripts/tests/test_open_path_wiring.mjs
//
// The WIRING between the event-wiring runtime and the open-path routing
// module. `test_open_path_routing.mjs` covers what the routing module does
// with a queue once it has one; this covers whether the runtime ever hands
// it one at all.
//
// That seam is a guard clause over two globals that are composed elsewhere
// (`termlabOpenPathRouting`, `termlabEditorService`). Nothing else asserts
// it, so a rename or a composition-order change on either side would turn
// `termlab notes.md` into a silent no-op with every routing test still
// green.
//
// Both real files are loaded — no stub stands in for the module under test.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

function loadRuntime() {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const rel of ['features/editor/open-path-routing.js', 'event-wiring-runtime.js']) {
    const file = path.join(APP, rel);
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return sandbox;
}

// The runtime exposes the drain wiring as a named seam rather than burying it
// inside init(), so it can be driven without stubbing forty unrelated deps.
{
  const sandbox = loadRuntime();
  assert.strictEqual(
    typeof sandbox.termlabEventWiringRuntime.wirePendingOpenDrain,
    'function',
    'event-wiring-runtime must export wirePendingOpenDrain',
  );
}

// Both globals present: the wiring builds a router and its drain pulls the
// queue through `take_pending_open_paths` and routes a file to the editor.
{
  const sandbox = loadRuntime();
  const calls = { invoked: [], opened: [], toasts: [] };
  sandbox.termlabEditorService = {
    openLocalFile: (p) => { calls.opened.push(p); },
  };
  sandbox.toast = {
    error: (title, body) => { calls.toasts.push(['error', title, body]); },
    info: (title, body) => { calls.toasts.push(['info', title, body]); },
  };

  const invoke = async (cmd, args) => {
    calls.invoked.push(cmd);
    if (cmd === 'take_pending_open_paths') return ['/tmp/notes.md'];
    if (cmd === 'local_stat') {
      assert.strictEqual(args.path, '/tmp/notes.md');
      return { name: 'notes.md', is_dir: false, size: 3, modified: null, permissions: null };
    }
    throw new Error('unexpected command ' + cmd);
  };

  const drain = sandbox.termlabEventWiringRuntime.wirePendingOpenDrain(sandbox, { invoke });
  assert.ok(drain, 'wiring must produce a drain when both globals are composed');
  assert.strictEqual(typeof drain.drainPendingOpens, 'function');

  await drain.drainPendingOpens();
  assert.deepStrictEqual(calls.invoked, ['take_pending_open_paths', 'local_stat']);
  assert.deepStrictEqual(calls.opened, ['/tmp/notes.md'], 'the file must reach openLocalFile');
  assert.deepStrictEqual(calls.toasts, [], 'a plain file open raises no toast');
}

// The toast deps really are wired to the global toast system (a directory is
// the cheapest path that produces one).
{
  const sandbox = loadRuntime();
  const toasts = [];
  sandbox.termlabEditorService = { openLocalFile: () => { throw new Error('must not open a directory'); } };
  sandbox.toast = {
    error: (title, body) => { toasts.push(['error', body]); },
    info: (title, body) => { toasts.push(['info', body]); },
  };
  const invoke = async (cmd) => {
    if (cmd === 'take_pending_open_paths') return ['/tmp/dir'];
    return { name: 'dir', is_dir: true, size: 0, modified: null, permissions: null };
  };
  const drain = sandbox.termlabEventWiringRuntime.wirePendingOpenDrain(sandbox, { invoke });
  await drain.drainPendingOpens();
  assert.strictEqual(toasts.length, 1);
  assert.deepStrictEqual(
    toasts[0],
    ['info', sandbox.termlabOpenPathRouting.DIRECTORY_COMING_SOON],
  );
}

// A missing toast global must not throw — the wiring's toast deps are
// optional passthroughs, not required collaborators.
{
  const sandbox = loadRuntime();
  sandbox.termlabEditorService = { openLocalFile: () => {} };
  const invoke = async (cmd) => {
    if (cmd === 'take_pending_open_paths') return ['/tmp/dir'];
    return { name: 'dir', is_dir: true, size: 0, modified: null, permissions: null };
  };
  const drain = sandbox.termlabEventWiringRuntime.wirePendingOpenDrain(sandbox, { invoke });
  await drain.drainPendingOpens();
}

// Guard clauses: a panel-host window has no editor service, and a build that
// dropped the routing script has no router. Neither may throw, and neither
// may produce a drain that would call into a missing global.
{
  const noEditor = loadRuntime();
  assert.strictEqual(
    noEditor.termlabEventWiringRuntime.wirePendingOpenDrain(noEditor, { invoke: async () => [] }),
    null,
    'no editor service -> no drain',
  );

  const badEditor = loadRuntime();
  badEditor.termlabEditorService = { openLocalFile: 'not a function' };
  assert.strictEqual(
    badEditor.termlabEventWiringRuntime.wirePendingOpenDrain(badEditor, { invoke: async () => [] }),
    null,
    'openLocalFile must be callable, not merely present',
  );

  const noRouting = loadRuntime();
  noRouting.termlabOpenPathRouting = undefined;
  noRouting.termlabEditorService = { openLocalFile: () => {} };
  assert.strictEqual(
    noRouting.termlabEventWiringRuntime.wirePendingOpenDrain(noRouting, { invoke: async () => [] }),
    null,
    'no routing module -> no drain',
  );
}

// The boot order inverted when editor windows arrived: main-runtime now
// pulls the queue (take_pending_open_paths) and routes it BEFORE creating
// any tab — a window with CLI paths gets ONLY editor tabs (no terminal to
// race), and a window without them creates the terminal tab as always.
// These source assertions pin that ordering and the fallback wiring.
{
  const src = fs.readFileSync(path.join(APP, 'main-runtime.js'), 'utf8');
  const pull = src.indexOf("take_pending_open_paths");
  const route = src.indexOf('routePendingPaths(');
  const firstTab = src.indexOf('createTab().catch');
  assert.ok(pull !== -1, 'main-runtime pulls the queue itself');
  assert.ok(route !== -1, 'main-runtime routes the pulled paths');
  assert.ok(firstTab !== -1, 'the default terminal tab still exists for normal windows');
  assert.ok(pull < firstTab && route < firstTab,
    'the pull+route must precede default-tab creation: an editor window '
    + 'skips the terminal tab entirely, so nothing may race the editor tab');
  assert.ok(src.indexOf('__termlabEditorWindow') !== -1,
    'the editor-window flag wires the close-all terminal fallback');
}

console.log('test_open_path_wiring: all assertions passed');
