// Run: node scripts/tests/test_pane_title.mjs
//
// Tab/window title composition. Pure string work over a snapshot of the pane's
// state, so it is tested directly against a trivial `window` stub rather than
// through the terminal (no jsdom here — see test_tl_dialog.mjs for precedent).
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/core/pane-title.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { composeTitle, collapseHome } = sandbox.termlabPaneTitle;

const HOME = '/Users/dustin';
const base = { user: 'dustin', host: 'mbp', home: HOME, cols: 120, rows: 30 };

// --- collapseHome ------------------------------------------------------------
assert.strictEqual(collapseHome('/Users/dustin/projects/conch', HOME), '~/projects/conch');
assert.strictEqual(collapseHome('/Users/dustin', HOME), '~');
assert.strictEqual(collapseHome('/etc/hosts', HOME), '/etc/hosts');
assert.strictEqual(collapseHome('/Users/dustinson/x', HOME), '/Users/dustinson/x',
  'a path that merely starts with the home string is not under home');
assert.strictEqual(collapseHome('/var/log', ''), '/var/log', 'no home known: path unchanged');
assert.strictEqual(collapseHome('', HOME), '', 'empty path stays empty');

// --- the shape the user asked for -------------------------------------------
assert.strictEqual(
  composeTitle({ ...base, cwd: '/Users/dustin/projects/conch' }),
  'dustin@mbp: ~/projects/conch — 120×30',
);

// A foreground program replaces the path.
assert.strictEqual(
  composeTitle({ ...base, cwd: '/Users/dustin/projects/conch', program: 'vim' }),
  'dustin@mbp: vim — 120×30',
);

// --- OSC titles --------------------------------------------------------------
// An explicit title from the shell or a program wins over the derived body.
assert.strictEqual(
  composeTitle({ ...base, cwd: '/Users/dustin', program: 'vim', oscTitle: 'make: *** [build]' }),
  'dustin@mbp: make: *** [build] — 120×30',
);

// Many shells already emit "user@host: dir" themselves. Prefixing that again
// would read "dustin@mbp: dustin@mbp: ~ — 120×30", so the prefix is dropped
// when the OSC title already carries it.
assert.strictEqual(
  composeTitle({ ...base, oscTitle: 'dustin@mbp: ~/src' }),
  'dustin@mbp: ~/src — 120×30',
);
assert.strictEqual(
  composeTitle({ ...base, oscTitle: 'DUSTIN@MBP:~' }),
  'DUSTIN@MBP:~ — 120×30',
  'the duplicate-prefix check is case-insensitive',
);

// --- degrading when pieces are missing ---------------------------------------
assert.strictEqual(
  composeTitle({ ...base, user: '', host: '', cwd: '/Users/dustin/projects/conch' }),
  '~/projects/conch — 120×30',
  'no user/host: no prefix, no stray colon',
);
assert.strictEqual(
  composeTitle({ ...base, host: '', cwd: '/tmp' }),
  '/tmp — 120×30',
  'a user without a host is not enough for a prefix',
);
assert.strictEqual(
  composeTitle({ ...base, cwd: '', program: '' }),
  'dustin@mbp — 120×30',
  'nothing to say about the body: prefix loses its colon',
);
assert.strictEqual(
  composeTitle({ ...base, cwd: '/tmp', cols: 0, rows: 0 }),
  'dustin@mbp: /tmp',
  'size is omitted until the terminal has been measured',
);
assert.strictEqual(
  composeTitle({ user: '', host: '', home: '', cwd: '', cols: 0, rows: 0 }),
  'Terminal',
  'a pane we know nothing about still has a usable label',
);
assert.strictEqual(composeTitle({}), 'Terminal', 'empty input does not throw');
assert.strictEqual(composeTitle(null), 'Terminal', 'null input does not throw');

// --- SSH panes ---------------------------------------------------------------
// An SSH pane knows its remote user/host but has no local cwd to report.
assert.strictEqual(
  composeTitle({ user: 'ubuntu', host: '10.0.0.7', home: '', cwd: '', cols: 80, rows: 24 }),
  'ubuntu@10.0.0.7 — 80×24',
);

console.log('pane title composition: all assertions passed');
