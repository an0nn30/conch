// Run: node scripts/tests/test_scratch_naming.mjs
//
// Scratch names must not collide with files already in the scratch directory,
// including ones the user renamed by hand.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/editor/scratch.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { nextScratchName } = sandbox.termlabEditorScratch;

assert.strictEqual(nextScratchName([]), 'scratch-1.txt');
assert.strictEqual(nextScratchName(['scratch-1.txt']), 'scratch-2.txt');

// Gaps are filled rather than skipped — the first free number wins.
assert.strictEqual(nextScratchName(['scratch-1.txt', 'scratch-3.txt']), 'scratch-2.txt');

// Unrelated files in the directory are ignored.
assert.strictEqual(nextScratchName(['notes.md', 'scratch-1.txt']), 'scratch-2.txt');

// Never collides with an existing name, whatever the ordering.
assert.strictEqual(
  nextScratchName(['scratch-3.txt', 'scratch-1.txt', 'scratch-2.txt']),
  'scratch-4.txt',
);

// Degenerate input does not throw.
assert.strictEqual(nextScratchName(null), 'scratch-1.txt');
assert.strictEqual(nextScratchName(undefined), 'scratch-1.txt');

console.log('scratch naming: all assertions passed');
