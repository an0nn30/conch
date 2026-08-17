// Run: node scripts/tests/test_import_preview.mjs
//
// No jsdom in this repo (see test_export_picker.mjs's precedent). summarise
// and suggestRename are pure functions over plain data, so they are
// exercised directly by loading the module against a trivial `window` stub
// — mount()'s DOM-driving behaviour needs a real browser and is verified by
// hand per the task-5 brief.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/ssh/import-preview.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { summarise, suggestRename, suggestRenameFor, findLabelCollisions } = sandbox.termlabImportPreview;
assert.ok(typeof summarise === 'function', 'summarise must be exported');
assert.ok(typeof suggestRename === 'function', 'suggestRename must be exported');
assert.ok(typeof suggestRenameFor === 'function', 'suggestRenameFor must be exported');
assert.ok(typeof findLabelCollisions === 'function', 'findLabelCollisions must be exported');

// summarise counts by action, in the order the footer displays them.
assert.strictEqual(
  summarise([{ action: 'add' }, { action: 'add' }, { action: 'replace' }, { action: 'skip' }, { action: 'rename' }]),
  '2 new, 1 replace, 1 skip, 1 rename',
);
assert.strictEqual(summarise([{ action: 'add' }]), '1 new');
assert.strictEqual(summarise([]), 'Nothing to import');

// suggestRename appends a counter and keeps going until the name is free.
assert.strictEqual(suggestRename('prod', ['prod']), 'prod (2)');
assert.strictEqual(suggestRename('prod', ['prod', 'prod (2)']), 'prod (3)');
assert.strictEqual(suggestRename('prod', []), 'prod');
assert.strictEqual(suggestRename('prod', ['other']), 'prod');

// Fix round 1, finding 1: suggestRenameFor must account for what OTHER rows
// in the same batch are about to become, not just each row's own original
// label — otherwise two same-kind rows that both originally collided with
// the same local label (e.g. two hosts both named "prod") independently
// suggest the identical "prod (2)" and the import recreates the exact
// collision it exists to resolve.
{
  const entries = [
    { kind: 'host', label: 'prod', action: 'add', renameLabel: '' },
    { kind: 'host', label: 'prod', action: 'add', renameLabel: '' },
  ];
  const first = suggestRenameFor(entries, 0);
  assert.strictEqual(first, 'prod (2)', 'first row suggests the usual (2)');

  // Simulate the first row's rename having been chosen (mount() would have
  // written this back into the row's own live state before computing a
  // suggestion for any other row).
  entries[0] = { ...entries[0], action: 'rename', renameLabel: first };

  const second = suggestRenameFor(entries, 1);
  assert.strictEqual(second, 'prod (3)', 'second row must avoid the label the first row just chose');
}

// A row with action 'skip' contributes nothing to the collision set — it
// creates nothing under this import, so it must not consume a suggestion
// slot for a sibling that IS being imported.
{
  const entries = [
    { kind: 'host', label: 'prod', action: 'skip', renameLabel: '' },
    { kind: 'host', label: 'prod', action: 'add', renameLabel: '' },
  ];
  assert.strictEqual(suggestRenameFor(entries, 1), 'prod (2)', 'a skipped sibling must not be treated as taken');
}

// findLabelCollisions: the submit-time guard. Two decisions resolving to the
// same (kind, label) must be reported; distinct labels must not be.
{
  const rows = [
    { kind: 'host', id: 'a', label: 'prod' },
    { kind: 'host', id: 'b', label: 'prod' },
  ];
  const colliding = [
    { kind: 'host', id: 'a', action: 'rename', label: 'prod (2)' },
    { kind: 'host', id: 'b', action: 'rename', label: 'prod (2)' },
  ];
  const collisions = findLabelCollisions(rows, colliding);
  assert.strictEqual(collisions.length, 1, 'two rows resolving to the same label must be reported as one collision group');
  assert.strictEqual(collisions[0].length, 2, 'the collision group must name both offending rows');

  const distinct = [
    { kind: 'host', id: 'a', action: 'rename', label: 'prod (2)' },
    { kind: 'host', id: 'b', action: 'rename', label: 'prod (3)' },
  ];
  assert.strictEqual(findLabelCollisions(rows, distinct).length, 0, 'distinct labels must not be reported');

  // A skipped row creates nothing, so it must never be reported as
  // colliding with a row that keeps the same original label.
  const oneSkipped = [
    { kind: 'host', id: 'a', action: 'skip' },
    { kind: 'host', id: 'b', action: 'add' },
  ];
  assert.strictEqual(findLabelCollisions(rows, oneSkipped).length, 0, 'a skipped row must not collide with anything');
}

console.log('import preview summarise/suggestRename/suggestRenameFor/findLabelCollisions: all assertions passed');
