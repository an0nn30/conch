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
const STARTUP = path.join(APP, 'startup-runtime.js');
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

// applyAppConfig's only DOM touch is `document.getElementById('app').classList`
// (add/remove 'zen-mode'); everything else it reads is an injected `invoke` or
// an optional global left undefined. This stub is deliberately that narrow —
// no jsdom, matching the rest of this suite.
function loadStartup() {
  let zenClass = false;
  const sandbox = { console, Promise, JSON, String, Object, Array, Error };
  sandbox.window = sandbox;
  sandbox.document = {
    getElementById: (id) => (id === 'app' ? {
      classList: {
        add: (c) => { if (c === 'zen-mode') zenClass = true; },
        remove: (c) => { if (c === 'zen-mode') zenClass = false; },
        contains: (c) => c === 'zen-mode' && zenClass,
      },
    } : null),
  };
  vm.createContext(sandbox);
  for (const file of [MODE, STARTUP]) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return { sandbox, hasZenClass: () => zenClass };
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

// Table-driven over every (queued-path kind) x (saved zen) combination.
// Deleting the kind==='project' gate, or the __termlabZenIsSessionDefault
// flag, leaves this red: it pins the effective zen decision AND that adopt
// is invoked for the project row alone (never for a mixed queue, which
// classifies as "files" — see project_adopt_pending's carry-in gate).
check('applyAppConfig computes the effective zen decision and gates adopt to the project row', async () => {
  const kinds = ['none', 'files', 'project', 'mixed-files'];
  for (const kind of kinds) {
    for (const savedZen of [true, false]) {
      const { sandbox, hasZenClass } = loadStartup();
      const invoked = [];
      const invoke = async (cmd) => {
        invoked.push(cmd);
        switch (cmd) {
          case 'get_app_config':
            return { editor_vim_mode: false, appearance_mode: 'dark', new_window_zen_mode: false };
          case 'current_window_label':
            // Fixed at 'main' so the secondary-window zen default never
            // participates — this table isolates the queued-path decision.
            return 'main';
          case 'pending_open_paths_kind':
            if (kind === 'project') return 'project';
            if (kind === 'files' || kind === 'mixed-files') return 'files';
            return 'none';
          case 'project_adopt_pending':
            return { adopted: { root: '/repo', name: 'repo' }, focusedExisting: false };
          case 'get_saved_layout':
            return {
              zen_mode: savedZen,
              files_panel_visible: true,
              ssh_panel_visible: true,
              bottom_panel_visible: true,
            };
          default:
            throw new Error('unexpected command ' + cmd);
        }
      };

      const runtime = sandbox.termlabStartupRuntime.create();
      await runtime.applyAppConfig(invoke);

      const label = `kind=${kind} savedZen=${savedZen}`;
      const expectZenOn = kind === 'project' ? false : (kind === 'files' || kind === 'mixed-files') ? true : savedZen;
      const expectSessionDefault = kind === 'project' || kind === 'files' || kind === 'mixed-files';
      const expectAdoptCalls = kind === 'project' ? 1 : 0;

      assert.strictEqual(hasZenClass(), expectZenOn, `${label}: zen-mode class on #app`);
      assert.strictEqual(sandbox.window.__termlabEffectiveZen, expectZenOn, `${label}: __termlabEffectiveZen`);
      assert.strictEqual(
        sandbox.window.__termlabZenIsSessionDefault === true,
        expectSessionDefault,
        `${label}: __termlabZenIsSessionDefault`,
      );
      assert.strictEqual(
        invoked.filter((c) => c === 'project_adopt_pending').length,
        expectAdoptCalls,
        `${label}: project_adopt_pending invocation count`,
      );
    }
  }
});

check('main-runtime only toasts the zen default when zen is actually effective', () => {
  const src = fs.readFileSync(path.join(APP, 'main-runtime.js'), 'utf8');
  const toastLine = src.split('\n').find((line) => line.includes("toast.info('Zen mode'"));
  assert.ok(toastLine, 'the zen-mode-default toast is still wired');
  const condition = src.slice(0, src.indexOf(toastLine)).split('\n').slice(-6).join('\n') + toastLine;
  assert.ok(
    condition.includes('__termlabEffectiveZen'),
    'the toast condition must read the effective zen decision, not just the session-default flag — ' +
    'otherwise a project window (session-default true, zen OFF) claims to be in zen when it visibly is not',
  );
});

check('tool-window-runtime hides panels only on the effective zen decision', () => {
  const src = fs.readFileSync(path.join(APP, 'tool-window-runtime.js'), 'utf8');
  assert.ok(
    !/initialLayoutData\s*&&\s*initialLayoutData\.zen_mode === true/.test(src),
    'the panel-hiding block must not key off the raw saved zen_mode value',
  );
  assert.ok(
    src.includes('__termlabEffectiveZen'),
    'the panel-hiding block must read the effective zen decision — otherwise a project window that ' +
    'inherited saved zen_mode=true hides its panels with no zen class to explain why',
  );
});

check('menu-actions seeds zen state from the effective decision', () => {
  const src = fs.readFileSync(path.join(APP, 'menu-actions.js'), 'utf8');
  assert.ok(
    src.includes('zenState') && src.includes('__termlabEffectiveZen'),
    'zenState.active must be seeded from the effective zen decision, not the stale initial-zen-mode flag',
  );
});

check('a failed project adopt (not a benign hand-off) reports the folder by name', async () => {
  const { sandbox } = loadStartup();
  const toasts = [];
  sandbox.window.toast = { error: (title, body) => toasts.push([title, body]), info: () => {} };
  const invoke = async (cmd) => {
    switch (cmd) {
      case 'get_app_config':
        return { editor_vim_mode: false, appearance_mode: 'dark', new_window_zen_mode: false };
      case 'current_window_label':
        return 'main';
      case 'pending_open_paths_kind':
        return 'project';
      case 'project_adopt_pending':
        // Folder vanished / permission denied mid-boot: a real failure, not
        // the benign "another window already has this root" hand-off.
        return { adopted: null, focusedExisting: false };
      case 'get_saved_layout':
        return { zen_mode: false, files_panel_visible: true, ssh_panel_visible: true, bottom_panel_visible: true };
      default:
        throw new Error('unexpected command ' + cmd);
    }
  };
  const runtime = sandbox.termlabStartupRuntime.create();
  await runtime.applyAppConfig(invoke);
  assert.strictEqual(toasts.length, 1, 'a real adopt failure must be reported, not silently swallowed');
  assert.strictEqual(toasts[0][0], 'Cannot Open Folder');
});

check('a benign focused-existing hand-off does not toast', async () => {
  const { sandbox } = loadStartup();
  const toasts = [];
  sandbox.window.toast = { error: (title, body) => toasts.push([title, body]), info: () => {} };
  const invoke = async (cmd) => {
    switch (cmd) {
      case 'get_app_config':
        return { editor_vim_mode: false, appearance_mode: 'dark', new_window_zen_mode: false };
      case 'current_window_label':
        return 'main';
      case 'pending_open_paths_kind':
        return 'project';
      case 'project_adopt_pending':
        // Another window already holds this root; Rust destroys this
        // window. Nothing went wrong, so nothing should be reported.
        return { adopted: null, focusedExisting: true };
      case 'get_saved_layout':
        return { zen_mode: false, files_panel_visible: true, ssh_panel_visible: true, bottom_panel_visible: true };
      default:
        throw new Error('unexpected command ' + cmd);
    }
  };
  const runtime = sandbox.termlabStartupRuntime.create();
  await runtime.applyAppConfig(invoke);
  assert.strictEqual(toasts.length, 0, 'a window on its way to being destroyed needs no explanation');
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
