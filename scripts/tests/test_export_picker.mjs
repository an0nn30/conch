// Run: node scripts/tests/test_export_picker.mjs
//
// The export picker's filter is the only thing standing between a user and a
// few hundred saved connections, so its matching rule gets real coverage.
//
// No jsdom in this repo (see test_tl_dialog.mjs for the precedent). matchesFilter
// is a pure function over a precomputed haystack string, so it is exercised
// directly by loading the module against a trivial `window` stub.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/ssh/export-picker.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { matchesFilter, mount } = sandbox.termlabExportPicker;
assert.ok(typeof matchesFilter === 'function', 'matchesFilter must be exported');
assert.ok(typeof mount === 'function', 'mount must be exported');

const ROW = 'prod-db ubuntu@10.0.0.7:22';

// An empty or whitespace query matches everything: the list is unfiltered on open.
assert.ok(matchesFilter(ROW, ''), 'empty query matches');
assert.ok(matchesFilter(ROW, '   '), 'whitespace-only query matches');
assert.ok(matchesFilter(ROW, null), 'null query matches');

// Substring matching across the whole row, case-insensitively.
assert.ok(matchesFilter(ROW, 'prod'), 'matches the label');
assert.ok(matchesFilter(ROW, 'ubuntu'), 'matches the user');
assert.ok(matchesFilter(ROW, '10.0.0.7'), 'matches the host');
assert.ok(matchesFilter(ROW, 'PROD'), 'is case-insensitive');
assert.ok(matchesFilter(ROW, '  prod  '), 'trims the query');

// Every term must match, and order must not matter — "prod ubuntu" should find
// a prod host whose user is ubuntu even though the user appears after the label.
assert.ok(matchesFilter(ROW, 'prod ubuntu'), 'all terms present, in order');
assert.ok(matchesFilter(ROW, 'ubuntu prod'), 'all terms present, reversed');
assert.ok(!matchesFilter(ROW, 'prod staging'), 'one term missing fails the whole query');

assert.ok(!matchesFilter(ROW, 'zzz'), 'non-matching query fails');
assert.ok(!matchesFilter('', 'prod'), 'empty haystack fails a real query');
assert.ok(!matchesFilter(null, 'prod'), 'null haystack fails a real query');

console.log('export picker filter: all assertions passed');
