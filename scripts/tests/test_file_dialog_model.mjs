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

const { sortEntries, filterEntries, splitBreadcrumbs, joinPath, parentPath, formatSize, formatModified } =
  sandbox.termlabFileDialogModel;

function entry(name, isDir, size = 0, modified = null) {
  return { name, is_dir: isDir, size, modified, permissions: null };
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

// --- sortEntries: key/direction (parameterized) -----------------------------
//
// Discriminating fixture: name-asc, size-asc and modified-asc all produce a
// DIFFERENT file order, so a sort that reads the wrong field (or ignores the
// `key` argument entirely) cannot accidentally pass.
//   name asc:     alpha, bravo, charlie, delta
//   size asc:     bravo(100), charlie(200), delta(300), alpha(400)
//   modified asc: bravo(1000), alpha(2000), delta(3000), charlie(4000)
// Two directories carry values that would NOT sort first under size/modified
// on their own merits (one small, one huge) — proving dirs-first is a
// separate partition step, not a side effect of the field being sorted.
{
  const dirSmall = entry('aaa-dir', true, 1, 100);
  const dirBig = entry('zzz-dir', true, 999999, 9999999);
  const fileAlpha = entry('alpha.txt', false, 400, 2000);
  const fileBravo = entry('bravo.txt', false, 100, 1000);
  const fileCharlie = entry('charlie.txt', false, 200, 4000);
  const fileDelta = entry('delta.txt', false, 300, 3000);
  // Deliberately unsorted input order, dirs interleaved with files.
  const fixture = [dirBig, fileAlpha, dirSmall, fileBravo, fileCharlie, fileDelta];

  function names(list) {
    return list.map((e) => e.name);
  }

  // name asc/desc
  assertNames(
    sortEntries(fixture, 'name', 'asc'),
    ['aaa-dir', 'zzz-dir', 'alpha.txt', 'bravo.txt', 'charlie.txt', 'delta.txt'],
    'sortEntries: key=name asc — dirs first (their own name order), then files by name asc',
  );
  assertNames(
    sortEntries(fixture, 'name', 'desc'),
    ['zzz-dir', 'aaa-dir', 'delta.txt', 'charlie.txt', 'bravo.txt', 'alpha.txt'],
    'sortEntries: key=name desc — dirs still first as a group, ordered by the same key/direction within the group',
  );

  // size asc/desc
  assertNames(
    sortEntries(fixture, 'size', 'asc'),
    ['aaa-dir', 'zzz-dir', 'bravo.txt', 'charlie.txt', 'delta.txt', 'alpha.txt'],
    'sortEntries: key=size asc — dirs first regardless of their own size, files by size asc',
  );
  assertNames(
    sortEntries(fixture, 'size', 'desc'),
    ['zzz-dir', 'aaa-dir', 'alpha.txt', 'delta.txt', 'charlie.txt', 'bravo.txt'],
    'sortEntries: key=size desc — dirs still first as a group (not intermixed with files by size), ordered by size within the group',
  );

  // modified asc/desc
  assertNames(
    sortEntries(fixture, 'modified', 'asc'),
    ['aaa-dir', 'zzz-dir', 'bravo.txt', 'alpha.txt', 'delta.txt', 'charlie.txt'],
    'sortEntries: key=modified asc — dirs first, files by modified asc',
  );
  assertNames(
    sortEntries(fixture, 'modified', 'desc'),
    ['zzz-dir', 'aaa-dir', 'charlie.txt', 'delta.txt', 'alpha.txt', 'bravo.txt'],
    'sortEntries: key=modified desc — dirs still first as a group, ordered by modified within the group (not last, even under desc)',
  );

  // Prove the three orderings really are pairwise distinct (guards the
  // fixture itself against regressing into a non-discriminating one).
  const byName = names(sortEntries(fixture, 'name', 'asc')).slice(2);
  const bySize = names(sortEntries(fixture, 'size', 'asc')).slice(2);
  const byModified = names(sortEntries(fixture, 'modified', 'asc')).slice(2);
  assert.notDeepStrictEqual(byName, bySize, 'fixture: name-asc and size-asc file order differ');
  assert.notDeepStrictEqual(byName, byModified, 'fixture: name-asc and modified-asc file order differ');
  assert.notDeepStrictEqual(bySize, byModified, 'fixture: size-asc and modified-asc file order differ');
}

// null `modified` sorts LAST regardless of direction.
{
  const withNulls = [
    entry('has-time.txt', false, 10, 500),
    entry('no-time-a.txt', false, 10, null),
    entry('earlier.txt', false, 10, 100),
    entry('no-time-b.txt', false, 10, null),
  ];
  assertNames(
    sortEntries(withNulls, 'modified', 'asc'),
    ['earlier.txt', 'has-time.txt', 'no-time-a.txt', 'no-time-b.txt'],
    'sortEntries: modified asc — null entries sort after all dated entries',
  );
  assertNames(
    sortEntries(withNulls, 'modified', 'desc'),
    ['has-time.txt', 'earlier.txt', 'no-time-a.txt', 'no-time-b.txt'],
    'sortEntries: modified desc — null entries STILL sort last, not first',
  );
}

// Stable sort: equal-key entries keep their relative input order.
{
  const sameSize = [
    entry('c.txt', false, 50, null),
    entry('a.txt', false, 50, null),
    entry('b.txt', false, 50, null),
  ];
  assertNames(
    sortEntries(sameSize, 'size', 'asc'),
    ['c.txt', 'a.txt', 'b.txt'],
    'sortEntries: equal size values preserve original relative order (stable)',
  );
  const sameModified = [
    entry('z.txt', false, 0, 42),
    entry('y.txt', false, 0, 42),
    entry('x.txt', false, 0, 42),
  ];
  assertNames(
    sortEntries(sameModified, 'modified', 'desc'),
    ['z.txt', 'y.txt', 'x.txt'],
    'sortEntries: equal modified values preserve original relative order (stable), any direction',
  );
}

// Backward compatibility: a no-arg call must equal an explicit
// ('name','asc') call, on a mixed-case, mixed-dir fixture.
{
  function namesOf(list) {
    return list.map((e) => e.name);
  }
  const fixture = [
    entry('apple.txt', false),
    entry('Banana.txt', false),
    entry('zzz-dir', true),
    entry('aaa-file', false),
  ];
  assertNames(
    sortEntries(fixture),
    namesOf(sortEntries(fixture, 'name', 'asc')),
    'sortEntries: no-arg call is identical to explicit (name, asc)',
  );
}

// Unknown key falls back to name (defensive; never throws).
assertNames(
  sortEntries([entry('b.txt', false), entry('a.txt', false)], 'bogus-key', 'asc'),
  ['a.txt', 'b.txt'],
  'sortEntries: unrecognized key falls back to name ordering',
);

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

// --- formatSize ----------------------------------------------------------
//
// `formatSize` NEVER returns '—' — the dir case ('—' for size) is the
// caller's job, not this function's. One decimal below 10 units, none at
// 10 and above. Every boundary below is picked to be unambiguous under
// floating point (no value lands on a rounding knife-edge).
[
  [0, '0 B', 'zero bytes'],
  [1, '1 B', 'one byte'],
  [999, '999 B', 'B/KB boundary: just under 1024 stays in B, no decimal'],
  [1023, '1023 B', 'still bytes at 1023'],
  [1024, '1.0 KB', 'exactly 1024 bytes crosses into KB with one decimal'],
  [10178, '9.9 KB', '~9.94 KB (below the 10-unit threshold) keeps its decimal'],
  [10240, '10 KB', 'exactly 10 KB (10 * 1024) drops the decimal'],
  [1048576, '1.0 MB', 'exactly 1024 KB crosses into MB with one decimal'],
  [1073741824, '1.0 GB', 'exactly 1024 MB crosses into GB with one decimal'],
  [5368709120, '5.0 GB', 'mid-range GB value keeps its decimal (< 10 units)'],
  [10737418240, '10 GB', '10 GB (>= 10 units) drops the decimal'],
].forEach(([bytes, expected, description]) => {
  assert.strictEqual(formatSize(bytes), expected, `formatSize(${bytes}): ${description}`);
});

// Defensive: never throws, never returns '—' (that is the caller's job).
assert.strictEqual(formatSize(null), '0 B', 'formatSize: null never throws, never returns —');
assert.strictEqual(formatSize(undefined), '0 B', 'formatSize: undefined never throws, never returns —');
assert.strictEqual(formatSize(NaN), '0 B', 'formatSize: NaN never throws, never returns —');

// --- formatModified --------------------------------------------------------

// Local-epoch-seconds helper matching the module's own local-time semantics
// (it constructs `new Date(epochSeconds * 1000)` and reads local getters),
// so tests pin calendar-day boundaries without depending on the host's UTC
// offset.
function localEpoch(year, month1to12, day, hour = 12, minute = 0, second = 0) {
  return Math.floor(new Date(year, month1to12 - 1, day, hour, minute, second).getTime() / 1000);
}

// null/undefined never throw and always render as the em dash.
assert.strictEqual(formatModified(null, localEpoch(2026, 1, 15)), '—', 'formatModified: null modified is —');
assert.strictEqual(formatModified(undefined, localEpoch(2026, 1, 15)), '—', 'formatModified: undefined modified is —');

// Today: same calendar day as `now`, regardless of time-of-day distance.
assert.strictEqual(
  formatModified(localEpoch(2026, 1, 15, 8, 0, 0), localEpoch(2026, 1, 15, 20, 0, 0)),
  'Today',
  'formatModified: earlier today is Today',
);

// Pinned midnight edge (the discriminating case an elapsed-time-based
// implementation gets wrong): `now` is 00:01 on day D; a modified time of
// 23:59 on day D-1 is only two minutes earlier in real time — well under
// 24 hours — but it is a DIFFERENT calendar day, so it must read
// "Yesterday", not "Today".
assert.strictEqual(
  formatModified(localEpoch(2026, 1, 14, 23, 59, 0), localEpoch(2026, 1, 15, 0, 1, 0)),
  'Yesterday',
  'formatModified: 23:59 the day before "now" at 00:01 is Yesterday, not Today (calendar day, not elapsed hours)',
);

// The mirror edge: 00:01 on day D, with `now` also 00:01 on day D, must
// still read "Today" even though it is nearly a full day before `now`'s
// end-of-day — i.e. the SAME two clock times one calendar day apart flip
// from Today to Yesterday purely on the date, not a 24-hour window.
assert.strictEqual(
  formatModified(localEpoch(2026, 1, 15, 0, 1, 0), localEpoch(2026, 1, 15, 0, 1, 0)),
  'Today',
  'formatModified: identical timestamp to now is Today',
);

// Two calendar days back (not Yesterday), same year => "Mon D".
assert.strictEqual(
  formatModified(localEpoch(2026, 1, 13, 23, 59, 0), localEpoch(2026, 1, 15, 0, 1, 0)),
  'Jan 13',
  'formatModified: two calendar days back is not Yesterday; renders as "Mon D"',
);

// Same year, several months back => "Mon D".
assert.strictEqual(
  formatModified(localEpoch(2026, 3, 1, 9, 0, 0), localEpoch(2026, 6, 15, 9, 0, 0)),
  'Mar 1',
  'formatModified: same year, month/day format',
);

// Different (earlier) year => full "YYYY-MM-DD", even if the month/day is
// close to now's month/day.
assert.strictEqual(
  formatModified(localEpoch(2025, 6, 15, 9, 0, 0), localEpoch(2026, 6, 15, 9, 0, 0)),
  '2025-06-15',
  'formatModified: previous year renders as YYYY-MM-DD',
);

// Different (future) year is treated the same as any other non-current year.
assert.strictEqual(
  formatModified(localEpoch(2027, 1, 5, 9, 0, 0), localEpoch(2026, 6, 15, 9, 0, 0)),
  '2027-01-05',
  'formatModified: a future year also renders as YYYY-MM-DD (zero-padded)',
);

console.log('file dialog model: all assertions passed');
