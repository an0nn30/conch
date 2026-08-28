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
const FILES_PANEL = path.join(APP, 'panels/files-panel.js');
const FILES_DATA_SERVICE_PATH = path.join(APP, 'features/files/data-service.js');
const FILES_PANE_STORE_PATH = path.join(APP, 'features/files/pane-store.js');
const FILES_ACTIONS_PATH = path.join(APP, 'features/files/actions.js');
const BANNER = path.join(APP, 'features/project/trust-banner.js');
const LSP_ROOT = path.join(APP, 'features/project/lsp-root.js');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

const plain = (value) => JSON.parse(JSON.stringify(value === undefined ? null : value));
function deepEq(actual, expected, message) {
  assert.deepStrictEqual(plain(actual), plain(expected), message);
}

// --- DOM stand-in for the trust-banner/lsp-root checks below -----------------
// Copied from test_problems_panel.mjs (makeElement, selectorParts, dataName)
// and given distinct names — this file already has its own, narrower
// `makeElement` below for the files-panel behavioral harness, which returns
// `null` from querySelector by design (each of those fixtures overrides
// querySelector itself); this one is a full stand-in so
// `body.querySelector('[data-project-trust="trust"]')` actually works.

function domDataName(attr) {
  return attr.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
}

function domSelectorParts(selector) {
  const match = /^([a-zA-Z]*)(?:#([\w-]+))?(?:\.([\w-]+))?(?:\[([\w-]+)(?:="([^"]*)")?\])?$/.exec(selector);
  if (!match) throw new Error(`unsupported selector in test DOM: ${selector}`);
  return { tag: match[1], id: match[2], className: match[3], attr: match[4], attrValue: match[5] };
}

function domMakeElement(tag) {
  const listeners = new Map();
  const attributes = new Map();
  let ownText = '';
  let className = '';
  const element = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    parentNode: null,
    dataset: {},
    style: {},
    id: '',
    hidden: false,
    disabled: false,
    type: '',
    value: '',
    title: '',
    get className() { return className; },
    set className(value) { className = String(value || ''); },
    classList: {
      add(...names) {
        const values = new Set(className.split(/\s+/).filter(Boolean));
        names.forEach((name) => values.add(name));
        className = Array.from(values).join(' ');
      },
      remove(...names) {
        const removed = new Set(names);
        className = className.split(/\s+/).filter((name) => name && !removed.has(name)).join(' ');
      },
      contains(name) { return className.split(/\s+/).includes(name); },
    },
    get textContent() {
      return ownText + this.children.map((child) => child.textContent || '').join('');
    },
    set textContent(value) {
      for (const child of this.children) child.parentNode = null;
      this.children = [];
      ownText = String(value == null ? '' : value);
    },
    appendChild(child) {
      if (child.parentNode) child.parentNode.removeChild(child);
      this.children.push(child);
      child.parentNode = this;
      return child;
    },
    removeChild(child) {
      const index = this.children.indexOf(child);
      if (index >= 0) this.children.splice(index, 1);
      child.parentNode = null;
      return child;
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    focus() {},
    setAttribute(name, value) {
      const stringValue = String(value);
      attributes.set(name, stringValue);
      if (name === 'id') this.id = stringValue;
      else if (name === 'class') this.className = stringValue;
      else if (name.startsWith('data-')) this.dataset[domDataName(name)] = stringValue;
    },
    removeAttribute(name) { attributes.delete(name); },
    getAttribute(name) {
      if (name === 'id' && this.id) return this.id;
      if (name === 'class' && this.className) return this.className;
      if (name.startsWith('data-')) {
        const value = this.dataset[domDataName(name)];
        return value === undefined ? null : String(value);
      }
      return attributes.has(name) ? attributes.get(name) : null;
    },
    matches(selector) {
      const parts = domSelectorParts(selector);
      if (parts.tag && this.tagName !== parts.tag.toUpperCase()) return false;
      if (parts.id && this.id !== parts.id) return false;
      if (parts.className && !this.classList.contains(parts.className)) return false;
      if (parts.attr) {
        const value = this.getAttribute(parts.attr);
        if (value === null) return false;
        if (parts.attrValue !== undefined && value !== parts.attrValue) return false;
      }
      return true;
    },
    closest(selector) {
      let current = this;
      while (current) {
        if (current.matches(selector)) return current;
        current = current.parentNode;
      }
      return null;
    },
    querySelectorAll(selector) {
      const found = [];
      const visit = (node) => {
        for (const child of node.children) {
          if (child.matches(selector)) found.push(child);
          visit(child);
        }
      };
      visit(this);
      return found;
    },
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; },
    addEventListener(name, handler) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(handler);
    },
    removeEventListener(name, handler) {
      const values = listeners.get(name) || [];
      const index = values.indexOf(handler);
      if (index >= 0) values.splice(index, 1);
    },
    // Not bubbling: every check in this file dispatches directly on the exact
    // element it wants to hear from (a specific button), so a plain
    // this-element-only dispatch is all the trust-banner checks need.
    dispatchEvent(event) {
      const type = event && event.type;
      const record = { target: this, ...event };
      for (const handler of (listeners.get(type) || []).slice()) handler(record);
      return true;
    },
  };
  return element;
}

function load(files) {
  const body = domMakeElement('body');
  const documentStub = {
    body,
    activeElement: body,
    createElement: (tag) => domMakeElement(tag),
    getElementById: (id) => body.querySelector(`#${id}`),
    querySelector: (selector) => body.querySelector(selector),
    querySelectorAll: (selector) => body.querySelectorAll(selector),
    addEventListener() {},
    removeEventListener() {},
  };
  const sandbox = {
    console, Promise, JSON, String, Object, Array, Error,
    document: documentStub, setTimeout, clearTimeout, Set, Map,
  };
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
  // fix round 1, F6: a successful adopt also fires a fire-and-forget
  // rebuild_menu so the newly-recorded recent reaches the native menu.
  deepEq(invoked, ['project_adopt_pending', 'rebuild_menu']);
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
          // Reached whenever adopt did not itself win a project (every kind
          // but 'project' here) — the F1 fallback (task-6 review) asks this
          // unconditionally once adopt leaves the window project-less. Null
          // keeps this table's non-'project' rows genuinely project-less,
          // matching expectSessionDefault/expectZenOn below.
          case 'project_info':
            return null;
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
      // The F1 fallback (task-6 review) fires here too — adopt left the
      // window project-less. Null: this window really has no project bound
      // in the registry either (the vanished/denied folder never got one).
      case 'project_info':
        return null;
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
      // The F1 fallback (task-6 review) fires here too, in the brief window
      // before Rust actually destroys this window — it holds no project of
      // its own either way.
      case 'project_info':
        return null;
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

// task-6 review, F1: every test above this line queues its project through
// PendingOpens (pending_open_paths_kind === 'project', drained by
// project_adopt_pending) — the CLI path. A window opened via project_open
// (Open Folder in the menu/palette, or a directory routed from an
// already-running window) never queues anything: project_open_build binds
// the registry directly, before the window is even shown, so the adopt
// block above is unconditionally a no-op for it (pending_open_paths_kind
// resolves 'none') and — before this fix — termlabProjectMode stayed null
// forever for that entire class of window. project_info resolves
// independently, by the calling window's own registry entry.
check('a window whose project was bound via project_open (not the CLI queue) is picked up through project_info', async () => {
  const { sandbox } = loadStartup();
  const invoke = async (cmd) => {
    switch (cmd) {
      case 'get_app_config':
        return { editor_vim_mode: false, appearance_mode: 'dark', new_window_zen_mode: false };
      case 'current_window_label':
        return 'window-3';
      case 'pending_open_paths_kind':
        // Nothing queued for this window — the adopt block stays fully
        // inert. project_adopt_pending must not even be called.
        return 'none';
      case 'project_adopt_pending':
        throw new Error('project_adopt_pending must not run when nothing is queued for this window');
      case 'project_info':
        return { root: '/repo', name: 'repo' };
      case 'get_saved_layout':
        return { zen_mode: false, files_panel_visible: true, ssh_panel_visible: true, bottom_panel_visible: true };
      default:
        throw new Error('unexpected command ' + cmd);
    }
  };
  const runtime = sandbox.termlabStartupRuntime.create();
  await runtime.applyAppConfig(invoke);
  assert.strictEqual(sandbox.termlabProjectMode.isActive(), true,
    'project_info must activate project mode when the adopt path never ran');
  assert.strictEqual(sandbox.termlabProjectMode.root(), '/repo');
  assert.strictEqual(sandbox.termlabProjectMode.name(), 'repo');
  deepEq(sandbox.__termlabProject, { root: '/repo', name: 'repo' });
});

check('project_info resolving null (an ordinary, project-less window) leaves the window project-less', async () => {
  const { sandbox } = loadStartup();
  const invoke = async (cmd) => {
    switch (cmd) {
      case 'get_app_config':
        return { editor_vim_mode: false, appearance_mode: 'dark', new_window_zen_mode: false };
      case 'current_window_label':
        return 'main';
      case 'pending_open_paths_kind':
        return 'none';
      case 'project_info':
        return null;
      case 'get_saved_layout':
        return { zen_mode: false, files_panel_visible: true, ssh_panel_visible: true, bottom_panel_visible: true };
      default:
        throw new Error('unexpected command ' + cmd);
    }
  };
  const runtime = sandbox.termlabStartupRuntime.create();
  await runtime.applyAppConfig(invoke);
  assert.strictEqual(sandbox.termlabProjectMode.isActive(), false,
    'the overwhelmingly common case (an ordinary window) must not be turned into a project window');
});

check('a successful CLI-queue adopt is never redundantly re-queried through project_info', async () => {
  const { sandbox } = loadStartup();
  const calls = [];
  const invoke = async (cmd) => {
    calls.push(cmd);
    switch (cmd) {
      case 'get_app_config':
        return { editor_vim_mode: false, appearance_mode: 'dark', new_window_zen_mode: false };
      case 'current_window_label':
        return 'main';
      case 'pending_open_paths_kind':
        return 'project';
      case 'project_adopt_pending':
        return { adopted: { root: '/queued', name: 'queued' }, focusedExisting: false };
      case 'project_info':
        return { root: '/wrong', name: 'wrong' };
      case 'get_saved_layout':
        return { zen_mode: false, files_panel_visible: true, ssh_panel_visible: true, bottom_panel_visible: true };
      default:
        throw new Error('unexpected command ' + cmd);
    }
  };
  const runtime = sandbox.termlabStartupRuntime.create();
  await runtime.applyAppConfig(invoke);
  assert.strictEqual(sandbox.termlabProjectMode.isActive(), true);
  assert.strictEqual(sandbox.termlabProjectMode.root(), '/queued',
    'the CLI-queue adopt already won this window a project — project_info must not overwrite it');
  assert.ok(!calls.includes('project_info'), 'the fallback is gated on !isActive(), so it must not even be called here');
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

// ===========================================================================
// Task 7: the trust banner and the LSP-root pass-through.
// ===========================================================================

check('the banner asks once for an untrusted root and never for a trusted one', () => {
  const sandbox = load([BANNER]);
  const decide = sandbox.termlabProjectTrustBanner.decide;
  assert.strictEqual(decide([], '/repo'), 'ask', 'no record means ask');
  assert.strictEqual(
    decide([{ root: '/repo', adapterId: null, decision: 'trusted' }], '/repo'),
    'settled',
    'an existing trust decision means the banner never returns',
  );
  assert.strictEqual(
    decide([{ root: '/repo', adapterId: null, decision: 'denied' }], '/repo'),
    'settled',
    'a recorded denial is also a decision — do not nag',
  );
  assert.strictEqual(
    decide([{ root: '/other', adapterId: null, decision: 'trusted' }], '/repo'),
    'ask',
    'a different project says nothing about this one',
  );
});

// Review F4/F7 (fix round 1): the banner grants strictly broader consent
// than the per-file dialog's per-adapter trust — so only a PROJECT-WIDE
// record (adapterId null/undefined) may settle it. A per-adapter-only
// record answers a narrower question and must not suppress the broader
// offer. A project-wide Revoked record is explicitly pinned as settled too
// (F7): this banner never re-asks a project the user already gave a
// project-wide answer to, whatever that answer was.
check('decide() only settles on a PROJECT-WIDE record — a per-adapter trust record still offers the broader grant', () => {
  const sandbox = load([BANNER]);
  const decide = sandbox.termlabProjectTrustBanner.decide;
  assert.strictEqual(
    decide([{ root: '/repo', adapterId: 'rust-analyzer', decision: 'trusted' }], '/repo'),
    'ask',
    'a per-adapter record answers one language server, not "every language server" — the banner still offers the broader grant',
  );
  assert.strictEqual(
    decide([
      { root: '/repo', adapterId: 'rust-analyzer', decision: 'trusted' },
      { root: '/repo', adapterId: 'typescript', decision: 'trusted' },
    ], '/repo'),
    'ask',
    'even every adapter trusted individually is not the same record as a project-wide decision',
  );
  assert.strictEqual(
    decide([{ root: '/repo', adapterId: null, decision: 'revoked' }], '/repo'),
    'settled',
    'F7: a project-wide Revoked record is settled too — revoked projects do not re-ask',
  );
  assert.strictEqual(
    decide([{ root: '/repo', decision: 'trusted' }], '/repo'),
    'settled',
    'a record with no adapterId field at all (undefined) is project-wide, same as adapterId: null',
  );
});

check('Trust project calls lsp_set_project_trust with the root and a trusted decision', async () => {
  const sandbox = load([BANNER]);
  const { body } = (() => {
    const host = sandbox.document.createElement('div');
    sandbox.document.body.appendChild(host);
    return { body: host };
  })();
  const calls = [];
  const decisions = [];
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host: body,
    root: '/repo',
    bridge: {
      trustedProjects: async () => [],
      setProjectTrust: async (root, adapterId, decision) => { calls.push([root, adapterId, decision]); },
    },
    onDecision: (d) => { decisions.push(d); },
  });
  await handle.ready;
  const trust = body.querySelector('[data-project-trust="trust"]');
  assert.ok(trust, 'the banner offers Trust project');
  assert.ok(body.querySelector('[data-project-trust="later"]'), 'and Not now');
  trust.dispatchEvent({ type: 'click', target: trust });
  await handle.settled();
  deepEq(calls, [['/repo', null, 'trusted']]);
  deepEq(decisions, ['trusted']);
  assert.strictEqual(body.querySelector('[data-project-trust="trust"]'), null, 'the banner leaves once decided');
});

check('Trust project is idempotent under a double click', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const calls = [];
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    bridge: {
      trustedProjects: async () => [],
      setProjectTrust: async (root, adapterId, decision) => { calls.push([root, adapterId, decision]); },
    },
    onDecision: () => {},
  });
  await handle.ready;
  const trust = host.querySelector('[data-project-trust="trust"]');
  trust.dispatchEvent({ type: 'click', target: trust });
  trust.dispatchEvent({ type: 'click', target: trust });
  await handle.settled();
  deepEq(calls, [['/repo', null, 'trusted']], 'a second click before the IPC round trip resolves must not send a second decision');
});

// Review F3 (ruling): the banner grants strictly broader consent than the
// per-file dialog while disclosing less — the copy has to name the project
// (name + path) and state the grant applies to every language server, not a
// generic "start language servers?" line.
check('F3: the banner copy names the project (name + path) and discloses that the grant covers every language server', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    name: 'myrepo',
    bridge: { trustedProjects: async () => [], setProjectTrust: async () => {} },
    onDecision: () => {},
  });
  await handle.ready;
  const banner = host.querySelector('.tl-project-banner');
  assert.ok(banner, 'the banner root element must carry the .tl-project-banner class');
  const title = host.querySelector('.tl-project-banner__title');
  assert.ok(title, 'the banner has a title line naming the project');
  assert.ok(title.textContent.includes('myrepo'), 'the title must name the project');
  assert.ok(title.textContent.includes('/repo'), 'the title must also disclose the full path, not just the friendly name');
  const detail = host.querySelector('.tl-project-banner__text');
  assert.ok(detail, 'the banner has a detail line disclosing scope');
  assert.ok(
    /language server/i.test(detail.textContent) && /every|all/i.test(detail.textContent),
    'the detail line must state the grant covers every language server for this project, not just "start language servers"',
  );
});

check('F3: without a project name, the banner still discloses the path alone', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    bridge: { trustedProjects: async () => [], setProjectTrust: async () => {} },
    onDecision: () => {},
  });
  await handle.ready;
  const title = host.querySelector('.tl-project-banner__title');
  assert.ok(title.textContent.includes('/repo'), 'the path is still disclosed with no project name supplied');
});

// Review F9 (nit): role="note" alone is only announced when focused —
// aria-live="polite" is what actually gets an unsolicited banner announced.
check('F9: the banner container is aria-live="polite"', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    bridge: { trustedProjects: async () => [], setProjectTrust: async () => {} },
    onDecision: () => {},
  });
  await handle.ready;
  const banner = host.querySelector('.tl-project-banner');
  assert.strictEqual(banner.getAttribute('aria-live'), 'polite');
});

// Review F6 (low): a synchronous throw from bridge.setProjectTrust (e.g. the
// bridge is missing the method entirely) happens before Promise.resolve()'s
// argument is evaluated, so it never reaches a .catch on that promise chain
// — reviewer showed this bricks `decided` at true forever, making both
// buttons permanently dead.
check('F6: a Trust click that throws synchronously does not brick the banner', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    // setProjectTrust is missing entirely: calling it throws synchronously.
    bridge: { trustedProjects: async () => [] },
    onDecision: () => {},
  });
  await handle.ready;
  const trust = host.querySelector('[data-project-trust="trust"]');
  assert.doesNotThrow(() => trust.dispatchEvent({ type: 'click', target: trust }));
  await handle.settled();
  assert.ok(
    host.querySelector('[data-project-trust="trust"]'),
    'a synchronous throw must reset `decided` — the banner stays and Trust remains clickable, not permanently bricked',
  );
});

check('Not now dismisses for the window lifetime without touching trust', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const calls = [];
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    bridge: { trustedProjects: async () => [], setProjectTrust: async (...a) => { calls.push(a); } },
    onDecision: () => {},
  });
  await handle.ready;
  const later = host.querySelector('[data-project-trust="later"]');
  later.dispatchEvent({ type: 'click', target: later });
  await handle.settled();
  deepEq(calls, [], 'Not now must never write a trust record');
  assert.strictEqual(host.querySelector('[data-project-trust="later"]'), null, 'the banner is gone for this window');
});

// Binding mount contract (task-6 review): the project tree handle — and this
// banner's mount point, noticeHost — does NOT survive a dual-pane <-> project
// mode toggle. setProjectMode(true) builds a fresh handle and files-panel.js
// calls mount() again from scratch. "One ask per project" therefore has to be
// state that outlives any single mount() call, for as long as the window
// lives — this drives two separate mount() calls at the SAME sandbox (i.e.
// the same loaded module instance) to stand in for that toggle, rather than
// two independent `load()` calls which would each get a fresh module and
// prove nothing about cross-toggle memory.
check('a Not now dismissal survives a fresh mount() call for the same root (mode-toggle simulation)', async () => {
  const sandbox = load([BANNER]);
  const bridge = { trustedProjects: async () => [], setProjectTrust: async () => {} };

  const firstHost = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(firstHost);
  const first = sandbox.termlabProjectTrustBanner.mount({ host: firstHost, root: '/repo', bridge, onDecision: () => {} });
  await first.ready;
  const later = firstHost.querySelector('[data-project-trust="later"]');
  assert.ok(later, 'the banner shows on the first mount');
  later.dispatchEvent({ type: 'click', target: later });
  await first.settled();

  // A toggle away and back destroys the old host/handle and mounts fresh,
  // exactly as renderProjectTree() does on every setProjectMode(true).
  const secondHost = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(secondHost);
  const second = sandbox.termlabProjectTrustBanner.mount({ host: secondHost, root: '/repo', bridge, onDecision: () => {} });
  await second.ready;
  assert.strictEqual(secondHost.children.length, 0,
    'a project dismissed with Not now must stay dismissed across a mode toggle, for the window\'s lifetime');
});

check('an already-trusted project never renders a banner at all', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    bridge: {
      trustedProjects: async () => [{ root: '/repo', adapterId: null, decision: 'trusted' }],
      setProjectTrust: async () => {},
    },
    onDecision: () => {},
  });
  await handle.ready;
  assert.strictEqual(host.children.length, 0, 'nothing is rendered for a settled project');
});

// Review F5 (ruling): gate mount on editor.lsp.enabled, read the way the
// Settings dialog itself reads it — the full document from get_all_settings,
// at `.editor.lsp.enabled` (settings/store.js, sections-editor.js both read
// it there; get_app_config's flattened snapshot does not carry this field).
check('F5: editor.lsp.enabled === false suppresses the banner entirely', async () => {
  const sandbox = load([BANNER]);
  const host = sandbox.document.createElement('div');
  sandbox.document.body.appendChild(host);
  const invokeCalls = [];
  const handle = sandbox.termlabProjectTrustBanner.mount({
    host,
    root: '/repo',
    bridge: { trustedProjects: async () => [], setProjectTrust: async () => {} },
    invoke: async (cmd) => {
      invokeCalls.push(cmd);
      if (cmd === 'get_all_settings') return { editor: { lsp: { enabled: false } } };
      throw new Error(`unexpected command ${cmd}`);
    },
    onDecision: () => {},
  });
  await handle.ready;
  assert.ok(invokeCalls.includes('get_all_settings'), 'the gate must actually read settings');
  assert.strictEqual(host.children.length, 0, 'globally-disabled LSP must suppress the banner');
});

check('F5: a missing invoke, a settings read failure, or editor.lsp.enabled left unset all default to showing the banner', async () => {
  const bridge = { trustedProjects: async () => [], setProjectTrust: async () => {} };

  // No invoke supplied at all (mirrors every earlier test in this file).
  {
    const sandbox = load([BANNER]);
    const host = sandbox.document.createElement('div');
    sandbox.document.body.appendChild(host);
    const handle = sandbox.termlabProjectTrustBanner.mount({ host, root: '/repo', bridge, onDecision: () => {} });
    await handle.ready;
    assert.ok(host.children.length > 0, 'no invoke function must not suppress the banner');
  }

  // invoke rejects.
  {
    const sandbox = load([BANNER]);
    const host = sandbox.document.createElement('div');
    sandbox.document.body.appendChild(host);
    const handle = sandbox.termlabProjectTrustBanner.mount({
      host, root: '/repo', bridge, invoke: async () => { throw new Error('settings unavailable'); }, onDecision: () => {},
    });
    await handle.ready;
    assert.ok(host.children.length > 0, 'a failed settings read must fail open (LspConfig::default().enabled is true on the Rust side)');
  }

  // editor.lsp.enabled is present but not `false` (missing/undefined/true).
  {
    const sandbox = load([BANNER]);
    const host = sandbox.document.createElement('div');
    sandbox.document.body.appendChild(host);
    const handle = sandbox.termlabProjectTrustBanner.mount({
      host, root: '/repo', bridge, invoke: async () => ({ editor: {} }), onDecision: () => {},
    });
    await handle.ready;
    assert.ok(host.children.length > 0, 'only an explicit false suppresses the banner');
  }
});

check('a file under the root adopts the project root as its LSP context', () => {
  const sandbox = load([MODE, LSP_ROOT]);
  const mode = sandbox.termlabProjectMode;
  mode.set({ root: '/repo', name: 'repo' });
  const should = sandbox.termlabProjectLspRoot.shouldAdoptRoot;
  const choosing = { documentId: 'doc-1', status: { state: 'choosingProject' } };
  assert.strictEqual(should(choosing, '/repo/src/main.rs', mode), true);
  assert.strictEqual(should(choosing, '/elsewhere/lib.rs', mode), false,
    'a file outside the root keeps the loose-file behaviour: no prompt, no attach');
  assert.strictEqual(should({ documentId: 'doc-1', status: { state: 'ready' } }, '/repo/src/main.rs', mode), false,
    'a document that already has a context is left alone');
  assert.strictEqual(should(choosing, '/repo/src/main.rs', { isActive: () => false, isUnderRoot: () => false }), false,
    'no project means no adoption');
});

check('install sets the context exactly once per document', async () => {
  const sandbox = load([MODE, LSP_ROOT]);
  sandbox.termlabProjectMode.set({ root: '/repo', name: 'repo' });
  let listener = null;
  const contexts = [];
  const unsubscribe = sandbox.termlabProjectLspRoot.install({
    state: {
      subscribe: (fn) => { listener = fn; return () => { listener = null; }; },
    },
    bridge: {
      setProjectContext: async (documentId, context) => { contexts.push([documentId, context]); },
    },
    mode: sandbox.termlabProjectMode,
  });
  const pane = { kind: 'editor', remote: null, filePath: '/repo/src/main.rs' };
  listener(pane, { documentId: 'doc-1', status: { state: 'choosingProject' } });
  listener(pane, { documentId: 'doc-1', status: { state: 'choosingProject' } });
  await new Promise((resolve) => setTimeout(resolve, 0));
  deepEq(contexts, [['doc-1', { kind: 'root', root: '/repo' }]]);
  assert.strictEqual(typeof unsubscribe, 'function');
});

check('install leaves a remote pane and a pane with no file path alone', async () => {
  const sandbox = load([MODE, LSP_ROOT]);
  sandbox.termlabProjectMode.set({ root: '/repo', name: 'repo' });
  let listener = null;
  const contexts = [];
  sandbox.termlabProjectLspRoot.install({
    state: { subscribe: (fn) => { listener = fn; return () => {}; } },
    bridge: { setProjectContext: async (documentId, context) => { contexts.push([documentId, context]); } },
    mode: sandbox.termlabProjectMode,
  });
  const choosing = { documentId: 'doc-1', status: { state: 'choosingProject' } };
  listener({ kind: 'editor', remote: { hostLabel: 'h', remotePath: '/repo/src/main.rs' }, filePath: '/repo/src/main.rs' }, choosing);
  listener({ kind: 'editor', remote: null, filePath: null }, choosing);
  listener({ kind: 'terminal', remote: null, filePath: '/repo/src/main.rs' }, choosing);
  await new Promise((resolve) => setTimeout(resolve, 0));
  deepEq(contexts, [], 'a remote pane, a paneless document, and a non-editor pane must never adopt the project root');
});

// Review F1 (medium): lsp-state re-publishes session status on every
// diagnostics revision, so a REJECTED adoption (the manager rejects roots
// not among the document's own candidates — InvalidProjectRoot; real case:
// root.rs drops non-matching candidates for a JSON file under a
// package.json subtree) must never be retried — otherwise every republish
// re-fires lsp_set_project_context. Reviewer proved 5 notifies -> 5 invokes
// under the pre-fix code; this pins exactly one.
check('F1: a permanently-rejected adoption is never retried, even across repeated notifies', async () => {
  const sandbox = load([MODE, LSP_ROOT]);
  sandbox.termlabProjectMode.set({ root: '/repo', name: 'repo' });
  let listener = null;
  const calls = [];
  sandbox.termlabProjectLspRoot.install({
    state: { subscribe: (fn) => { listener = fn; return () => {}; } },
    bridge: {
      setProjectContext: async (documentId, context) => {
        calls.push([documentId, context]);
        throw new Error('InvalidProjectRoot');
      },
    },
    mode: sandbox.termlabProjectMode,
  });
  const pane = { kind: 'editor', remote: null, filePath: '/repo/pkg/manifest.json' };
  const choosing = { documentId: 'doc-json', status: { state: 'choosingProject' } };
  // Five republishes of the same choosingProject status, exactly as
  // lsp-state.js does on every diagnostics revision for a document whose
  // session never settles.
  for (let i = 0; i < 5; i += 1) {
    listener(pane, choosing);
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  assert.strictEqual(calls.length, 1, 'a rejected adoption must back off permanently, not resend on every republish');
});

// Review F2 (medium, mostly documentation): the fallback this rejection
// leaves behind is the per-file chooser (project-context.js), reachable
// because lsp-root.js only ever calls bridge.setProjectContext itself — it
// never wraps, replaces, or disables the bridge method the chooser uses, so
// a rejection for one document changes nothing about the bridge or state
// modules any other document (or the chooser's own explicit call) relies on.
check('F2: a rejected adoption leaves the bridge itself untouched — the per-file chooser path stays reachable', async () => {
  const sandbox = load([MODE, LSP_ROOT]);
  sandbox.termlabProjectMode.set({ root: '/repo', name: 'repo' });
  let listener = null;
  const calls = [];
  const bridge = {
    setProjectContext: async (documentId, context) => {
      calls.push([documentId, context]);
      if (documentId === 'doc-json') throw new Error('InvalidProjectRoot');
    },
  };
  sandbox.termlabProjectLspRoot.install({
    state: { subscribe: (fn) => { listener = fn; return () => {}; } },
    bridge,
    mode: sandbox.termlabProjectMode,
  });
  listener(
    { kind: 'editor', remote: null, filePath: '/repo/pkg/manifest.json' },
    { documentId: 'doc-json', status: { state: 'choosingProject' } },
  );
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.strictEqual(calls.length, 1);

  // The per-file chooser (a human choosing a project explicitly, e.g. via
  // project-context.js's chooseProject()) calls the SAME bridge method
  // directly — this must still work normally for a different document,
  // proving the module-internal `failed` bookkeeping is scoped per document
  // id and never touches the bridge or any other document's path.
  await bridge.setProjectContext('doc-other', { kind: 'root', root: '/repo' });
  deepEq(calls[1], ['doc-other', { kind: 'root', root: '/repo' }],
    'the bridge call the per-file chooser would make is unaffected by another document\'s rejected adoption');
});

// TEST HOLE (required): the reviewer showed that swapping mode.isUnderRoot()
// for a naive string startsWith() inside the adoption path passes every
// Task 7 check that existed before this fix round — because the only
// out-of-root fixture used ('/elsewhere/lib.rs') shares no string prefix
// with the root at all. This drives the full install() PATH (not the
// isUnderRoot/shouldAdoptRoot unit tests, which already cover the pure
// function) with a TRUE sibling-prefix case: '/repository' shares the
// string prefix '/repo' with the root '/repo' but is a different directory.
// A regression that swapped isUnderRoot for startsWith(root) here would
// send the project root as this document's context; this pins that it must
// not.
check('TEST HOLE: install() never adopts the root for a sibling path that merely shares a string prefix', async () => {
  const sandbox = load([MODE, LSP_ROOT]);
  sandbox.termlabProjectMode.set({ root: '/repo', name: 'repo' });
  let listener = null;
  const contexts = [];
  sandbox.termlabProjectLspRoot.install({
    state: { subscribe: (fn) => { listener = fn; return () => {}; } },
    bridge: { setProjectContext: async (documentId, context) => { contexts.push([documentId, context]); } },
    mode: sandbox.termlabProjectMode,
  });
  listener(
    { kind: 'editor', remote: null, filePath: '/repository/src/x.rs' },
    { documentId: 'doc-sibling', status: { state: 'choosingProject' } },
  );
  await new Promise((resolve) => setTimeout(resolve, 0));
  deepEq(contexts, [],
    '"/repository" shares a string prefix with "/repo" but is a different directory — a startsWith(root) regression would have adopted it');
});

check('the project feature modules use no regex lookbehind and no control bytes', () => {
  for (const file of [BANNER, LSP_ROOT]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(!/\(\?<[=!]/.test(source), `${file} uses a lookbehind`);
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
        `${file}: control byte at offset ${i}`);
    }
  }
});

check('index.html loads the trust banner and the LSP root adapter after the bridge', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(name);
  assert.ok(at('app/features/project/trust-banner.js') > 0);
  assert.ok(at('app/features/project/lsp-root.js') > 0);
  assert.ok(at('app/features/editor/lsp-bridge.js') < at('app/features/project/lsp-root.js'));
  assert.ok(at('app/features/editor/lsp-state.js') < at('app/features/project/lsp-root.js'));
  assert.ok(at('app/features/project/trust-banner.js') < at('app/panels/files-panel.js'));
});

check('files-panel.js mounts the trust banner into the project tree\'s noticeHost, not directly into the panel', () => {
  const src = fs.readFileSync(FILES_PANEL, 'utf8');
  assert.ok(src.includes('termlabProjectTrustBanner'), 'renderProjectTree wires the banner module');
  assert.ok(src.includes('projectTreeHandle.noticeHost'), 'the banner mounts into the handle\'s noticeHost, per the task-6 review contract');
});

check('tool-window-runtime installs the LSP root pass-through once per window', () => {
  const src = fs.readFileSync(path.join(APP, 'tool-window-runtime.js'), 'utf8');
  assert.ok(src.includes('termlabProjectLspRoot'), 'the runtime wires the lsp-root module');
  assert.ok(src.includes('termlabProjectLspRoot.install'), 'install() is actually called, not merely referenced');
});

check('Open Folder is reachable from the menu, the palette and Rust', () => {
  const actions = fs.readFileSync(path.join(APP, 'menu-actions.js'), 'utf8');
  assert.ok(actions.includes("action === 'open-folder'"), 'menu-actions handles open-folder');
  assert.ok(actions.includes("invoke('project_pick_folder')"), 'it uses the native directory picker');
  assert.ok(actions.includes("invoke('project_open'"), 'the picked folder is opened as a project');

  const palette = fs.readFileSync(path.join(APP, 'command-palette-runtime.js'), 'utf8');
  assert.ok(palette.includes("'core:open-folder'"), 'the palette exposes Open Folder as a Project');
  assert.ok(palette.includes("handleMenuAction('open-folder')"), 'the palette routes through the one handler');

  const menuRs = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/menu.rs'), 'utf8');
  assert.ok(menuRs.includes('MENU_OPEN_FOLDER_ID'), 'the File menu has an Open Folder id');
  assert.ok(menuRs.includes('"open-folder"'), 'the menu action string exists');
  // Both builders: the plugin-aware rebuild must not silently drop the item.
  const occurrences = menuRs.split('MENU_OPEN_FOLDER_ID').length - 1;
  assert.ok(occurrences >= 4, `Open Folder must appear in both menu builders, saw ${occurrences} references`);
});

check('the files panel switches on projectRoot and can toggle back to dual-pane', () => {
  const src = fs.readFileSync(FILES_PANEL, 'utf8');
  assert.ok(src.includes('opts.projectRoot'), 'init takes a project root');
  assert.ok(src.includes('termlabProjectTree'), 'project mode renders the tree module');
  assert.ok(src.includes('fp-pane-container'), 'the dual-pane path is still built for non-project windows');
  assert.ok(src.includes('isProjectMode'), 'the mode is queryable');
  assert.ok(src.includes('setProjectMode'), 'the header toggle can switch back to dual-pane');
  assert.ok(src.includes('projectTree'), 'the tree handle is reachable for git tints and the trust banner');
});

check('the tree context menu reuses the panel local operations plus new file and reveal', () => {
  const src = fs.readFileSync(FILES_PANEL, 'utf8');
  assert.ok(src.includes('buildTreeContextMenuItems'), 'the tree has its own item list');
  for (const label of ["'New File…'", "'New Folder…'", "'Rename…'", "'Delete'", "'Copy Path'", "'Reveal in File Manager'"]) {
    assert.ok(src.includes(label), `the tree context menu is missing ${label}`);
  }
  assert.ok(src.includes("invoke('project_reveal_path'"), 'reveal goes through the Rust command');
  assert.ok(src.includes('doNewFile'), 'New File is a real local operation, not a stub');
});

check('the SFTP tool window stays reachable and is registered with a project-aware title', () => {
  const src = fs.readFileSync(path.join(APP, 'tool-window-runtime.js'), 'utf8');
  assert.ok(src.includes('projectRoot'), 'the runtime hands the project root to the files panel');
  assert.ok(src.includes("termlabProjectMode"), 'the runtime asks the mode resolver');
  const register = src.indexOf("register('file-explorer'");
  const mode = src.indexOf('termlabProjectMode');
  assert.ok(mode < register || src.indexOf('projectRoot', register) > register,
    'the root must be known by the time file-explorer registers');
});

// task-6 review, F3/F5(iv): this used to also assert `src.includes('knowsBottom')`
// — the ORIGINAL guard that suppressed the reveal whenever the saved layout
// already recorded a bottom-zone window. The controller's ruling dropped
// that guard entirely (it was a near-universal no-op: the layout is global,
// not per-project, and every install's layout records a bottom-zone window
// the moment file-explorer's own default zone has ever been used). A grep
// for the string 'knowsBottom' would have kept passing anyway even with the
// guard removed, purely by coincidence — it also matches the unrelated,
// still-present `knowsBottomZone` identifier from the SFTP-bottom-zone
// migration a few dozen lines above this block — which is exactly the kind
// of source-grep-that-survives-any-fix problem the review flagged. The real
// regression coverage (the reveal firing unconditionally, AND the
// zen-effective override still winning over it) is a BEHAVIORAL test in
// test_panel_host.mjs (checks 16a/16b), which actually executes
// tool-window-runtime.js's init() against a fake toolWindowManager — chosen
// over duplicating that harness here because test_panel_host.mjs already
// has the exact fake-manager/loadRuntime machinery this needs.
check('a project window opens with the Files tool window visible, unconditionally (not gated on a stale knowsBottom guard)', () => {
  const src = fs.readFileSync(path.join(APP, 'tool-window-runtime.js'), 'utf8');
  // Task 12/F7: this now passes { save: false }, the same suppression the
  // panel-visibility reveal right above it already used — a project window's
  // OWN project_layouts entry must not be written from transient boot state.
  assert.ok(src.includes("activate('file-explorer', { save: false })"), 'a project window activates the Files panel without saving');
  assert.ok(!src.includes('const knowsBottom ='), 'the knowsBottom guard must be gone, not merely bypassed');
  const register = src.indexOf('registerBuiltInToolWindows();');
  const activate = src.indexOf("activate('file-explorer', { save: false })");
  assert.ok(register < activate, 'the panel must be registered before it can be activated');
  const zenBlock = src.indexOf('window.__termlabEffectiveZen === true');
  assert.ok(zenBlock > activate,
    'the zen-effective override must run AFTER the reveal, so it is still the last word on visibility');
});

// ===========================================================================
// Behavioral harness (task-6 review, F5): files-panel.js driven for real,
// with `projectRoot` set and window.termlabProjectTree stubbed — the pattern
// test_files_dnd.mjs's/test_sftp_connect.mjs's setupLogicHarness already
// established (real files-panel.js + real data-service/pane-store/actions,
// termlabFilesPaneView stubbed as a recording spy) extended with a fake
// project tree. The four checks that follow replace/augment the
// source-grep-only checks above: reverting any ONE of the four review-found
// deviations (F10's statLocal swap aside — that one has no independent
// behavioral signature) leaves at least one of these red, which a plain
// `.includes('buildTreeContextMenuItems')`-style check cannot say.
// ===========================================================================

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = async (times = 6) => { for (let i = 0; i < times; i += 1) await tick(); };

// A minimal element good enough for files-panel.js's DOM touches in the
// project-tree render path: classList, appendChild/querySelector (the
// latter overridden per-fixture, same idiom test_files_dnd.mjs/
// test_sftp_connect.mjs use), a settable .innerHTML (never actually parsed —
// nothing in this harness needs it to be), and `closest('[data-tree-path]')`
// for the F13 background-vs-row contextmenu distinction.
function makeElement(tag) {
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    dataset: {},
    className: '',
    title: '',
    textContent: '',
    value: '',
    parentNode: null,
    firstElementChild: undefined,
    classList: {
      _set: new Set(),
      add(...c) { c.forEach((x) => this._set.add(x)); },
      remove(...c) { c.forEach((x) => this._set.delete(x)); },
      contains(c) { return this._set.has(c); },
    },
    appendChild(child) {
      this.children.push(child);
      if (child && typeof child === 'object') child.parentNode = this;
      return child;
    },
    removeChild(child) {
      const i = this.children.indexOf(child);
      if (i >= 0) this.children.splice(i, 1);
      if (child && typeof child === 'object') child.parentNode = null;
      return child;
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    removeEventListener() {},
    dispatchEvent(evt) {
      for (const fn of (listeners.get(evt.type) || []).slice()) fn(evt);
      return true;
    },
    setAttribute(n, v) { el._attrs = el._attrs || new Map(); el._attrs.set(n, String(v)); },
    getAttribute(n) { return el._attrs && el._attrs.has(n) ? el._attrs.get(n) : null; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }; },
    focus() {},
    select() {},
    closest(sel) {
      let node = el;
      while (node) {
        if (sel === '[data-tree-path]' && node._attrs && node._attrs.has('data-tree-path')) return node;
        node = node.parentNode;
      }
      return null;
    },
  };
  return el;
}

// One fake tree "instance" per create() call — the toggle round-trip check
// (F5(ii)) needs to tell two separate handles apart (a fresh one must not be
// the destroyed one it replaced).
function makeFakeTreeHandle(id, opCalls) {
  let destroyed = false;
  const element = makeElement('div');
  return {
    _id: id,
    element,
    isDestroyed: () => destroyed,
    expand: (p) => { opCalls.push({ id, op: 'expand', path: p }); },
    collapse: (p) => { opCalls.push({ id, op: 'collapse', path: p }); },
    refresh: (p) => { opCalls.push({ id, op: 'refresh', path: p }); return Promise.resolve(); },
    refreshAll: () => { opCalls.push({ id, op: 'refreshAll' }); return Promise.resolve(); },
    settled: () => Promise.resolve(),
    activePath: () => null,
    rows: () => [],
    setGitStatus: () => {},
    setMissing: (v) => { opCalls.push({ id, op: 'setMissing', value: v === true }); },
    setShowHidden: () => {},
    focus: () => {},
    noticeHost: makeElement('div'),
    destroy: () => { destroyed = true; opCalls.push({ id, op: 'destroy' }); },
  };
}

// tlDialog.open() stub good enough for showTextPromptDialog AND
// showConfirmDialog: both call window.tlDialog.open(cfg) once, read
// cfg.buttons for a `primary` entry to trigger confirmation, and (the text
// prompt only) read `handle.el.querySelector('#fp-dlg-input').value` inside
// their own `confirm()` closure — never anything this stub has to interpret
// itself. Capturing `cfg` and a controllable fake input is all a caller
// needs to drive either dialog kind end to end.
function makeDialogStub() {
  const opens = [];
  return {
    opens,
    tlDialog: {
      open(cfg) {
        const bodyEl = makeElement('div');
        const inputEl = makeElement('input');
        bodyEl.querySelector = (sel) => (sel === '#fp-dlg-input' ? inputEl : null);
        if (typeof cfg.body === 'function') cfg.body(bodyEl);
        const handle = { el: bodyEl, close() {} };
        opens.push({ cfg, handle, inputEl });
        return handle;
      },
    },
  };
}

function confirmPrompt(dialogs, value) {
  const { cfg, inputEl } = dialogs.opens[dialogs.opens.length - 1];
  inputEl.value = value;
  const primary = cfg.buttons.find((b) => b.primary);
  primary.onSelect();
}

function confirmDialog(dialogs) {
  const { cfg } = dialogs.opens[dialogs.opens.length - 1];
  const primary = cfg.buttons.find((b) => b.primary);
  primary.onSelect();
}

async function setupProjectFilesHarness(options) {
  const opts = options || {};
  const invokeCalls = [];
  const toastCalls = [];
  const renderCalls = [];
  const treeCreateCalls = [];
  const treeHandles = [];
  const treeOpCalls = [];
  const openedFiles = [];
  const listeners = {};
  const localStatOverrides = new Map(Object.entries(opts.localStatOverrides || {}));

  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    const extra = typeof opts.invokeExtra === 'function' ? opts.invokeExtra(cmd, args) : undefined;
    if (extra !== undefined) return extra;
    switch (cmd) {
      case 'get_home_dir': return Promise.resolve('/home/demo');
      case 'get_all_settings': return Promise.resolve({});
      case 'local_list_dir': return Promise.resolve([]);
      case 'remote_get_servers': return Promise.resolve({ folders: [], ungrouped: [], ssh_config: [] });
      case 'remote_get_sessions': return Promise.resolve([]);
      case 'local_stat': {
        const p = args && args.path;
        if (localStatOverrides.has(p)) {
          const entry = localStatOverrides.get(p);
          return entry instanceof Error ? Promise.reject(entry) : Promise.resolve(entry);
        }
        return Promise.reject(new Error(`ENOENT: no such file or directory, stat '${p}'`));
      }
      case 'editor_write_file': return Promise.resolve(undefined);
      case 'local_mkdir': return Promise.resolve(undefined);
      case 'local_rename': return Promise.resolve(undefined);
      case 'local_remove': return Promise.resolve(undefined);
      case 'clipboard_write_text': return Promise.resolve(undefined);
      case 'project_reveal_path': return Promise.resolve(undefined);
      default: return Promise.resolve(undefined);
    }
  };

  const sandbox = {
    console, setTimeout, clearTimeout,
    setInterval: () => 0,
    clearInterval: () => {},
    Promise, Math, Array, JSON, Object, String, Number,
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  sandbox.document = { createElement: (t) => makeElement(t) };
  sandbox.utils = {
    formatSize: () => '', formatDate: () => '',
    esc: (v) => String(v == null ? '' : v), attr: (v) => String(v == null ? '' : v),
  };
  sandbox.toast = {
    error: (...args) => toastCalls.push({ kind: 'error', args }),
    info: (...args) => toastCalls.push({ kind: 'info', args }),
    warn: (...args) => toastCalls.push({ kind: 'warn', args }),
    success: (...args) => toastCalls.push({ kind: 'success', args }),
  };
  sandbox.toolWindowManager = { isVisible: () => true, activate() {}, deactivate() {}, toggle() {} };
  const dialogs = makeDialogStub();
  sandbox.tlDialog = dialogs.tlDialog;
  sandbox.termlabFilesPaneView = {
    renderPane: (pane, el, deps) => { renderCalls.push({ pane, el, deps }); },
    showColumnMenu: () => {},
    showRowContextMenu: (event, items) => { sandbox.__lastMenuItems = items; },
  };
  sandbox.termlabProjectTree = {
    create(createOptions) {
      treeCreateCalls.push(createOptions);
      const handle = makeFakeTreeHandle(treeCreateCalls.length, treeOpCalls);
      treeHandles.push(handle);
      return handle;
    },
  };
  sandbox.termlabEditorService = {
    openLocalFile: (p) => { openedFiles.push(p); return Promise.resolve(); },
  };

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(FILES_PANE_STORE_PATH, 'utf8'), sandbox, { filename: FILES_PANE_STORE_PATH });
  vm.runInContext(fs.readFileSync(FILES_DATA_SERVICE_PATH, 'utf8'), sandbox, { filename: FILES_DATA_SERVICE_PATH });
  vm.runInContext(fs.readFileSync(FILES_ACTIONS_PATH, 'utf8'), sandbox, { filename: FILES_ACTIONS_PATH });
  vm.runInContext(fs.readFileSync(FILES_PANEL, 'utf8'), sandbox, { filename: FILES_PANEL });

  const panelEl = makeElement('div');
  const localRootEl = makeElement('div');
  const remoteRootEl = makeElement('div');
  panelEl.querySelector = (sel) => {
    if (sel === '#fp-local') return localRootEl;
    if (sel === '#fp-remote') return remoteRootEl;
    if (sel === '.fp-pane-container') return makeElement('div');
    return null;
  };

  sandbox.filesPanel.init({
    invoke,
    panelEl,
    panelWrapEl: makeElement('div'),
    resizeHandleEl: makeElement('div'),
    layoutService: null,
    fitActiveTab: () => {},
    getActiveTab: () => null,
    listen: (name, handler) => { listeners[name] = handler; },
    projectRoot: opts.projectRoot,
  });
  await settle();

  return {
    sandbox, invoke, invokeCalls, toastCalls, renderCalls, listeners,
    treeCreateCalls, treeHandles, treeOpCalls, openedFiles, dialogs, panelEl,
    lastMenuItems: () => sandbox.__lastMenuItems,
  };
}

check('F5(i): init({projectRoot}) renders the project tree, not the dual-pane explorer', async () => {
  const h = await setupProjectFilesHarness({ projectRoot: '/repo' });
  assert.strictEqual(h.treeCreateCalls.length, 1, 'exactly one tree must be created on boot');
  assert.strictEqual(h.treeCreateCalls[0].root, '/repo', 'the tree must be rooted at projectRoot');
  assert.ok(h.panelEl.children.includes(h.treeHandles[0].element),
    'the tree\'s element must actually be mounted into the panel');
  assert.strictEqual(h.sandbox.filesPanel.isProjectMode(), true);
  assert.strictEqual(h.sandbox.filesPanel.projectTree(), h.treeHandles[0]);
});

check('fix round 1, F6: a successful reopen through the missing-root recovery button refreshes the native menu', async () => {
  const h = await setupProjectFilesHarness({
    projectRoot: '/repo',
    invokeExtra: (cmd) => {
      if (cmd === 'project_pick_folder') return Promise.resolve('/new/repo');
      if (cmd === 'project_open') {
        return Promise.resolve({ root: '/new/repo', name: 'repo', windowLabel: 'window-2', focusedExisting: false });
      }
      return undefined;
    },
  });
  const onReopen = h.treeCreateCalls[0].onReopen;
  assert.strictEqual(typeof onReopen, 'function', 'files-panel.js must wire onReopen into the tree');
  onReopen();
  await settle();
  const cmds = h.invokeCalls.map((c) => c.cmd);
  assert.ok(cmds.includes('project_pick_folder'));
  assert.ok(cmds.includes('project_open'));
  assert.ok(cmds.includes('rebuild_menu'),
    'a successful reopen must refresh the native File menu with the newly-recorded recent');
});

check('fix round 1, F6: a cancelled reopen (no folder picked) never calls project_open or rebuild_menu', async () => {
  const h = await setupProjectFilesHarness({ projectRoot: '/repo' });
  const onReopen = h.treeCreateCalls[0].onReopen;
  onReopen();
  await settle();
  const cmds = h.invokeCalls.map((c) => c.cmd);
  assert.ok(cmds.includes('project_pick_folder'));
  assert.ok(!cmds.includes('project_open'), 'cancelling the picker must not open anything');
  assert.ok(!cmds.includes('rebuild_menu'), 'nothing was recorded, so nothing needs to refresh');
});

check('F5(ii)/F2: toggle round trip project -> dual-pane -> project renders the remote pane and never reuses a destroyed tree handle', async () => {
  const h = await setupProjectFilesHarness({ projectRoot: '/repo' });
  const firstHandle = h.treeHandles[0];
  assert.strictEqual(firstHandle.isDestroyed(), false);

  h.renderCalls.length = 0;
  h.sandbox.filesPanel.setProjectMode(false);
  await settle();

  assert.strictEqual(firstHandle.isDestroyed(), true, 'leaving project mode must destroy the tree handle');
  // F2: renderDualPane must populate #fp-remote every time it runs, not just
  // once at the original init() — before the fix, refreshHostCombo() (and
  // therefore this renderPane call) never ran again after the initial
  // project-mode boot, so the SFTP half stayed permanently blank on toggle.
  const remoteRender = h.renderCalls.find((c) => c.pane && c.pane.prefix === 'remote');
  assert.ok(remoteRender, 'toggling back to dual-pane must render the remote pane (F2)');
  assert.strictEqual(remoteRender.el, h.panelEl.querySelector('#fp-remote'));

  h.sandbox.filesPanel.setProjectMode(true);
  await settle();
  assert.strictEqual(h.treeCreateCalls.length, 2, 'toggling back to project mode must create a FRESH tree handle');
  const secondHandle = h.treeHandles[1];
  assert.notStrictEqual(secondHandle, firstHandle, 'the fresh handle must not be the destroyed one it replaced');
  assert.strictEqual(secondHandle.isDestroyed(), false);
  assert.strictEqual(h.sandbox.filesPanel.projectTree(), secondHandle);
});

// F5(iii): the deviation-(b) bug class from the original task-6 pass — a
// shared pseudo-pane whose `currentPath` was the wrong directory for a
// DIRECTORY node — must not be able to return silently. Asserted against the
// real invoke() call args, for both a file node (which was already correct)
// and a directory node (which was not).
check('F5(iii): buildTreeContextMenuItems targets the right paths for a FILE node', async () => {
  const h = await setupProjectFilesHarness({ projectRoot: '/repo' });
  const onContextMenu = h.treeCreateCalls[0].onContextMenu;
  onContextMenu({}, { path: '/repo/src/main.rs', name: 'main.rs', isDir: false, parentPath: '/repo/src' });
  const items = h.lastMenuItems();
  assert.ok(items, 'the context menu must have been shown');

  h.invokeCalls.length = 0;
  items.find((i) => i.label === 'Rename…').action();
  confirmPrompt(h.dialogs, 'renamed.rs');
  await settle();
  const rename = h.invokeCalls.find((c) => c.cmd === 'local_rename');
  assert.ok(rename, 'Rename must invoke local_rename');
  assert.strictEqual(rename.args.from, '/repo/src/main.rs');
  assert.strictEqual(rename.args.to, '/repo/src/renamed.rs');

  h.invokeCalls.length = 0;
  items.find((i) => i.label === 'Copy Path').action();
  await settle();
  const copy = h.invokeCalls.find((c) => c.cmd === 'clipboard_write_text');
  assert.ok(copy, 'Copy Path must invoke clipboard_write_text');
  assert.strictEqual(copy.args.text, '/repo/src/main.rs');
});

check('F5(iii): buildTreeContextMenuItems targets the right paths for a DIRECTORY node (the deviation-(b) bug class)', async () => {
  const h = await setupProjectFilesHarness({ projectRoot: '/repo' });
  const onContextMenu = h.treeCreateCalls[0].onContextMenu;
  onContextMenu({}, { path: '/repo/src', name: 'src', isDir: true, parentPath: '/repo' });
  const items = h.lastMenuItems();

  // Rename: under the original brief-verbatim pseudo-pane, `from` would have
  // computed as '/repo/src/src' (containingDir === node.path for a
  // directory, so joining the directory's own name onto itself targets a
  // nonexistent child of itself instead of the directory).
  h.invokeCalls.length = 0;
  items.find((i) => i.label === 'Rename…').action();
  confirmPrompt(h.dialogs, 'lib');
  await settle();
  const rename = h.invokeCalls.find((c) => c.cmd === 'local_rename');
  assert.ok(rename, 'Rename must invoke local_rename');
  assert.strictEqual(rename.args.from, '/repo/src');
  assert.strictEqual(rename.args.to, '/repo/lib');

  // Copy Path: same bug class — would have copied '/repo/src/src'.
  h.invokeCalls.length = 0;
  items.find((i) => i.label === 'Copy Path').action();
  await settle();
  const copy = h.invokeCalls.find((c) => c.cmd === 'clipboard_write_text');
  assert.ok(copy, 'Copy Path must invoke clipboard_write_text');
  assert.strictEqual(copy.args.text, '/repo/src');

  // Delete: same bug class — would have tried to delete '/repo/src/src'.
  h.invokeCalls.length = 0;
  items.find((i) => i.label === 'Delete').action();
  confirmDialog(h.dialogs);
  await settle();
  const remove = h.invokeCalls.find((c) => c.cmd === 'local_remove');
  assert.ok(remove, 'Delete must invoke local_remove');
  assert.strictEqual(remove.args.path, '/repo/src');
  assert.strictEqual(remove.args.isDir, true);

  // New Folder still targets the directory's own contents (containingDir),
  // which the file/directory split must NOT have broken.
  h.invokeCalls.length = 0;
  items.find((i) => i.label === 'New Folder…').action();
  confirmPrompt(h.dialogs, 'nested');
  await settle();
  const mkdir = h.invokeCalls.find((c) => c.cmd === 'local_mkdir');
  assert.ok(mkdir, 'New Folder must invoke local_mkdir');
  assert.strictEqual(mkdir.args.path, '/repo/src/nested');
});

// F5(v): the doNewFile collision guard. F12 (controller ruling) is covered
// in the same two-part flow for free: the happy path (ii) asserts the
// created dir expands and the new file opens in the editor.
check('F5(v): doNewFile refuses to overwrite an existing file, and (F12) creating a new one expands the dir and opens it', async () => {
  // (i) collision: local_stat RESOLVES for the target -> refuse, no write.
  {
    const h = await setupProjectFilesHarness({
      projectRoot: '/repo',
      localStatOverrides: {
        '/repo/src/main.rs': { name: 'main.rs', is_dir: false, size: 3, modified: 0, permissions: null },
      },
    });
    const onContextMenu = h.treeCreateCalls[0].onContextMenu;
    onContextMenu({}, { path: '/repo/src', name: 'src', isDir: true, parentPath: '/repo' });
    const items = h.lastMenuItems();

    h.invokeCalls.length = 0;
    h.toastCalls.length = 0;
    items.find((i) => i.label === 'New File…').action();
    confirmPrompt(h.dialogs, 'main.rs');
    await settle();

    assert.ok(!h.invokeCalls.some((c) => c.cmd === 'editor_write_file'),
      'a name that already exists must never reach editor_write_file (it would silently truncate it)');
    const errorToast = h.toastCalls.find((t) => t.kind === 'error');
    assert.ok(errorToast, 'the collision must be reported');
    assert.strictEqual(errorToast.args[0], 'New File Failed');
  }

  // (ii) happy path: local_stat REJECTS (nothing there) -> write proceeds,
  // the containing dir expands, and the new file opens in the editor.
  {
    const h = await setupProjectFilesHarness({ projectRoot: '/repo' });
    const onContextMenu = h.treeCreateCalls[0].onContextMenu;
    onContextMenu({}, { path: '/repo/src', name: 'src', isDir: true, parentPath: '/repo' });
    const items = h.lastMenuItems();

    h.invokeCalls.length = 0;
    items.find((i) => i.label === 'New File…').action();
    confirmPrompt(h.dialogs, 'new.rs');
    await settle();

    const write = h.invokeCalls.find((c) => c.cmd === 'editor_write_file');
    assert.ok(write, 'a genuinely new name must reach editor_write_file');
    assert.strictEqual(write.args.path, '/repo/src/new.rs');
    assert.strictEqual(write.args.contents, '');

    const expandCall = h.treeOpCalls.find((c) => c.op === 'expand' && c.path === '/repo/src');
    assert.ok(expandCall, 'F12: the containing directory must expand so the new file is not created-but-invisible');
    assert.deepStrictEqual(h.openedFiles, ['/repo/src/new.rs'],
      'F12: the new file must open straight into the editor (IDE convention)');
  }
});

// F13: the tree-background context menu (no row target at all) offers New
// File/New Folder scoped to projectRoot.
check('F13: right-clicking the tree background (no row) offers New File/New Folder at the project root', async () => {
  const h = await setupProjectFilesHarness({ projectRoot: '/repo' });
  const treeElement = h.treeHandles[0].element;
  let defaultPrevented = false;
  treeElement.dispatchEvent({
    type: 'contextmenu',
    target: treeElement,
    preventDefault: () => { defaultPrevented = true; },
  });
  assert.ok(defaultPrevented, 'the background click must suppress the native context menu');
  const items = h.lastMenuItems();
  assert.ok(items, 'a root context menu must have been shown');
  const labels = items.map((i) => i.label).filter(Boolean);
  assert.ok(labels.includes('New File…'));
  assert.ok(labels.includes('New Folder…'));

  h.invokeCalls.length = 0;
  items.find((i) => i.label === 'New Folder…').action();
  confirmPrompt(h.dialogs, 'newdir');
  await settle();
  const mkdir = h.invokeCalls.find((c) => c.cmd === 'local_mkdir');
  assert.ok(mkdir, 'New Folder from the background menu must invoke local_mkdir');
  assert.strictEqual(mkdir.args.path, '/repo/newdir', 'it must land directly in projectRoot');
});

// F13 continued: a right-click that DOES resolve to a row must not ALSO
// trigger the background menu (no double-firing).
check('F13: a right-click that resolves to a row does not also trigger the background menu', async () => {
  const h = await setupProjectFilesHarness({ projectRoot: '/repo' });
  const treeElement = h.treeHandles[0].element;
  const row = makeElement('div');
  row.setAttribute('data-tree-path', '/repo/src');
  row.parentNode = treeElement;
  let defaultPrevented = false;
  treeElement.dispatchEvent({
    type: 'contextmenu',
    target: row,
    preventDefault: () => { defaultPrevented = true; },
  });
  assert.strictEqual(defaultPrevented, false,
    'a row click must be left to project-tree.js\'s own listener, never intercepted here');
  assert.strictEqual(h.lastMenuItems(), undefined, 'no root menu must have been shown for a row click');
});

check('the panel modules use no regex lookbehind and no control bytes', () => {
  for (const file of [FILES_PANEL]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(!/\(\?<[=!]/.test(source), `${file} uses a lookbehind`);
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
        `${file}: control byte at offset ${i}`);
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
