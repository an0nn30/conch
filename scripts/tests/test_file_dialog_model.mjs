// Run: node scripts/tests/test_file_dialog_model.mjs
//
// Pure model for the file-open/save-as dialog (Tasks 5-6 build UI on top of
// this): sorting, filtering, breadcrumbs, and path arithmetic. No DOM, no
// invokes — table-driven so it is exhaustively testable here.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/editor/file-dialog-model.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { sortEntries, filterEntries, splitBreadcrumbs, joinPath, parentPath } =
  sandbox.termlabFileDialogModel;

function entry(name, isDir) {
  return { name, is_dir: isDir, size: 0, modified: null, permissions: null };
}

// `deepStrictEqual` fails on arrays produced inside the vm sandbox (a
// different realm than the host array literal it is compared against), even
// when their contents are equal — so every comparison here is field-by-field
// against a plain host-realm array of expected values instead.
function assertNames(list, expectedNames, description) {
  assert.strictEqual(list.length, expectedNames.length, `${description}: length`);
  list.forEach((e, i) => {
    assert.strictEqual(e.name, expectedNames[i], `${description}: [${i}].name`);
  });
}

function assertEmpty(list, description) {
  assert.strictEqual(list.length, 0, description);
}

// --- sortEntries ---------------------------------------------------------

// Fixture is chosen so a naive alphabetical-only sort AND a naive
// case-sensitive sort both disagree with the correct output:
//  - "zzz-dir" (a dir) must sort before "aaa-file" (a plain file) even
//    though 'z' > 'a' — dir-first beats alphabetical order.
//  - Among the files, "apple.txt" must sort before "Banana.txt" even though
//    case-sensitive/ASCII order puts 'B' (66) before 'a' (97) — that only
//    happens under a correct case-insensitive comparison.
{
  const input = [
    entry('apple.txt', false),
    entry('Banana.txt', false),
    entry('zzz-dir', true),
    entry('aaa-file', false),
  ];
  const sorted = sortEntries(input);
  assertNames(
    sorted,
    ['zzz-dir', 'aaa-file', 'apple.txt', 'Banana.txt'],
    'sortEntries: dirs first, then case-insensitive name order',
  );
  // Original array must not be mutated.
  assert.strictEqual(input[0].name, 'apple.txt', 'sortEntries: does not mutate input order in place');
}

// Empty/null input never throws and yields an empty array.
assertEmpty(sortEntries([]), 'sortEntries: empty array in, empty array out');
assertEmpty(sortEntries(null), 'sortEntries: null in, empty array out');
assertEmpty(sortEntries(undefined), 'sortEntries: undefined in, empty array out');

// --- filterEntries ---------------------------------------------------------

{
  const input = [
    entry('readme.md', false),
    entry('notes-final.txt', false),
    entry('final-notes.txt', false),
    entry('.hidden', false),
    entry('.git', true),
  ];

  // Multi-term query: every term must appear (order-independent), proving
  // AND semantics rather than OR — a query whose terms match different
  // entries individually must not match either of them.
  assertNames(
    filterEntries(input, 'notes final', true),
    ['notes-final.txt', 'final-notes.txt'],
    'filterEntries: multi-term query requires every term present',
  );

  // A term that is a substring of one entry's name but absent from the
  // query-matching entry's *other* required term must exclude it — this is
  // the case a `.some()`-based (OR) implementation would wrongly pass,
  // since "notes" alone is a substring of "notes-final.txt".
  assertEmpty(
    filterEntries(input, 'notes missing-term', true),
    'filterEntries: a query term absent from the name excludes the entry even if other terms match',
  );

  // Case-insensitive.
  assertNames(
    filterEntries(input, 'README', true),
    ['readme.md'],
    'filterEntries: query is case-insensitive',
  );

  // Empty/whitespace query matches everything (subject to hidden filtering).
  assertNames(
    filterEntries(input, '', true),
    ['readme.md', 'notes-final.txt', 'final-notes.txt', '.hidden', '.git'],
    'filterEntries: empty query matches everything when hidden entries are shown',
  );
  assertNames(
    filterEntries(input, '   ', true),
    ['readme.md', 'notes-final.txt', 'final-notes.txt', '.hidden', '.git'],
    'filterEntries: whitespace-only query matches everything',
  );

  // Hidden off: dotfiles/dot-dirs excluded regardless of query.
  assertNames(
    filterEntries(input, '', false),
    ['readme.md', 'notes-final.txt', 'final-notes.txt'],
    'filterEntries: showHidden=false excludes leading-dot entries',
  );

  // Hidden on: dotfiles included when they also match the query.
  assertNames(
    filterEntries(input, 'hidden', true),
    ['.hidden'],
    'filterEntries: showHidden=true includes a matching dotfile',
  );
}

// Empty/null input never throws and yields an empty array.
assertEmpty(filterEntries([], 'x', true), 'filterEntries: empty array in, empty array out');
assertEmpty(filterEntries(null, 'x', true), 'filterEntries: null in, empty array out');
assertEmpty(filterEntries(undefined, '', true), 'filterEntries: undefined in, empty array out');

// --- splitBreadcrumbs -------------------------------------------------------

function checkCrumbs(actual, expected, description) {
  assert.strictEqual(actual.length, expected.length, `${description}: length`);
  actual.forEach((crumb, i) => {
    assert.strictEqual(crumb.label, expected[i].label, `${description}: crumb[${i}].label`);
    assert.strictEqual(crumb.path, expected[i].path, `${description}: crumb[${i}].path`);
  });
}

checkCrumbs(splitBreadcrumbs('/'), [{ label: '/', path: '/' }], 'splitBreadcrumbs("/")');

checkCrumbs(
  splitBreadcrumbs('/etc'),
  [
    { label: '/', path: '/' },
    { label: 'etc', path: '/etc' },
  ],
  'splitBreadcrumbs("/etc")',
);

// Each crumb's path is the cumulative joined prefix, not the raw segment —
// this is what a naive `path.split('/')` (which would yield the segment
// itself, e.g. "conf.d", as the path) would get wrong.
checkCrumbs(
  splitBreadcrumbs('/etc/nginx/conf.d'),
  [
    { label: '/', path: '/' },
    { label: 'etc', path: '/etc' },
    { label: 'nginx', path: '/etc/nginx' },
    { label: 'conf.d', path: '/etc/nginx/conf.d' },
  ],
  'splitBreadcrumbs("/etc/nginx/conf.d")',
);

// A trailing slash must not introduce a trailing empty crumb — raw
// `'/etc/'.split('/')` yields `['', 'etc', '']`, and a naive implementation
// that does not filter empty segments would emit a bogus trailing crumb.
checkCrumbs(
  splitBreadcrumbs('/etc/'),
  [
    { label: '/', path: '/' },
    { label: 'etc', path: '/etc' },
  ],
  'splitBreadcrumbs("/etc/") trailing slash produces no extra crumb',
);

// Empty/null input never throws; falls back to root.
checkCrumbs(splitBreadcrumbs(''), [{ label: '/', path: '/' }], 'splitBreadcrumbs("")');
checkCrumbs(splitBreadcrumbs(null), [{ label: '/', path: '/' }], 'splitBreadcrumbs(null)');
checkCrumbs(splitBreadcrumbs(undefined), [{ label: '/', path: '/' }], 'splitBreadcrumbs(undefined)');

// --- joinPath ----------------------------------------------------------

assert.strictEqual(joinPath('/a/b', 'c.txt'), '/a/b/c.txt', 'joinPath: no trailing slash on dir');
assert.strictEqual(joinPath('/a/b/', 'c.txt'), '/a/b/c.txt', 'joinPath: trailing slash on dir collapses to single slash');
assert.strictEqual(joinPath('/', 'c.txt'), '/c.txt', 'joinPath: root dir does not double the slash');
assert.strictEqual(joinPath(null, 'c.txt'), '/c.txt', 'joinPath: null dir never throws');
assert.strictEqual(joinPath('/a', null), '/a', 'joinPath: null name never throws');
assert.strictEqual(joinPath(null, null), '/', 'joinPath: null dir and name never throws');

// --- parentPath ----------------------------------------------------------

assert.strictEqual(parentPath('/a/b'), '/a', 'parentPath: two segments');
assert.strictEqual(parentPath('/a'), '/', 'parentPath: one segment');
assert.strictEqual(parentPath('/'), '/', 'parentPath: root is its own parent');
assert.strictEqual(parentPath(''), '/', 'parentPath: empty string never throws');
assert.strictEqual(parentPath(null), '/', 'parentPath: null never throws');
assert.strictEqual(parentPath(undefined), '/', 'parentPath: undefined never throws');

// Chain all the way down to root.
{
  let p = '/etc/nginx/conf.d';
  const chain = [p];
  for (let i = 0; i < 5; i++) {
    p = parentPath(p);
    chain.push(p);
    if (p === '/') break;
  }
  assert.deepStrictEqual(
    chain,
    ['/etc/nginx/conf.d', '/etc/nginx', '/etc', '/'],
    'parentPath: repeated application reaches root and stays there',
  );
}

console.log('file dialog model: all assertions passed');
