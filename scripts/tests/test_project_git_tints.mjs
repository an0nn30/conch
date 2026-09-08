// Run: node scripts/tests/test_project_git_tints.mjs
//
// Git tints in the project tree: the frontend consumer of the Task 10 Rust
// snapshot. Pure path-boundary logic (relativeTo, stateForPath's directory
// rollup) plus the refresh-trigger wiring (startPolling) and the CSS/HTML
// wiring that turns a snapshot into what the tree actually paints.
//
// No jsdom (see test_tl_dialog.mjs for the precedent). Deliberately does NOT
// define `sandbox.global` — see test_problems_panel.mjs's note.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const GIT = path.join(APP, 'features/project/git-tints.js');
const TREE_CSS = path.join(ROOT, 'styles/design-system/components/project-tree.css');
const BASE_CSS = path.join(ROOT, 'styles/design-system/base.css');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

const plain = (value) => JSON.parse(JSON.stringify(value === undefined ? null : value));
function deepEq(actual, expected, message) {
  assert.deepStrictEqual(plain(actual), plain(expected), message);
}

function loadGit() {
  const sandbox = {
    console, JSON, Object, Array, String, Number, Math, Map, Set, Promise,
    setTimeout, clearTimeout, setInterval, clearInterval,
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(GIT, 'utf8'), sandbox, { filename: GIT });
  return sandbox.termlabProjectGit;
}

const snapshot = (files) => ({ available: true, files: files || {} });

check('relativeTo answers on path boundaries', () => {
  const git = loadGit();
  assert.strictEqual(git.relativeTo('/repo', '/repo/src/main.rs'), 'src/main.rs');
  assert.strictEqual(git.relativeTo('/repo', '/repo'), '');
  assert.strictEqual(git.relativeTo('/repo', '/repository/a.rs'), null,
    'a sibling sharing a prefix is not inside the repo');
  assert.strictEqual(git.relativeTo('/repo', '/elsewhere/a.rs'), null);
});

check('a file carries its own state and nothing else', () => {
  const git = loadGit();
  const snap = snapshot({ 'src/main.rs': 'modified', 'new.rs': 'added' });
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/src/main.rs', false), 'modified');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/new.rs', false), 'added');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/clean.rs', false), null);
});

check('a directory rolls up to modified when anything beneath it has a state', () => {
  const git = loadGit();
  const snap = snapshot({ 'src/deep/a.rs': 'added', 'other.rs': 'untracked' });
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/src', true), 'modified');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/src/deep', true), 'modified');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/tests', true), null,
    'a clean directory carries no tint');
});

check('rollup matches on path segments, not string prefixes', () => {
  const git = loadGit();
  const snap = snapshot({ 'srcfoo/a.rs': 'modified' });
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/src', true), null,
    '"srcfoo" is not inside "src"');
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo/srcfoo', true), 'modified');
});

check('an unavailable snapshot tints nothing', () => {
  const git = loadGit();
  const off = { available: false, files: {} };
  assert.strictEqual(git.stateForPath(off, '/repo', '/repo/src/main.rs', false), null);
  assert.strictEqual(git.stateForPath(null, '/repo', '/repo/src/main.rs', false), null);
});

check('the project root itself rolls up when anything inside has a state', () => {
  const git = loadGit();
  const snap = snapshot({ 'src/main.rs': 'modified' });
  assert.strictEqual(git.stateForPath(snap, '/repo', '/repo', true), 'modified',
    'the tree root is a directory too, and rolls up like any other');
});

check('polling refreshes on focus, on save and on the timer, and stops cleanly', async () => {
  const git = loadGit();
  const applied = [];
  const listeners = new Map();
  const fakeWindow = {
    addEventListener: (type, fn) => { listeners.set(type, fn); },
    removeEventListener: (type) => { listeners.delete(type); },
  };
  let calls = 0;
  const stop = git.startPolling({
    invoke: async () => { calls += 1; return snapshot({ 'a.rs': 'modified' }); },
    getTree: () => ({ setGitStatus: (s) => applied.push(s) }),
    intervalMs: 10,
    target: fakeWindow,
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.strictEqual(calls, 1, 'an immediate first refresh');
  listeners.get('focus')();
  listeners.get('termlab:editor-saved')();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.strictEqual(calls, 3, 'focus and save each refresh');
  await new Promise((resolve) => setTimeout(resolve, 25));
  assert.ok(calls > 3, 'the timer keeps it fresh while the panel is visible');
  const before = calls;
  stop();
  await new Promise((resolve) => setTimeout(resolve, 25));
  assert.strictEqual(calls, before, 'stopping really stops');
  assert.ok(applied.length > 0, 'each snapshot reaches the tree');
});

check('the timer is gated on visibility, but focus and save never are', async () => {
  const git = loadGit();
  const listeners = new Map();
  const fakeWindow = {
    addEventListener: (type, fn) => { listeners.set(type, fn); },
    removeEventListener: (type) => { listeners.delete(type); },
  };
  let calls = 0;
  const stop = git.startPolling({
    invoke: async () => { calls += 1; return snapshot({}); },
    getTree: () => ({ setGitStatus: () => {} }),
    intervalMs: 10,
    isVisible: () => false,
    target: fakeWindow,
  });
  await new Promise((resolve) => setTimeout(resolve, 35));
  assert.strictEqual(calls, 1, 'only the immediate first refresh — the timer is gated');
  listeners.get('focus')();
  listeners.get('termlab:editor-saved')();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.strictEqual(calls, 3, 'a user act always refreshes, hidden or not');
  stop();
});

check('the tints are tokens, defined for both themes, and shape-paired', () => {
  const base = fs.readFileSync(BASE_CSS, 'utf8');
  for (const token of [
    '--tl-git-modified', '--tl-git-added', '--tl-git-deleted',
    '--tl-git-conflicted', '--tl-git-untracked',
  ]) {
    assert.ok(base.includes(token + ':'), `${token} has no semantic alias`);
  }
  const tree = fs.readFileSync(TREE_CSS, 'utf8');
  for (const state of ['modified', 'added', 'deleted', 'renamed', 'conflicted', 'untracked']) {
    assert.ok(
      tree.includes(`[data-git-state="${state}"]`),
      `${state} has no rule — its tint would silently be the default colour`,
    );
  }
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(tree), 'design-system components use tokens only');
});

check('git-tints uses no regex lookbehind and no control bytes', () => {
  const source = fs.readFileSync(GIT, 'utf8');
  assert.ok(!/\(\?<[=!]/.test(source), `${GIT} uses a lookbehind`);
  const bytes = fs.readFileSync(GIT);
  for (let i = 0; i < bytes.length; i += 1) {
    assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
      `${GIT}: control byte at offset ${i}`);
  }
});

check('index.html loads git-tints before the files panel that starts it', () => {
  const html = fs.readFileSync(path.join(ROOT, 'index.html'), 'utf8');
  assert.ok(html.indexOf('app/features/project/git-tints.js') > 0);
  assert.ok(html.indexOf('app/features/project/git-tints.js') < html.indexOf('app/panels/project-tree.js'));
});

for (const { name, fn } of queued) {
  ran += 1;
  try {
    await fn();
    console.log(`  ok   ${name}`);
  } catch (error) {
    failures += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${(error && error.stack) || error}`);
  }
}
if (failures) {
  console.log(`project git tints: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`project git tints: all ${ran} checks passed`);
}
