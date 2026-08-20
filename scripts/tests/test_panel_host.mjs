// Run: node scripts/tests/test_panel_host.mjs
//
// Pop-out tool windows, manager half (Task 3): the View Mode trait every
// registered tool window carries, the rail-toggle routing that trait adds,
// the dock-back remount, and the persistence/restore round trip.
//
// Two halves, both driven with the vm-harness idiom this repo already uses
// for DOM-touching frontend modules (test_tool_window_closed_state.mjs stubs
// the same document surface for tool-window-manager.js; test_settings_
// terminal_theme_picker.mjs is the richer element stub this borrows the
// listener plumbing from — there is no jsdom in this repo):
//
//   Part 1 — app/layout/tool-window-manager.js, loaded for real. Covers the
//   trait surface (menu entries for EVERY registered id, including a
//   synthetic plugin registration), the Window selection, the toggle matrix
//   (with the legacy dock path pinned), the docked/aborted events, and
//   restore seeding.
//
//   Part 2 — app/tool-window-runtime.js, loaded for real against a FAKE
//   toolWindowManager, so the wiring is checked where it lives: the save
//   payload's `tool_window_view_modes` key, the read-back seeding
//   (`setPersistedViewModes` before any register()), the four panel-host
//   event subscriptions, and the post-registration summon.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const FRONTEND = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app',
);
const MANAGER_PATH = path.join(FRONTEND, 'layout/tool-window-manager.js');
const RUNTIME_PATH = path.join(FRONTEND, 'tool-window-runtime.js');

// --- shared element stub ---------------------------------------------------

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
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    removeEventListener() {},
    setAttribute(n, v) { attrs.set(n, String(v)); },
    getAttribute(n) { return attrs.has(n) ? attrs.get(n) : null; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }; },
  };
  return el;
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

// Values that cross the vm-realm boundary carry the sandbox's intrinsics, so
// assert.deepStrictEqual's prototype check would reject an otherwise identical
// object/array. Compare their plain data instead.
const plain = (v) => JSON.parse(JSON.stringify(v));

// ===========================================================================
// Part 1 — tool-window-manager.js
// ===========================================================================

// A real zone element for `zoneName`, so ensureWindowElement() has somewhere
// to mount and "did the panel detach from its zone?" is observable.
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

function loadManager(opts) {
  const options = opts || {};
  const body = makeElement('body');
  const zoneEls = new Map();
  for (const z of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom']) {
    zoneEls.set(z, makeZoneEl(z));
  }

  const menuOpens = [];
  const invokeCalls = [];
  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    const handler = options.invokeHandlers && options.invokeHandlers[cmd];
    if (typeof handler === 'function') return handler(args);
    return Promise.resolve(undefined);
  };

  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    Promise,
    Math,
    document: {
      body,
      createElement: (t) => makeElement(t),
      getElementById: () => null,
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
  sandbox.global = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MANAGER_PATH, 'utf8'), sandbox, { filename: MANAGER_PATH });

  const twm = sandbox.toolWindowManager;
  assert.ok(twm, 'tool-window-manager.js must expose window.toolWindowManager');
  return { twm, sandbox, zoneEls, menuOpens, invokeCalls, invoke };
}

function viewModeItems(items) {
  return Array.from(items).filter((i) => typeof i.label === 'string' && i.label.startsWith('View Mode:'));
}

const HOSTS = { title: 'Hosts', icon: 'web', type: 'built-in', defaultZone: 'right-top' };
const TUNNELS = { title: 'Tunnels', type: 'built-in', defaultZone: 'right-bottom' };

// --- 1. The trait: EVERY registered id carries both View Mode entries -------
{
  const { twm, invoke, menuOpens } = loadManager();
  const saves = [];
  twm.init({ fitActiveTab: () => {}, saveLayout: () => saves.push(1), invoke });

  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => {} });
  twm.register('file-explorer', { title: 'SFTP', type: 'built-in', defaultZone: 'bottom', renderFn: () => {} });
  // The trait test: a synthetic plugin registration with nothing but a
  // renderFn gets the same menu the built-ins get. Nothing in the manager
  // may special-case built-in ids.
  twm.register('plugin:fake', { renderFn: () => {} });

  for (const id of ['ssh-sessions', 'file-explorer', 'plugin:fake']) {
    const items = twm.buildContextMenuItems(id);
    const modes = viewModeItems(items);
    assert.deepStrictEqual(
      modes.map((i) => i.label),
      ['View Mode: Dock', 'View Mode: Window'],
      `${id} must carry BOTH flattened View Mode entries (tl-menu has no submenus)`,
    );
    assert.strictEqual(modes[0].checked, true, `${id} starts docked, so Dock is the checked entry`);
    assert.strictEqual(modes[1].checked, false, `${id} is not in window mode yet`);
    for (const item of modes) {
      assert.ok(!Object.prototype.hasOwnProperty.call(item, 'items'),
        'no submenu payload may be introduced — tl-menu has no submenu support');
    }
  }

  // The entries must reach tlMenu, not just the pure builder.
  twm.showContextMenu(10, 20, 'plugin:fake');
  assert.strictEqual(menuOpens.length, 1, 'showContextMenu must route through window.tlMenu.open');
  assert.deepStrictEqual(
    viewModeItems(menuOpens[0].items).map((i) => i.label),
    ['View Mode: Dock', 'View Mode: Window'],
  );
  assert.strictEqual(menuOpens[0].x, 10);
  assert.strictEqual(menuOpens[0].y, 20);
}

// --- 2. Selecting Window opens a host and detaches from the zone -----------
{
  const { twm, invoke, invokeCalls, zoneEls } = loadManager();
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => { renders += 1; } });

  const content = zoneEls.get('right-top')._contentEl;
  assert.strictEqual(renders, 1, 'registering into an empty visible zone renders once, as before');
  assert.strictEqual(content.children.length, 1, 'the docked panel element is mounted in its zone');

  const windowItem = viewModeItems(twm.buildContextMenuItems('ssh-sessions'))[1];
  windowItem.onSelect();

  assert.deepStrictEqual(
    plain(invokeCalls),
    [{ cmd: 'open_panel_host', args: { toolWindowId: 'ssh-sessions', title: 'Hosts' } }],
    'selecting Window must invoke open_panel_host with the id AND the title',
  );
  assert.strictEqual(twm.getViewModes()['ssh-sessions'], 'window');
  assert.strictEqual(content.children.length, 0, 'the panel element must be DETACHED from its zone');
  assert.strictEqual(twm.getContentElement('ssh-sessions'), null,
    'renderFn is render-once: the manager drops its element so the HOST renders fresh');
  assert.strictEqual(renders, 1, 'the manager must not re-render on the way out — the host does that');
  assert.strictEqual(twm.getZoneForWindow('ssh-sessions'), 'right-top',
    'the zone assignment is REMEMBERED while popped out');
  assert.strictEqual(twm.getActiveZoneAssignments()['right-top'], 'ssh-sessions',
    'a visible popped-out window still records itself as its zone\'s open window, '
    + 'so restore knows to summon it');
}

// --- 3. getRegistration() hands Task 4 everything it needs to mount --------
{
  const { twm, invoke } = loadManager();
  const renderFn = () => {};
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('ssh-sessions', { ...HOSTS, renderFn });

  const registration = twm.getRegistration('ssh-sessions');
  assert.deepStrictEqual(plain(registration), {
    id: 'ssh-sessions',
    title: 'Hosts',
    icon: 'web',
    type: 'built-in',
  });
  assert.strictEqual(registration.renderFn, renderFn,
    'the host mounts through the SAME renderFn the manager registered');
  assert.strictEqual(twm.getRegistration('nope'), null);
}

// --- 4. Toggle matrix: window + visible -> hide ----------------------------
{
  const { twm, invoke, invokeCalls } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => {} });
  viewModeItems(twm.buildContextMenuItems('ssh-sessions'))[1].onSelect();
  invokeCalls.length = 0;

  twm.toggle('ssh-sessions'); // host is visible (open just succeeded)
  assert.deepStrictEqual(plain(invokeCalls), [
    { cmd: 'hide_panel_host', args: { toolWindowId: 'ssh-sessions' } },
  ]);
  assert.strictEqual(twm.getViewModes()['ssh-sessions'], 'window', 'hiding is not un-popping');
  assert.strictEqual(twm.getActiveZoneAssignments()['right-top'], '',
    'a hidden host must not persist as its zone\'s open window');
}

// --- 5. Toggle matrix: window + hidden -> focus (no open when it works) ----
{
  const { twm, invoke, invokeCalls } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => {} });
  viewModeItems(twm.buildContextMenuItems('ssh-sessions'))[1].onSelect();
  twm.toggle('ssh-sessions'); // -> hidden
  invokeCalls.length = 0;

  twm.toggle('ssh-sessions'); // -> summon
  await tick();
  assert.deepStrictEqual(plain(invokeCalls), [
    { cmd: 'focus_panel_host', args: { toolWindowId: 'ssh-sessions' } },
  ], 'a live host is summoned with focus_panel_host — no second window is built');
}

// --- 6. Toggle matrix: focus Err -> open_panel_host fallback ---------------
{
  const { twm, invoke, invokeCalls } = loadManager({
    invokeHandlers: {
      // What Rust returns after an app relaunch: the mode survived in the
      // saved layout, but no host window exists in this session.
      focus_panel_host: () => Promise.reject(new Error('no host')),
    },
  });
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => {} });
  twm.setViewMode('ssh-sessions', 'window');
  twm.toggle('ssh-sessions'); // -> hidden
  invokeCalls.length = 0;

  twm.toggle('ssh-sessions');
  await tick();
  assert.deepStrictEqual(invokeCalls.map((c) => c.cmd), ['focus_panel_host', 'open_panel_host']);
  assert.deepStrictEqual(plain(invokeCalls[1].args), { toolWindowId: 'ssh-sessions', title: 'Hosts' });
  assert.strictEqual(twm.getViewModes()['ssh-sessions'], 'window');
}

// --- 7. Toggle matrix: the DOCK path is untouched --------------------------
// Pinned by behaviour, not by re-implementation: the legacy branches are
// (a) active + panel hidden -> reveal the panel, keep the window active,
// (b) active + panel visible -> deactivate, (c) inactive -> activate.
// Plus the hard pin that no panel-host command is reachable from dock mode.
{
  const { twm, invoke, invokeCalls } = loadManager();
  const saves = [];
  twm.init({ fitActiveTab: () => {}, saveLayout: () => saves.push(1), invoke });
  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => {} });

  assert.strictEqual(twm.isVisible('ssh-sessions'), true);

  // (b) active + panel visible -> deactivate (the legacy deactivate() path:
  // zone.activeId cleared, recorded as the zone's closed state, save fired)
  saves.length = 0;
  twm.toggle('ssh-sessions');
  assert.strictEqual(twm.isVisible('ssh-sessions'), false);
  assert.strictEqual(twm.getActiveZoneAssignments()['right-top'], '');
  assert.strictEqual(saves.length, 1, 'deactivate() still triggers exactly one layout save');

  // (c) inactive -> activate (the legacy activate() path)
  saves.length = 0;
  twm.toggle('ssh-sessions');
  assert.strictEqual(twm.isVisible('ssh-sessions'), true);
  assert.strictEqual(twm.getActiveZoneAssignments()['right-top'], 'ssh-sessions');
  assert.strictEqual(saves.length, 1, 'activate() still triggers exactly one layout save');

  // (a) active + panel hidden -> reveal the panel, window stays active
  twm.setPanelVisibility('right', false, { save: false });
  twm.toggle('ssh-sessions');
  assert.strictEqual(twm.isPanelVisible('right'), true);
  assert.strictEqual(twm.isVisible('ssh-sessions'), true,
    'revealing a hidden panel must not also toggle the window off');

  assert.deepStrictEqual(plain(invokeCalls), [],
    'a docked tool window must never reach a panel-host command');
}

// --- 8. panel-host-docked remounts into the REMEMBERED zone ----------------
{
  const { twm, invoke, invokeCalls, zoneEls } = loadManager();
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { renders += 1; } });
  // Move it first, so "remembered zone" means something other than the
  // default one it registered into.
  twm.moveTo('tunnels', 'left-bottom');
  renders = 0;
  twm.setViewMode('tunnels', 'window');
  assert.strictEqual(zoneEls.get('left-bottom')._contentEl.children.length, 0);
  invokeCalls.length = 0;

  twm.notifyHostDocked('tunnels');

  assert.strictEqual(twm.getViewModes().tunnels, 'dock');
  assert.strictEqual(twm.getZoneForWindow('tunnels'), 'left-bottom',
    'dock-back must land in the zone the window was popped out of');
  assert.strictEqual(twm.getActiveZoneAssignments()['left-bottom'], 'tunnels');
  assert.strictEqual(renders, 1, 'the remount is a FRESH renderFn call (render-once, new element)');
  assert.strictEqual(zoneEls.get('left-bottom')._contentEl.children.length, 1);
  assert.deepStrictEqual(plain(invokeCalls), [],
    'the host destroys itself on dock — the parent must not command it');
}

// --- 9. Choosing "View Mode: Dock" from the menu hides the live host -------
{
  const { twm, invoke, invokeCalls, zoneEls } = loadManager();
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { renders += 1; } });
  twm.setViewMode('tunnels', 'window');
  renders = 0;
  invokeCalls.length = 0;

  viewModeItems(twm.buildContextMenuItems('tunnels'))[0].onSelect();
  assert.deepStrictEqual(plain(invokeCalls), [
    { cmd: 'hide_panel_host', args: { toolWindowId: 'tunnels' } },
  ], 'the parent can only hide its host; dock_panel_host is host-only');
  assert.strictEqual(twm.getViewModes().tunnels, 'dock');
  assert.strictEqual(renders, 1);
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 1);
}

// --- 10. panel-host-aborted resets the mode without remounting ------------
{
  const { twm, invoke, zoneEls } = loadManager();
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { renders += 1; } });
  twm.setViewMode('tunnels', 'window');
  renders = 0;

  twm.notifyHostAborted('tunnels');

  assert.strictEqual(twm.getViewModes().tunnels, 'dock',
    'a host that self-aborted never had a panel — reset the id to dock');
  assert.strictEqual(twm.isVisible('tunnels'), false, 'the rail state is cleared, not lit');
  assert.strictEqual(twm.getActiveZoneAssignments()['right-bottom'], '');
  assert.strictEqual(renders, 0, 'an abort has nothing to remount');
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 0);
}

// --- 11. shown/hidden events sync the rail lit-state -----------------------
{
  const { twm, invoke } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => {} });
  twm.setViewMode('ssh-sessions', 'window');

  twm.notifyHostHidden('ssh-sessions');
  assert.strictEqual(twm.isVisible('ssh-sessions'), false);
  assert.strictEqual(twm.getActiveZoneAssignments()['right-top'], '');

  twm.notifyHostShown('ssh-sessions');
  assert.strictEqual(twm.isVisible('ssh-sessions'), true);
  assert.strictEqual(twm.getActiveZoneAssignments()['right-top'], 'ssh-sessions');
}

// --- 12. Restore: seed modes, summon ONLY the previously-open ones ---------
{
  // After a relaunch the mode survived in the saved layout but no host window
  // did, so the summon's focus_panel_host answers Err and falls back to open.
  const { twm, invoke, invokeCalls, zoneEls } = loadManager({
    invokeHandlers: { focus_panel_host: () => Promise.reject(new Error('no host')) },
  });
  let hostRenders = 0;
  let tunnelRenders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });

  twm.setPersistedZones({ 'ssh-sessions': 'right-top', tunnels: 'right-bottom' });
  twm.setPersistedViewModes({ 'ssh-sessions': 'window', tunnels: 'window' });
  // Only ssh-sessions was open when the layout was saved.
  twm.setPersistedActiveZoneWindows({ 'right-top': 'ssh-sessions', 'right-bottom': '' });
  twm.setPersistedPanelVisibility({ left: true, right: true, bottom: true });

  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => { hostRenders += 1; } });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { tunnelRenders += 1; } });

  assert.strictEqual(hostRenders, 0, 'a window-mode tool window must NOT render docked on boot');
  assert.strictEqual(tunnelRenders, 0);
  assert.strictEqual(zoneEls.get('right-top')._contentEl.children.length, 0);
  assert.deepStrictEqual(plain(twm.getViewModes()), { 'ssh-sessions': 'window', tunnels: 'window' });
  assert.deepStrictEqual(plain(invokeCalls), [], 'nothing is summoned until registrations finish');

  twm.summonPendingWindowHosts();
  await tick();

  assert.deepStrictEqual(plain(invokeCalls), [
    { cmd: 'focus_panel_host', args: { toolWindowId: 'ssh-sessions' } },
    { cmd: 'open_panel_host', args: { toolWindowId: 'ssh-sessions', title: 'Hosts' } },
  ], 'only the previously-open window-mode id is summoned, via focus then the '
    + 'open fallback (no host exists after a relaunch)');
  assert.strictEqual(twm.getViewModes().tunnels, 'window',
    'the one that was closed stays in window mode and waits to be summoned');
}

// --- 13. Restore: a plugin registering after the summon pass still opens ---
{
  const { twm, invoke, invokeCalls } = loadManager({
    invokeHandlers: { focus_panel_host: () => Promise.resolve() },
  });
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.setPersistedZones({ 'plugin:fake': 'bottom' });
  twm.setPersistedViewModes({ 'plugin:fake': 'window' });
  twm.setPersistedActiveZoneWindows({ bottom: 'plugin:fake' });

  twm.summonPendingWindowHosts(); // nothing registered yet
  assert.deepStrictEqual(plain(invokeCalls), []);

  // Plugin tool windows register asynchronously, well after the built-ins.
  twm.register('plugin:fake', { title: 'Fake', renderFn: () => {} });
  await tick();
  assert.deepStrictEqual(invokeCalls.map((c) => c.cmd), ['focus_panel_host']);
}

// --- 14. Dock-mode persistence is unchanged for everyone else --------------
{
  const { twm, invoke } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => {} });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => {} });

  assert.deepStrictEqual(plain(twm.getViewModes()), { 'ssh-sessions': 'dock', tunnels: 'dock' },
    'getViewModes() mirrors getZoneAssignments(): one entry per registered id');
  assert.deepStrictEqual(plain(twm.getZoneAssignments()), {
    'ssh-sessions': 'right-top',
    tunnels: 'right-bottom',
  });
}

// ===========================================================================
// Part 2 — tool-window-runtime.js wiring (fake toolWindowManager)
// ===========================================================================

function makeFakeManager() {
  const calls = [];
  const record = (name) => (...args) => { calls.push({ name, args }); };
  return {
    calls,
    viewModes: { 'ssh-sessions': 'window' },
    init: record('init'),
    setPersistedZones: record('setPersistedZones'),
    setPersistedActiveZoneWindows: record('setPersistedActiveZoneWindows'),
    setPersistedPanelVisibility: record('setPersistedPanelVisibility'),
    setPersistedViewModes: record('setPersistedViewModes'),
    setSidebarWidth: record('setSidebarWidth'),
    setSplitRatio: record('setSplitRatio'),
    setPanelVisibility: record('setPanelVisibility'),
    register(id, opts) { calls.push({ name: 'register', args: [id, opts] }); },
    summonPendingWindowHosts: record('summonPendingWindowHosts'),
    notifyHostShown: record('notifyHostShown'),
    notifyHostHidden: record('notifyHostHidden'),
    notifyHostDocked: record('notifyHostDocked'),
    notifyHostAborted: record('notifyHostAborted'),
    getSidebarWidths: () => ({ left: 240, right: 300 }),
    isPanelOpen: () => true,
    isPanelVisible: () => true,
    getZoneAssignments: () => ({ 'ssh-sessions': 'right-top' }),
    getActiveZoneAssignments: () => ({ 'right-top': 'ssh-sessions' }),
    getViewModes() { return this.viewModes; },
    getSplitRatios: () => ({ left: 0.5, right: 0.5 }),
  };
}

async function loadRuntime(savedLayout) {
  const twm = makeFakeManager();
  const invokeCalls = [];
  const listeners = new Map();
  const elements = new Map([
    ['bottom-zone-wrap', makeElement('div')],
    ['bottom-zone-resize', makeElement('div')],
  ]);

  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    Promise,
    Math,
    Number,
    document: {
      body: makeElement('body'),
      createElement: (t) => makeElement(t),
      getElementById: (id) => elements.get(id) || null,
      querySelector: () => null,
      querySelectorAll: () => [],
      addEventListener() {},
    },
    addEventListener() {},
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  sandbox.toolWindowManager = twm;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(RUNTIME_PATH, 'utf8'), sandbox, { filename: RUNTIME_PATH });

  const runtime = sandbox.termlabToolWindowRuntime.create({
    invoke: (cmd, args) => { invokeCalls.push({ cmd, args }); return Promise.resolve(undefined); },
    listen: () => Promise.resolve(() => {}),
    listenOnCurrentWindow: (name, fn) => {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
      return Promise.resolve(() => {});
    },
    layoutService: {
      getSavedLayout: () => Promise.resolve(savedLayout || {}),
      saveLayout: (layout) => { invokeCalls.push({ cmd: 'save_window_layout', args: { layout } }); },
    },
    debouncedFitAndResize: () => {},
    getCurrentTab: () => null,
    getCurrentPane: () => null,
  });

  const result = await runtime.init();
  const emit = (name, payload) => {
    for (const fn of listeners.get(name) || []) fn({ payload });
  };
  return { twm, invokeCalls, listeners, emit, result };
}

// --- 15. The save payload carries the view modes ---------------------------
{
  const { twm, invokeCalls, result } = await loadRuntime({});
  invokeCalls.length = 0;
  result.debouncedSaveLayout();

  const save = invokeCalls.find((c) => c.cmd === 'save_window_layout');
  assert.ok(save, 'saving must go through the layout service');
  assert.deepStrictEqual(plain(save.args.layout.tool_window_view_modes), twm.viewModes,
    'the layout payload must carry tool_window_view_modes alongside tool_window_zones');
  assert.deepStrictEqual(plain(save.args.layout.tool_window_zones), { 'ssh-sessions': 'right-top' },
    'the neighbouring keys are untouched');
}

// --- 16. Restore: the read-back seeds the manager BEFORE any register() ----
{
  const { twm } = await loadRuntime({
    tool_window_zones: { 'ssh-sessions': 'right-top' },
    tool_window_view_modes: { 'ssh-sessions': 'window' },
    active_tool_windows: { 'right-top': 'ssh-sessions' },
  });

  const seedIdx = twm.calls.findIndex((c) => c.name === 'setPersistedViewModes');
  const firstRegisterIdx = twm.calls.findIndex((c) => c.name === 'register');
  const summonIdx = twm.calls.findIndex((c) => c.name === 'summonPendingWindowHosts');

  assert.ok(seedIdx >= 0, 'the read-back must be handed to setPersistedViewModes');
  assert.deepStrictEqual(plain(twm.calls[seedIdx].args[0]), { 'ssh-sessions': 'window' });
  assert.ok(seedIdx < firstRegisterIdx, 'modes must be seeded BEFORE the first register()');
  assert.ok(summonIdx > firstRegisterIdx, 'summoning happens AFTER registrations complete');
  const lastRegisterIdx = twm.calls.map((c) => c.name).lastIndexOf('register');
  assert.ok(summonIdx > lastRegisterIdx, 'summoning happens after the LAST registration');
}

// --- 17. The four panel-host events are wired to the manager --------------
{
  const { twm, emit } = await loadRuntime({});
  emit('panel-host-shown', { toolWindowId: 'a' });
  emit('panel-host-hidden', { toolWindowId: 'b' });
  emit('panel-host-docked', { toolWindowId: 'c' });
  emit('panel-host-aborted', { toolWindowId: 'd' });

  const named = (name) => twm.calls.filter((c) => c.name === name).map((c) => c.args[0]);
  assert.deepStrictEqual(named('notifyHostShown'), ['a']);
  assert.deepStrictEqual(named('notifyHostHidden'), ['b']);
  assert.deepStrictEqual(named('notifyHostDocked'), ['c']);
  assert.deepStrictEqual(named('notifyHostAborted'), ['d']);
}

console.log('panel host (manager half): all assertions passed');
