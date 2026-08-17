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

const { summarise, suggestRename } = sandbox.termlabImportPreview;
assert.ok(typeof summarise === 'function', 'summarise must be exported');
assert.ok(typeof suggestRename === 'function', 'suggestRename must be exported');

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

console.log('import preview summarise/suggestRename: all assertions passed');
