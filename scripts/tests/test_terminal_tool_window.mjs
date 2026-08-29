// Run: node scripts/tests/test_terminal_tool_window.mjs
//
// The Terminal tool window: a real xterm on the same PTY path terminal TABS
// use, hosted in the bottom zone of every window, plus the project-window
// boot shape it replaces (bottom zone opens on the Terminal, main area boots
// empty with a placeholder, and closing every tab no longer spawns one).
//
// Four parts:
//   Part 1 — app/layout/tool-window-manager.js, loaded for real, for the two
//   registration options this feature needed: `autoActivate: false` (so the
//   first registrant of the empty bottom-right zone does not claim it on
//   every fresh profile) and `poppable: false` (a PTY is keyed by THIS
//   window's label, so there is nothing for a panel host to host).
//
//   Part 2 — app/panels/terminal-panel.js, loaded for real against a stubbed
//   terminal runtime: lazy spawn on first render, hide-keeps-the-shell,
//   pty-exit state and respawn, and the teardown contract.
//
//   Part 3 — the boot decision, source-level: registration order and scope in
//   tool-window-runtime.js, main-runtime.js's project branch, index.html.
//
//   Part 4 — app/tab-manager.js, loaded for real: the project-aware zero-tabs
//   fallback and the main-area placeholder.
//
// No jsdom in this repo (see test_tl_dialog.mjs for the precedent). The DOM
// stub is the one test_panel_host.mjs uses. Deliberately does NOT define
// `sandbox.global` — see test_problems_panel.mjs's note: tool-window-
// manager.js binds `exports`, never `global`, and a harness that helpfully
// supplies one would hide a bare `global.` regression.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const MANAGER_PATH = path.join(APP, 'layout/tool-window-manager.js');
const PANEL_PATH = path.join(APP, 'panels/terminal-panel.js');
const RUNTIME_PATH = path.join(APP, 'tool-window-runtime.js');
const TAB_MANAGER_PATH = path.join(APP, 'tab-manager.js');
const MAIN_RUNTIME_PATH = path.join(APP, 'main-runtime.js');
const SHORTCUTS_PATH = path.join(APP, 'shortcut-runtime.js');
const CSS_PATH = path.join(ROOT, 'styles/design-system/components/terminal-panel.css');
const INDEX_HTML = path.join(ROOT, 'index.html');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

// --- shared element stub (from test_panel_host.mjs) --------------------------

function makeElement(tag) {
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    dataset: {},
    className: '',
    title: '',
    type: '',
    hidden: false,
    textContent: '',
    offsetWidth: 0,
    offsetHeight: 0,
    parentNode: null,
    classList: {
      _set: new Set(),
      add(...c) { c.forEach((x) => this._set.add(x)); },
      remove(...c) { c.forEach((x) => this._set.delete(x)); },
      toggle(c, force) {
        if (force === undefined) {
          if (this._set.has(c)) this._set.delete(c); else this._set.add(c);
        } else if (force) this._set.add(c);
        else this._set.delete(c);
      },
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
    insertBefore(child) { return this.appendChild(child); },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    contains(other) {
      if (other === el) return true;
      for (const child of el.children) {
        if (child === other) return true;
        if (child && typeof child.contains === 'function' && child.contains(other)) return true;
      }
      return false;
    },
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    _fire(name, event) {
      for (const fn of (listeners.get(name) || []).slice()) fn(event || {});
    },
    removeEventListener() {},
    setAttribute(n, v) { attrs.set(n, String(v)); },
    getAttribute(n) { return attrs.has(n) ? attrs.get(n) : null; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }; },
  };
  return el;
}

function findByClass(node, className) {
  if (!node) return null;
  const names = String(node.className || '').split(/\s+/);
  if (names.indexOf(className) !== -1) return node;
  for (const child of node.children || []) {
    const hit = findByClass(child, className);
    if (hit) return hit;
  }
  return null;
}

// ===========================================================================
// Part 1 — the two new register() options
// ===========================================================================

function makeZoneEl(zoneName) {
  const zoneEl = makeElement('div');
  zoneEl.dataset.zone = zoneName;
  const contentEl = makeElement('div');
  contentEl.className = 'zone-content';
  const tabStripEl = makeElement('div');
  tabStripEl.className = 'zone-tab-strip';
  zoneEl.querySelector = (sel) => {
    if (sel === '.zone-content') return contentEl;
    if (sel === '.zone-tab-strip') return tabStripEl;
    return null;
  };
  zoneEl._contentEl = contentEl;
  return zoneEl;
}

function loadManager(extra) {
  const body = makeElement('body');
  const zoneEls = new Map();
  for (const z of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right']) {
    zoneEls.set(z, makeZoneEl(z));
  }
  const byId = new Map();
  const invokeCalls = [];
  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    return Promise.resolve(undefined);
  };
  const menuOpens = [];
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    Promise,
    Math,
    Object,
    Array,
    Map,
    Set,
    JSON,
    String,
    Number,
    Error,
    requestAnimationFrame: (fn) => setTimeout(fn, 0),
    document: {
      body,
      createElement: (t) => makeElement(t),
      getElementById: (id) => byId.get(id) || null,
      querySelector: (sel) => {
        const m = /^\[data-zone="([^"]+)"\]$/.exec(sel);
        if (m) return zoneEls.get(m[1]) || null;
        return null;
      },
      querySelectorAll: () => [],
      addEventListener() {},
    },
    tlMenu: { open: (o) => { menuOpens.push(o); } },
  };
  sandbox.window = sandbox;
  Object.assign(sandbox, extra || {});
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MANAGER_PATH, 'utf8'), sandbox, { filename: MANAGER_PATH });
  return { twm: sandbox.toolWindowManager, sandbox, zoneEls, byId, invoke, invokeCalls, menuOpens };
}

function viewModeItems(items) {
  return Array.from(items).filter((i) => typeof i.label === 'string' && i.label.startsWith('View Mode:'));
}

check('autoActivate:false keeps a first registrant from claiming its empty zone', () => {
  const { twm, invoke, zoneEls } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.setPanelVisibility('bottom', true, { save: false });

  let renders = 0;
  twm.register('terminal', {
    title: 'Terminal',
    type: 'built-in',
    defaultZone: 'bottom-right',
    autoActivate: false,
    renderFn: () => { renders += 1; },
  });

  assert.strictEqual(twm.isVisible('terminal'), false,
    'a plain window must find the Terminal tool window hidden, not open');
  assert.strictEqual(renders, 0, 'no render means no PTY: the spawn is lazy, on first show');
  assert.strictEqual(zoneEls.get('bottom-right')._contentEl.children.length, 0);
  assert.strictEqual(twm.getZoneForWindow('terminal'), 'bottom-right',
    'the zone assignment is still recorded, so the rail button and a later activate() work');
});

check('the default (no autoActivate opt) still claims an empty zone, as before', () => {
  const { twm, invoke } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('file-explorer', {
    title: 'Project', type: 'built-in', defaultZone: 'bottom', renderFn: () => {},
  });
  assert.strictEqual(twm.isVisible('file-explorer'), true,
    'the first-registrant-activates-the-zone rule is unchanged for everyone else');
});

check('autoActivate:false still honours a layout that saved the window as active', () => {
  const { twm, invoke, zoneEls } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.setPersistedActiveZoneWindows({ 'bottom-right': 'terminal' });
  twm.setPanelVisibility('bottom', true, { save: false });

  let renders = 0;
  twm.register('terminal', {
    title: 'Terminal',
    type: 'built-in',
    defaultZone: 'bottom-right',
    autoActivate: false,
    renderFn: () => { renders += 1; },
  });

  assert.strictEqual(twm.isVisible('terminal'), true,
    'a returning window restores exactly what it saved — the opt-out is only about empty zones');
  assert.strictEqual(renders, 1, 'the restore renders it once');
  assert.strictEqual(zoneEls.get('bottom-right')._contentEl.children.length, 1);
});

check('poppable:false refuses window mode and disables the menu entry', () => {
  const { twm, invoke, invokeCalls } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('terminal', {
    title: 'Terminal',
    type: 'built-in',
    defaultZone: 'bottom-right',
    autoActivate: false,
    poppable: false,
    renderFn: () => {},
  });

  const modes = viewModeItems(twm.buildContextMenuItems('terminal'));
  assert.deepStrictEqual(modes.map((i) => i.label), ['View Mode: Dock', 'View Mode: Window'],
    'both flattened entries stay present — tl-menu has no submenus and the trait test pins this');
  assert.strictEqual(modes[1].disabled, true, 'View Mode: Window must be disabled, not missing');

  twm.setViewMode('terminal', 'window');
  assert.strictEqual(twm.getViewMode('terminal'), 'dock', 'setViewMode must refuse the pop-out');
  assert.deepStrictEqual(
    invokeCalls.filter((c) => c.cmd === 'open_panel_host'),
    [],
    'no panel host may be opened for a window that cannot be hosted',
  );
});

check('poppable:false ignores a persisted window mode from a hand-edited layout', () => {
  const { twm, invoke, invokeCalls } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.setPersistedViewModes({ terminal: 'window' });
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('terminal', {
    title: 'Terminal',
    type: 'built-in',
    defaultZone: 'bottom-right',
    autoActivate: false,
    poppable: false,
    renderFn: () => {},
  });
  assert.strictEqual(twm.getViewMode('terminal'), 'dock');
  assert.deepStrictEqual(invokeCalls.filter((c) => c.cmd === 'dock_panel_host'), []);
});

check('the poppable default is unchanged for every other tool window', () => {
  const { twm, invoke } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('ssh-sessions', { title: 'Hosts', type: 'built-in', defaultZone: 'right-top', renderFn: () => {} });
  const modes = viewModeItems(twm.buildContextMenuItems('ssh-sessions'));
  assert.strictEqual(modes[1].disabled, false, 'ordinary tool windows still pop out');
});

// ===========================================================================
// Part 2 — app/panels/terminal-panel.js
// ===========================================================================

function makeFakeTerm() {
  const dataHandlers = [];
  const textarea = makeElement('textarea');
  textarea.className = 'xterm-helper-textarea';
  return {
    textarea,
    focused: false,
    written: [],
    resets: 0,
    disposed: 0,
    onData(fn) { dataHandlers.push(fn); },
    onTitleChange() {},
    write(text) { this.written.push(text); },
    writeln(text) { this.written.push(text + '\n'); },
    reset() { this.resets += 1; },
    focus() { this.focused = true; },
    dispose() { this.disposed += 1; },
    options: {},
    _emitData(data) { for (const fn of dataHandlers) fn(data); },
  };
}

function loadPanel(options) {
  const opts = options || {};
  const body = makeElement('body');
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    Promise,
    Math,
    Object,
    Array,
    Map,
    Set,
    JSON,
    String,
    Number,
    Error,
    requestAnimationFrame: (fn) => setTimeout(fn, 0),
    document: {
      body,
      activeElement: body,
      createElement: (t) => makeElement(t),
      getElementById: () => null,
      querySelector: () => null,
      querySelectorAll: () => [],
      addEventListener() {},
    },
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(PANEL_PATH, 'utf8'), sandbox, { filename: PANEL_PATH });

  const invoked = [];
  const panes = new Map();
  const listeners = new Map();
  const term = makeFakeTerm();
  const fitAddon = {
    fits: 0,
    proposeDimensions: () => ({ cols: 120, rows: 30 }),
    fit() { this.fits += 1; },
  };
  const panelEl = sandbox.document.createElement('div');
  body.appendChild(panelEl);
  const unlistens = [];

  const handle = sandbox.termlabTerminalPanel.init(Object.assign({
    panelEl,
    paneId: 7,
    cwd: '/proj',
    panes,
    invoke: (cmd, args) => {
      invoked.push({ cmd, args });
      if (opts.failSpawn && cmd === 'spawn_shell') return Promise.reject(new Error('boom'));
      return Promise.resolve(undefined);
    },
    listen: (name, fn) => {
      listeners.set(name, fn);
      const un = () => { unlistens.push(name); };
      return Promise.resolve(un);
    },
    initTerminal: () => ({ term, fitAddon }),
    setupTmuxRightClickBridge: () => () => {},
    createPaneResizeObserver: (pane) => { pane._observed = true; return { disconnect() {} }; },
    fitAndResizePane: (pane) => { if (pane.spawned) fitAddon.fit(); },
  }, opts.overrides || {}));

  return {
    sandbox, handle, invoked, panes, term, fitAddon, panelEl, unlistens,
    emitExit: (paneId) => {
      const fn = listeners.get('pty-exit');
      if (fn) fn({ payload: { window_label: 'window-1', pane_id: paneId } });
    },
    exitEl: () => findByClass(panelEl, 'tl-terminal-panel__exit'),
    restartBtn: () => findByClass(panelEl, 'tl-terminal-panel__restart'),
    setActive: (node) => { sandbox.document.activeElement = node; },
  };
}

check('the panel spawns exactly one shell, at the project root, when it renders', async () => {
  const h = loadPanel();
  await tick();
  const spawns = h.invoked.filter((c) => c.cmd === 'spawn_shell');
  assert.strictEqual(spawns.length, 1, 'first show spawns once');
  assert.deepStrictEqual(JSON.parse(JSON.stringify(spawns[0].args)),
    { paneId: 7, cols: 120, rows: 30, cwd: '/proj' },
    'the same spawn_shell contract terminal tabs use, with the project root as cwd');
});

check('a window with no project spawns at the default cwd', async () => {
  const h = loadPanel({ overrides: { cwd: null } });
  await tick();
  const spawn = h.invoked.find((c) => c.cmd === 'spawn_shell');
  assert.strictEqual(spawn.args.cwd, null, 'null cwd means the backend picks the default');
});

check('the pane joins the shared pane map so output, theme and font reach it', async () => {
  const h = loadPanel();
  await tick();
  const pane = h.panes.get(7);
  assert.ok(pane, 'the panel registers its pane under its own id');
  assert.strictEqual(pane.kind, 'terminal');
  assert.strictEqual(pane.type, 'local');
  assert.strictEqual(pane.tabId, null,
    'no tab owns it — every tab-shaped teardown path is guarded on tabs.get(pane.tabId)');
  assert.strictEqual(pane.spawned, true);
  assert.strictEqual(pane._observed, true, 'a resize observer keeps xterm fitted to the zone');
});

check('keystrokes go to this pane\'s PTY, and only once it is spawned', async () => {
  const h = loadPanel();
  h.term._emitData('early');
  await tick();
  h.term._emitData('ls\r');
  await tick();
  const writes = h.invoked.filter((c) => c.cmd === 'write_to_pty');
  assert.strictEqual(writes.length, 1, 'input before the spawn resolves is dropped, as in tabs');
  assert.deepStrictEqual(JSON.parse(JSON.stringify(writes[0].args)), { paneId: 7, data: 'ls\r' });
});

check('hiding the panel does not touch the shell', async () => {
  const h = loadPanel();
  await tick();
  // Hiding is `tw.el.style.display = 'none'` in the manager — the panel is
  // never told, and must never have registered anything that would react.
  assert.deepStrictEqual(h.invoked.map((c) => c.cmd), ['spawn_shell'],
    'no close_pty, no second spawn: the shell keeps running behind a hidden panel');
  assert.strictEqual(h.panes.get(7).spawned, true);
});

check('pty-exit shows the exit state and offers a respawn', async () => {
  const h = loadPanel();
  await tick();
  assert.strictEqual(h.exitEl().hidden, true, 'a live shell shows no exit state');

  h.emitExit(7);
  assert.strictEqual(h.exitEl().hidden, false);
  assert.ok(/exited/i.test(findByClass(h.panelEl, 'tl-terminal-panel__exit-text').textContent),
    'the exit state says the shell exited');
  assert.strictEqual(h.panes.get(7).spawned, false, 'a dead pane must not accept input');

  h.restartBtn()._fire('click', { stopPropagation() {} });
  await tick();
  const spawns = h.invoked.filter((c) => c.cmd === 'spawn_shell');
  assert.strictEqual(spawns.length, 2, 'the restart spawns a new shell on the same pane id');
  assert.strictEqual(h.exitEl().hidden, true, 'and clears the exit state');
  assert.strictEqual(h.panes.get(7).spawned, true);
});

check('a pty-exit for another pane is ignored', async () => {
  const h = loadPanel();
  await tick();
  h.emitExit(99);
  assert.strictEqual(h.exitEl().hidden, true, 'only this panel\'s pane id may change its state');
});

check('a failed spawn lands in the same exit state rather than a silent blank panel', async () => {
  const h = loadPanel({ failSpawn: true });
  await tick();
  assert.strictEqual(h.exitEl().hidden, false);
  assert.strictEqual(h.panes.get(7).spawned, false);
});

check('destroy() releases the pane, the listener and the PTY', async () => {
  const h = loadPanel();
  await tick();
  h.handle.destroy();
  assert.strictEqual(h.panes.has(7), false, 'the shared pane map must not keep a dead pane');
  assert.deepStrictEqual(h.unlistens, ['pty-exit'], 'the event subscription is released');
  assert.ok(h.invoked.some((c) => c.cmd === 'close_pty' && c.args.paneId === 7),
    'the PTY is closed — dock/undock must not leak a shell');
  assert.strictEqual(h.term.disposed, 1);
});

check('the panel reports focus so the shortcut router can treat it as a terminal', async () => {
  const h = loadPanel();
  await tick();
  assert.strictEqual(h.sandbox.termlabTerminalPanel.hasFocus(), false);
  h.setActive(h.term.textarea);
  // The textarea is xterm's, mounted inside the panel's own surface in the
  // real DOM; the stub reproduces the containment the check relies on.
  const surface = findByClass(h.panelEl, 'tl-terminal-panel__surface');
  surface.appendChild(h.term.textarea);
  assert.strictEqual(h.sandbox.termlabTerminalPanel.hasFocus(), true);
});

check('a window with no terminal runtime gets a note, not a thrown renderFn', () => {
  const h = loadPanel({ overrides: { initTerminal: null, invoke: null } });
  assert.ok(findByClass(h.panelEl, 'tl-terminal-panel__unavailable'),
    'a panel host has no pane registry and no PTY — say so instead of throwing');
  assert.strictEqual(typeof h.handle.destroy, 'function');
});

// ===========================================================================
// Part 3 — registration, boot shape and wiring (source level)
// ===========================================================================

check('the runtime registers Terminal last among the bottom-zone built-ins', () => {
  const src = fs.readFileSync(RUNTIME_PATH, 'utf8');
  const terminal = src.indexOf("register('terminal'");
  assert.ok(terminal !== -1, 'the Terminal tool window is registered');
  for (const earlier of ["register('file-explorer'", "register('transfer-center'", "register('problems'", "register('project-search'"]) {
    assert.ok(src.indexOf(earlier) !== -1 && src.indexOf(earlier) < terminal,
      `${earlier} must register before Terminal, so Terminal never steals a default zone`);
  }
});

check('Terminal is registered in every window, not only project windows', () => {
  const src = fs.readFileSync(RUNTIME_PATH, 'utf8');
  const search = src.indexOf("register('project-search'");
  const terminal = src.indexOf("register('terminal'");
  const guard = src.slice(0, terminal).lastIndexOf('if (projectRoot)');
  assert.ok(guard < search,
    'the last projectRoot guard before the Terminal registration is Search\'s, '
    + 'so Terminal itself is unguarded — it exists (hidden) in plain windows too');
});

check('the Terminal registration declares its zone, its opt-outs and a cwd', () => {
  const src = fs.readFileSync(RUNTIME_PATH, 'utf8');
  const block = src.slice(src.indexOf("register('terminal'"), src.indexOf("register('notifications'"));
  assert.ok(block.includes("defaultZone: 'bottom-right'"),
    'the bottom zone is a left/right pair: the project tree keeps bottom-left, the shell takes bottom-right');
  assert.ok(block.includes('autoActivate: false'), 'a plain window must find it hidden');
  assert.ok(block.includes('poppable: false'), 'a PTY is bound to this window; a host cannot hold one');
  assert.ok(block.includes('cwd: projectRoot'), 'a project window opens its shell at the root');
});

check('a fresh project window opens the bottom zone on Terminal beside the tree', () => {
  const src = fs.readFileSync(RUNTIME_PATH, 'utf8');
  const gate = src.indexOf('!(initialLayoutData && initialLayoutData.has_project_layout)');
  assert.ok(gate !== -1, 'the Task 12 gate still decides fresh vs returning');
  const block = src.slice(gate, gate + 1600);
  assert.ok(block.includes("activate('file-explorer', { save: false })"),
    'the Files reveal is unchanged');
  assert.ok(block.includes("activate('terminal', { save: false })"),
    'and the Terminal comes up alongside it, in the other half of the bottom zone');
});

check('the two fresh-project activations really do compose in the manager', () => {
  const { twm, invoke } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('file-explorer', { title: 'Project', type: 'built-in', defaultZone: 'bottom', renderFn: () => {} });
  twm.register('problems', { title: 'Problems', type: 'built-in', defaultZone: 'bottom', renderFn: () => {} });
  twm.register('project-search', { title: 'Search', type: 'built-in', defaultZone: 'bottom', renderFn: () => {} });
  twm.register('terminal', {
    title: 'Terminal', type: 'built-in', defaultZone: 'bottom-right',
    autoActivate: false, poppable: false, renderFn: () => {},
  });

  twm.setPanelVisibility('bottom', true, { save: false });
  twm.activate('file-explorer', { save: false });
  twm.activate('terminal', { save: false });

  assert.strictEqual(twm.isVisible('file-explorer'), true, 'the project tree stays on screen');
  assert.strictEqual(twm.isVisible('terminal'), true, 'and the shell is up alongside it');
  assert.strictEqual(twm.isVisible('project-search'), false, 'Search is still not what a user finds open');
  const active = JSON.parse(JSON.stringify(twm.getActiveZoneAssignments()));
  assert.strictEqual(active['bottom-left'], 'file-explorer');
  assert.strictEqual(active['bottom-right'], 'terminal');
});

check('a project window no longer boots a terminal TAB', () => {
  const src = fs.readFileSync(MAIN_RUNTIME_PATH, 'utf8');
  assert.ok(!src.includes('createTab({ cwd: projectRoot })'),
    'the project terminal moved into the tool window; the main area boots empty');
  assert.ok(src.includes('createTab().catch'), 'a plain window still opens its first terminal tab');
  const pull = src.indexOf('take_pending_open_paths');
  assert.ok(pull !== -1 && pull < src.indexOf('createTab().catch'),
    'the CLI queue pull still precedes any tab creation');
  const projectBranch = src.indexOf('} else if (projectRoot) {');
  assert.ok(projectBranch !== -1, 'the project branch is still there — it just creates nothing now');
});

check('index.html carries the panel, its stylesheet and the placeholder', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  assert.ok(html.includes('app/panels/terminal-panel.js'), 'the panel script is loaded');
  assert.ok(html.indexOf('app/panels/terminal-panel.js') < html.indexOf('app/tool-window-runtime.js'),
    'it must be defined before the runtime that registers it');
  assert.ok(html.includes('styles/design-system/components/terminal-panel.css'));
  assert.ok(/id="editor-placeholder"/.test(html), 'the empty main area has a placeholder element');
  assert.ok(html.indexOf('id="terminal-host"') < html.indexOf('id="editor-placeholder"')
    || /id="terminal-host"[\s\S]{0,400}id="editor-placeholder"/.test(html),
    'the placeholder lives inside the terminal host');
});

check('the shortcut router treats the panel terminal as a terminal context', () => {
  const src = fs.readFileSync(SHORTCUTS_PATH, 'utf8');
  assert.ok(src.includes('termlabTerminalPanel'),
    'cmd+s in the Terminal tool window must reach the shell, not the editor');
  const guard = src.indexOf('EDITOR_SCOPED_ACTIONS.indexOf(coreHit.action)');
  assert.ok(guard !== -1);
  assert.ok(src.indexOf('termlabTerminalPanel') < guard
    || /terminalToolWindowFocused\(\)/.test(src.slice(guard, guard + 400)),
    'the scope check is consulted by the editor-scoped drop');
});

check('the terminal panel css styles every class it renders, with tokens only', () => {
  const css = fs.readFileSync(CSS_PATH, 'utf8');
  for (const name of [
    'tl-terminal-panel', 'tl-terminal-panel__surface', 'tl-terminal-panel__exit',
    'tl-terminal-panel__restart', 'tl-terminal-panel__unavailable',
  ]) {
    assert.ok(css.includes(`.${name}`), `${name} is rendered but never styled`);
  }
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(css), 'design-system components use tokens only');
});

check('the new modules use no regex lookbehind and no control bytes', () => {
  for (const file of [PANEL_PATH, CSS_PATH]) {
    const source = fs.readFileSync(file, 'utf8');
    assert.ok(!/\(\?<[=!]/.test(source), `${file} uses a lookbehind`);
    const bytes = fs.readFileSync(file);
    for (let i = 0; i < bytes.length; i += 1) {
      assert.ok(bytes[i] >= 0x20 || bytes[i] === 0x0a || bytes[i] === 0x09,
        `${file}: control byte at offset ${i} — git treats the file as binary`);
    }
  }
});

check('tool-window-manager still binds no bare `global`', () => {
  const src = fs.readFileSync(MANAGER_PATH, 'utf8');
  assert.ok(!/\bglobal\s*\./.test(src),
    'tool-window-manager.js must use window.*, it has no `global` binding');
});

// ===========================================================================
// Part 4 — the project-aware zero-tabs fallback and the placeholder
// ===========================================================================

function loadTabManager(options) {
  const opts = options || {};
  const placeholderEl = makeElement('div');
  placeholderEl.id = 'editor-placeholder';
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    setInterval: () => 0,
    clearInterval: () => {},
    requestAnimationFrame: (cb) => cb(),
    Promise,
    Math,
    Object,
    Array,
    Map,
    Set,
    JSON,
    String,
    Number,
    Error,
  };
  sandbox.window = sandbox;
  sandbox.document = {
    createElement: (tag) => makeElement(tag),
    getElementById: (id) => (id === 'editor-placeholder' ? placeholderEl : null),
    addEventListener() {},
  };
  sandbox.dispatchEvent = () => true;
  sandbox.CustomEvent = class { constructor(type, init) { this.type = type; Object.assign(this, init); } };
  sandbox.splitTree = { makeLeaf: (paneId) => ({ type: 'leaf', paneId }) };
  sandbox.splitPane = {
    setupDividerDrag: () => {},
    createPaneResizeObserver: () => ({ disconnect() {} }),
  };
  if (opts.projectRoot) {
    sandbox.termlabProjectMode = {
      isActive: () => true,
      root: () => opts.projectRoot,
    };
  }
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(TAB_MANAGER_PATH, 'utf8'), sandbox, { filename: TAB_MANAGER_PATH });

  const tabs = new Map();
  const panes = new Map();
  let destroyedWindow = 0;
  let spawnedShells = 0;
  let activeTabId = 1;
  let nextId = 50;

  const fakeTerm = () => ({
    onTitleChange: () => {}, onData: () => {}, dispose: () => {}, focus: () => {}, options: {},
  });

  const manager = sandbox.termlabTabManager.create({
    getTabs: () => tabs,
    getPanes: () => panes,
    getActiveTabId: () => activeTabId,
    setActiveTabId: (id) => { activeTabId = id; },
    getFocusedPaneId: () => null,
    setFocusedPaneId: () => {},
    setNextTabLabel: () => {},
    appEl: makeElement('div'),
    getTermFontSize: () => 13,
    setFocusedPane: () => {},
    fitAndResizeTab: () => {},
    fitAndResizePane: () => {},
    onTabChanged: () => {},
    allPanesInTab: (tabId) => Array.from(panes.values())
      .filter((p) => p.tabId === tabId).map((p) => p.paneId),
    rememberPluginViewSize: () => {},
    unregisterPaneDnd: () => {},
    notifyTerminalClosed: () => {},
    notifyPluginViewClosed: () => {},
    deletePluginViewPane: () => {},
    showStatus: () => {},
    destroyCurrentWindow: async () => { destroyedWindow += 1; },
    allocateTabId: () => nextId++,
    allocatePaneId: () => nextId++,
    allocateTabLabel: () => 'Terminal',
    tabBarEl: makeElement('div'),
    terminalHostEl: makeElement('div'),
    setWindowTitle: async () => {},
    getLocalPaneCwd: async () => null,
    getLocalPaneProcess: async () => null,
    getHostIdentity: async () => null,
    getWorkspaceDir: async () => null,
    refreshSshSessions: () => {},
    getCurrentWindowLabel: () => 'window-1',
    initTerminal: () => ({
      term: fakeTerm(),
      fitAddon: { proposeDimensions: () => ({ cols: 80, rows: 24 }), fit: () => {} },
    }),
    setupTmuxRightClickBridge: () => () => {},
    createPaneResizeObserver: () => ({ disconnect() {} }),
    makeLeaf: (paneId) => ({ type: 'leaf', paneId }),
    setupDividerDrag: () => {},
    normalizeTabTitle: (raw, fallback) => raw || fallback,
    spawnShell: async () => { spawnedShells += 1; },
    spawnDefaultShell: async () => { spawnedShells += 1; },
    onTerminalData: () => {},
  });

  function addEditorTab(tabId) {
    tabs.set(tabId, {
      id: tabId, label: 'notes.md', button: makeElement('button'),
      containerEl: makeElement('div'), focusedPaneId: tabId * 10,
    });
    panes.set(tabId * 10, {
      paneId: tabId * 10, tabId, kind: 'editor', filePath: '/p/notes.md', dirty: false, view: null,
    });
  }

  return {
    sandbox, manager, tabs, panes, addEditorTab, placeholderEl,
    counts: () => ({ destroyedWindow, spawnedShells }),
  };
}

check('a project window survives closing its last tab and shows the placeholder', async () => {
  const h = loadTabManager({ projectRoot: '/proj' });
  h.addEditorTab(1);
  await h.manager.closeTab(1);
  assert.strictEqual(h.counts().destroyedWindow, 0,
    'a project window boots with zero tabs — zero tabs must never close it');
  assert.strictEqual(h.counts().spawnedShells, 0,
    'and it must not spawn a terminal tab either: the shell lives in the tool window now');
  assert.strictEqual(h.tabs.size, 0);
  assert.strictEqual(h.placeholderEl.hidden, false, 'the empty main area explains itself');
});

check('a plain window still closes on its last tab', async () => {
  const h = loadTabManager();
  h.addEditorTab(1);
  await h.manager.closeTab(1);
  assert.strictEqual(h.counts().destroyedWindow, 1, 'unchanged for non-project windows');
});

check('the CLI editor-window fallback still wins in a plain window', async () => {
  const h = loadTabManager();
  h.sandbox.__termlabEditorWindow = true;
  h.addEditorTab(1);
  await h.manager.closeTab(1);
  assert.strictEqual(h.counts().spawnedShells, 1, 'termlab notes.md still falls back to a terminal tab');
  assert.strictEqual(h.counts().destroyedWindow, 0);
});

check('window teardown never revives a project window or a placeholder tab', async () => {
  const h = loadTabManager({ projectRoot: '/proj' });
  h.addEditorTab(1);
  await h.manager.closeTab(1, { closeWindowWhenLast: false });
  assert.strictEqual(h.counts().spawnedShells, 0);
  assert.strictEqual(h.counts().destroyedWindow, 0);
});

check('the placeholder hides as soon as a tab exists', async () => {
  const h = loadTabManager({ projectRoot: '/proj' });
  await h.manager.createTab();
  assert.strictEqual(h.placeholderEl.hidden, true, 'a window with tabs shows its tabs, not the hint');
  const id = h.tabs.keys().next().value;
  await h.manager.closeTab(id);
  assert.strictEqual(h.placeholderEl.hidden, false, 'and it comes back when the last one goes');
});

for (const { name, fn } of queued) {
  ran += 1;
  try {
    await fn();
  } catch (error) {
    failures += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${(error && error.stack) || error}`);
  }
}
if (failures) {
  console.log(`terminal tool window: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`terminal tool window: all ${ran} checks passed`);
}
