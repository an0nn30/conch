// Run: node scripts/tests/test_project_mode.mjs
//
// Project mode's boot half: the mode resolver every other project surface
// reads, the routing seam that used to answer a directory with a
// "coming soon" toast, and the source-level ordering that keeps a project
// window out of zen and gives it a terminal tab at the project root.
//
// No jsdom (see test_tl_dialog.mjs for the precedent). Deliberately does NOT
// define `sandbox.global` — see test_problems_panel.mjs's note.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const MODE = path.join(APP, 'features/project/project-mode.js');
const ROUTING = path.join(APP, 'features/editor/open-path-routing.js');
const INDEX_HTML = path.join(ROOT, 'index.html');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

const plain = (value) => JSON.parse(JSON.stringify(value === undefined ? null : value));
function deepEq(actual, expected, message) {
  assert.deepStrictEqual(plain(actual), plain(expected), message);
}

function load(files) {
  const sandbox = { console, Promise, JSON, String, Object, Array, Error };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return sandbox;
}

check('adopt binds the window and publishes the globals', async () => {
  const sandbox = load([MODE]);
  const mode = sandbox.termlabProjectMode;
  assert.strictEqual(mode.isActive(), false, 'a fresh window has no project');
  const invoked = [];
  const invoke = async (cmd) => {
    invoked.push(cmd);
    return { adopted: { root: '/repo', name: 'repo' }, focusedExisting: false };
  };
  const info = await mode.adopt(invoke);
  deepEq(info, { root: '/repo', name: 'repo' });
  deepEq(invoked, ['project_adopt_pending']);
  assert.strictEqual(mode.isActive(), true);
  assert.strictEqual(mode.root(), '/repo');
  assert.strictEqual(mode.name(), 'repo');
  deepEq(sandbox.__termlabProject, { root: '/repo', name: 'repo' });
  assert.strictEqual(sandbox.__termlabProjectName, 'repo');
});

check('adopt with nothing to adopt leaves the window project-less', async () => {
  const sandbox = load([MODE]);
  const mode = sandbox.termlabProjectMode;
  const info = await mode.adopt(async () => ({ adopted: null, focusedExisting: false }));
  assert.strictEqual(info, null);
  assert.strictEqual(mode.isActive(), false);
  assert.strictEqual(sandbox.__termlabProjectName, undefined);
});

check('a failing adopt is not fatal', async () => {
  const sandbox = load([MODE]);
  const info = await sandbox.termlabProjectMode.adopt(async () => { throw new Error('no command'); });
  assert.strictEqual(info, null, 'a missing backend must not break boot');
});

check('isUnderRoot answers on path boundaries, not string prefixes', () => {
  const sandbox = load([MODE]);
  const mode = sandbox.termlabProjectMode;
  mode.set({ root: '/repo', name: 'repo' });
  assert.strictEqual(mode.isUnderRoot('/repo/src/main.rs'), true);
  assert.strictEqual(mode.isUnderRoot('/repo'), true, 'the root itself is under the root');
  assert.strictEqual(mode.isUnderRoot('/repository/src/main.rs'), false, 'a sibling that shares a prefix is not inside');
  assert.strictEqual(mode.isUnderRoot('/elsewhere/lib.rs'), false);
  mode.reset();
  assert.strictEqual(mode.isUnderRoot('/repo/src/main.rs'), false, 'no project means nothing is under it');
});

check('a directory routes to project_open instead of a toast', async () => {
  const sandbox = load([ROUTING]);
  const calls = { opened: [], projects: [], infos: [], errors: [] };
  const routing = sandbox.termlabOpenPathRouting.create({
    invoke: async (cmd, args) => {
      if (cmd === 'local_stat') {
        return args.path === '/tmp/dir'
          ? { name: 'dir', is_dir: true, size: 0, modified: null, permissions: null }
          : { name: 'a.txt', is_dir: false, size: 1, modified: null, permissions: null };
      }
      throw new Error('unexpected command ' + cmd);
    },
    openLocalFile: (p) => { calls.opened.push(p); },
    openProject: async (p) => { calls.projects.push(p); },
    toastError: (title, body) => { calls.errors.push(body); },
    toastInfo: (title, body) => { calls.infos.push(body); },
  });
  const opened = await routing.routePaths(['/tmp/a.txt', '/tmp/dir']);
  assert.strictEqual(opened, 1, 'only the file counts as an opened editor');
  deepEq(calls.opened, ['/tmp/a.txt']);
  deepEq(calls.projects, ['/tmp/dir']);
  assert.strictEqual(calls.infos.length, 0, 'a directory is opened, not explained away');
  assert.strictEqual(
    sandbox.termlabOpenPathRouting.DIRECTORY_COMING_SOON,
    undefined,
    'the coming-soon seam is gone, not merely unused',
  );
});

check('a failing project_open reports the path rather than dropping it', async () => {
  const sandbox = load([ROUTING]);
  const errors = [];
  const routing = sandbox.termlabOpenPathRouting.create({
    invoke: async () => ({ name: 'dir', is_dir: true, size: 0, modified: null, permissions: null }),
    openLocalFile: () => {},
    openProject: async () => { throw new Error('not a folder'); },
    toastError: (title, body) => { errors.push(body); },
    toastInfo: () => {},
  });
  const opened = await routing.routePaths(['/tmp/dir']);
  assert.strictEqual(opened, 0);
  assert.strictEqual(errors.length, 1);
  assert.ok(errors[0].includes('/tmp/dir'), 'the error names the path');
});

check('startup-runtime keeps a project window out of zen', () => {
  const src = fs.readFileSync(path.join(APP, 'startup-runtime.js'), 'utf8');
  assert.ok(src.includes("invoke('pending_open_paths_kind')"), 'the boot layout asks what kind of paths are queued');
  assert.ok(!src.includes("has_pending_open_paths"), 'the boolean peek is gone');
  assert.ok(src.includes('termlabProjectMode'), 'the adoption happens before the layout read');
  const adopt = src.indexOf('termlabProjectMode');
  const layout = src.indexOf("invoke('get_saved_layout')");
  assert.ok(adopt < layout, 'adopt must precede the layout read so the per-project layout applies');
});

check('main-runtime gives a project window a terminal tab at the project root', () => {
  const src = fs.readFileSync(path.join(APP, 'main-runtime.js'), 'utf8');
  const pull = src.indexOf('take_pending_open_paths');
  const projectTab = src.indexOf('createTab({ cwd: projectRoot })');
  const firstTab = src.indexOf('createTab().catch');
  assert.ok(projectTab !== -1, 'a project window opens its first terminal at the root');
  assert.ok(firstTab !== -1, 'the plain terminal tab still exists for ordinary windows');
  assert.ok(pull < projectTab && pull < firstTab, 'the queue pull still precedes any tab creation');
});

check('the window title carries the project name across tab switches', () => {
  const src = fs.readFileSync(path.join(APP, 'tab-manager.js'), 'utf8');
  assert.ok(src.includes('__termlabProjectName'), 'updateWindowTitle reads the project name');
  const read = src.indexOf('__termlabProjectName');
  const set = src.indexOf('setWindowTitle(title)');
  assert.ok(read < set, 'the prefix is applied before the title is pushed to the OS');
});

check('index.html loads project-mode.js before the modules that read it', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(name);
  assert.ok(at('app/features/project/project-mode.js') > 0, 'project-mode.js is not loaded');
  assert.ok(at('app/features/project/project-mode.js') < at('app/features/editor/open-path-routing.js'));
  assert.ok(at('app/features/project/project-mode.js') < at('app/startup-runtime.js'));
});

check('the project modules use no regex lookbehind', () => {
  for (const file of [MODE, ROUTING]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(
      !/\(\?<[=!]/.test(source),
      `${file} uses a lookbehind — it costs the whole file on an older WKWebView`,
    );
  }
});

check('the project modules contain no control bytes', () => {
  for (const file of [MODE, ROUTING]) {
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      const byte = bytes[i];
      assert.ok(
        byte >= 0x20 || byte === 0x0a || byte === 0x09,
        `${file}: control byte 0x${byte.toString(16)} at offset ${i} — git treats the file as binary`,
      );
    }
  }
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
  console.log(`project mode: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`project mode: all ${ran} checks passed`);
}
