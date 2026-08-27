// Run: node scripts/tests/test_open_path_routing.mjs
//
// Startup routing for paths handed in on the CLI or a second-instance IPC
// arrival: Rust queues them per-window (`take_pending_open_paths`) and this
// module drains that queue once, stats each path, and routes it — a file
// opens in the editor, a directory gets a "coming soon" toast (the seam the
// LSP/workspace branch later replaces), and a path that no longer exists
// gets an error naming it. One bad path must never block the rest, so
// routing is sequential and catches per-path.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

function load({ pending, statMap }) {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  const file = path.join(APP, 'features/editor/open-path-routing.js');
  vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });

  const calls = { opened: [], errors: [], infos: [] };
  let drained = false;
  const routing = sandbox.termlabOpenPathRouting.create({
    invoke: async (cmd, args) => {
      if (cmd === 'take_pending_open_paths') {
        if (drained) return [];
        drained = true;
        return pending;
      }
      if (cmd === 'local_stat') {
        if (!(args.path in statMap)) throw new Error('no such file');
        return statMap[args.path];
      }
      throw new Error('unexpected command ' + cmd);
    },
    openLocalFile: (p) => { calls.opened.push(p); },
    toastError: (title, body) => { calls.errors.push(body); },
    toastInfo: (title, body) => { calls.infos.push(body); },
  });
  return { routing, calls, sandbox };
}

// file -> editor; directory -> coming-soon info; missing -> error naming the path
{
  const { routing, calls } = load({
    pending: ['/tmp/a.txt', '/tmp/dir', '/tmp/ghost.txt'],
    statMap: {
      '/tmp/a.txt': { name: 'a.txt', is_dir: false, size: 12, modified: 1700000000, permissions: null },
      '/tmp/dir': { name: 'dir', is_dir: true, size: 0, modified: 1700000000, permissions: null },
    },
  });
  await routing.drainPendingOpens();
  assert.deepStrictEqual(JSON.parse(JSON.stringify(calls.opened)), ['/tmp/a.txt']);
  assert.strictEqual(calls.infos.length, 1, 'directory gets the coming-soon toast');
  assert.strictEqual(calls.errors.length, 1);
  assert.ok(calls.errors[0].includes('/tmp/ghost.txt'), 'error names the path');
}

// empty queue: no invokes beyond the pull, no toasts
{
  const { routing, calls } = load({ pending: [], statMap: {} });
  await routing.drainPendingOpens();
  assert.deepStrictEqual(calls.opened, []);
  assert.strictEqual(calls.errors.length + calls.infos.length, 0);
}

// sequential, deterministic order: opens happen in queue order even with
// mixed outcomes interleaved
{
  const { routing, calls } = load({
    pending: ['/tmp/one.txt', '/tmp/missing.txt', '/tmp/two.txt'],
    statMap: {
      '/tmp/one.txt': { name: 'one.txt', is_dir: false, size: 1, modified: null, permissions: null },
      '/tmp/two.txt': { name: 'two.txt', is_dir: false, size: 2, modified: null, permissions: null },
    },
  });
  await routing.drainPendingOpens();
  assert.deepStrictEqual(calls.opened, ['/tmp/one.txt', '/tmp/two.txt']);
  assert.strictEqual(calls.errors.length, 1);
  assert.ok(calls.errors[0].includes('/tmp/missing.txt'));
}

// draining twice only pulls the queue once per call to drainPendingOpens;
// the second drain sees an empty queue (take_pending_open_paths already
// drained it server-side) and does nothing
{
  const { routing, calls } = load({
    pending: ['/tmp/a.txt'],
    statMap: { '/tmp/a.txt': { name: 'a.txt', is_dir: false, size: 1, modified: null, permissions: null } },
  });
  await routing.drainPendingOpens();
  await routing.drainPendingOpens();
  assert.deepStrictEqual(calls.opened, ['/tmp/a.txt']);
}

// the exported DIRECTORY_COMING_SOON constant is the seam later work
// replaces, and it must actually be the copy handed to toastInfo
{
  const { routing, calls, sandbox } = load({
    pending: ['/tmp/dir'],
    statMap: { '/tmp/dir': { name: 'dir', is_dir: true, size: 0, modified: null, permissions: null } },
  });
  await routing.drainPendingOpens();
  assert.strictEqual(calls.infos.length, 1);
  assert.strictEqual(calls.infos[0], sandbox.termlabOpenPathRouting.DIRECTORY_COMING_SOON);
}

console.log('test_open_path_routing: all assertions passed');
