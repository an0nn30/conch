// Run: node scripts/tests/test_tab_switcher_mru.mjs
//
// Pure most-recently-used ordering for the ctrl+tab switcher: activation
// history maintenance and wrap-around selection stepping. No DOM.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/tab-switcher/mru.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { create, stepIndex } = sandbox.termlabTabMru;

// Arrays cross the vm realm boundary with a foreign Array prototype, which
// deepStrictEqual rejects; JSON-roundtrip flattens them into this realm.
const plain = (x) => JSON.parse(JSON.stringify(x));

// --- activation history ------------------------------------------------------
{
  const mru = create();
  mru.touch(1);
  mru.touch(2);
  mru.touch(3);
  assert.deepStrictEqual(plain(mru.order([1, 2, 3])), [3, 2, 1], 'most recent first');

  mru.touch(2);
  assert.deepStrictEqual(plain(mru.order([1, 2, 3])), [2, 3, 1], 're-touch moves to front');

  mru.remove(3);
  assert.deepStrictEqual(plain(mru.order([1, 2])), [2, 1], 'removed tab drops out');
}

// --- order() is defensive against ids it never saw ---------------------------
{
  const mru = create();
  mru.touch(2);
  assert.deepStrictEqual(
    plain(mru.order([9, 2, 7])),
    [2, 9, 7],
    'unseen tabs append after known ones, keeping their given order',
  );
}

// --- order() filters ids that no longer exist --------------------------------
{
  const mru = create();
  mru.touch(1);
  mru.touch(2);
  assert.deepStrictEqual(plain(mru.order([2])), [2], 'closed tabs never resurface');
}

// --- wrap-around stepping ----------------------------------------------------
assert.strictEqual(stepIndex(3, 0, 1), 1);
assert.strictEqual(stepIndex(3, 2, 1), 0, 'forward wraps to start');
assert.strictEqual(stepIndex(3, 0, -1), 2, 'backward wraps to end');
assert.strictEqual(stepIndex(1, 0, 1), 0, 'single item stays put');
assert.strictEqual(stepIndex(0, 0, 1), 0, 'empty list is safe');

console.log('test_tab_switcher_mru: all assertions passed');
