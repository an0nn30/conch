// Run: node scripts/tests/test_panel_host.mjs
//
// Pop-out tool windows, manager half (Task 3): the View Mode trait every
// registered tool window carries, the rail-toggle routing that trait adds,
// the dock-back remount, and the persistence/restore round trip.
//
// Four parts, all driven with the vm-harness idiom this repo already uses
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
const MAIN_RUNTIME_PATH = path.join(FRONTEND, 'main-runtime.js');
const HOST_RUNTIME_PATH = path.join(FRONTEND, 'panel-host-runtime.js');
const BRIDGE_PATH = path.join(FRONTEND, 'core/panel-host-bridge.js');
const MANAGER_COMPOSE_PATH = path.join(FRONTEND, 'manager-compose-runtime.js');
const FILES_PANEL_PATH = path.join(FRONTEND, 'panels/files-panel.js');
const FILES_DATA_SERVICE_PATH = path.join(FRONTEND, 'features/files/data-service.js');
const FILES_PANE_STORE_PATH = path.join(FRONTEND, 'features/files/pane-store.js');
const FILES_ACTIONS_PATH = path.join(FRONTEND, 'features/files/actions.js');

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
    remove() {
      if (this.parentNode) this.parentNode.removeChild(this);
    },
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    // Test-only: fire the handlers registered for `name`. There is no real
    // event system behind this stub, so a click has to be delivered by hand.
    _fire(name, event) {
      for (const fn of listeners.get(name) || []) fn(event || {});
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
  for (const z of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right']) {
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
    [{ cmd: 'open_panel_host', args: { toolWindowId: 'ssh-sessions', title: 'Hosts', companionIds: [] } }],
    'selecting Window must invoke open_panel_host with the id AND the title, and companionIds '
    + '(empty here — this registration carries no companions)',
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
  assert.deepStrictEqual(plain(invokeCalls[1].args), { toolWindowId: 'ssh-sessions', title: 'Hosts', companionIds: [] });
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

// --- 9. Choosing "View Mode: Dock" DESTROYS the live host ------------------
// Not a hide: the panel is mounted and stateful inside the host, so hiding
// would leave two live instances of one panel (the hidden host's, plus the one
// remounted below). dock_panel_host now takes a parent caller naming its own
// id — see src/panel_host.rs's resolve_dock_target.
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
    { cmd: 'dock_panel_host', args: { toolWindowId: 'tunnels' } },
  ], 'the parent must destroy its host, not hide it');
  assert.strictEqual(twm.getViewModes().tunnels, 'dock');
  assert.strictEqual(renders, 1);
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 1);

  // Rust answers the destroy by emitting panel-host-docked back to this
  // window. The mode is already 'dock', so notifyHostDocked's guard makes the
  // echo inert — no second remount, no second render.
  invokeCalls.length = 0;
  twm.notifyHostDocked('tunnels');
  assert.strictEqual(renders, 1, 'the docked echo must not remount a second time');
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 1);
  assert.deepStrictEqual(plain(invokeCalls), []);
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

// --- 10b. A LATE abort, arriving after the window is docked again, is inert -
// The reachable race: the parent picks Dock while the host is still booting,
// the host's boot then aborts, and the event lands on a window that is
// mounted and active again. Without the same mode guard its three sibling
// handlers carry, the reset would clear the active flag underneath a visible
// panel — a dark rail button over a showing panel.
{
  const { twm, invoke, zoneEls } = loadManager();
  let renders = 0;
  const saves = [];
  twm.init({ fitActiveTab: () => {}, saveLayout: () => saves.push(1), invoke });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { renders += 1; } });
  twm.setViewMode('tunnels', 'window');
  viewModeItems(twm.buildContextMenuItems('tunnels'))[0].onSelect(); // dock again
  assert.strictEqual(twm.getViewModes().tunnels, 'dock');
  const before = {
    active: twm.isVisible('tunnels'),
    zoneSlot: twm.getActiveZoneAssignments()['right-bottom'],
    children: zoneEls.get('right-bottom')._contentEl.children.length,
    renders,
  };
  assert.strictEqual(before.active, true, 'precondition: the panel is docked and showing');
  saves.length = 0;

  twm.notifyHostAborted('tunnels');

  assert.strictEqual(twm.isVisible('tunnels'), before.active,
    'a stale abort must not clear the active flag of an already-docked window');
  assert.strictEqual(twm.getActiveZoneAssignments()['right-bottom'], before.zoneSlot);
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, before.children);
  assert.strictEqual(renders, before.renders);
  assert.strictEqual(saves.length, 0, 'an inert event must not churn the saved layout');
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
    { cmd: 'open_panel_host', args: { toolWindowId: 'ssh-sessions', title: 'Hosts', companionIds: [] } },
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

// --- 14b. Moving a POPPED-OUT window re-aims its dock target only ----------
// Scenario 8 moves before popping out, so it never enters moveTo()'s
// window-mode branch. This one moves while popped out, into a zone that
// already has a docked window showing — the branch must not reparent
// anything, must not render, and must not take the destination zone over the
// way a docked move does.
{
  const { twm, invoke, zoneEls } = loadManager();
  let hostRenders = 0;
  let tunnelRenders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('ssh-sessions', {
    ...HOSTS, defaultZone: 'left-top', renderFn: () => { hostRenders += 1; },
  });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { tunnelRenders += 1; } });
  twm.setViewMode('tunnels', 'window');
  hostRenders = 0;
  tunnelRenders = 0;

  twm.moveTo('tunnels', 'left-top');

  assert.strictEqual(twm.getZoneForWindow('tunnels'), 'left-top',
    'the move re-aims where the window will dock BACK to');
  assert.strictEqual(twm.getViewModes().tunnels, 'window', 'moving is not docking');
  assert.deepStrictEqual(Array.from(twm.getWindowsInZone('right-bottom')), [],
    'the window leaves its old zone\'s list');
  assert.ok(Array.from(twm.getWindowsInZone('left-top')).includes('tunnels'),
    'and joins the new one\'s, so its rail button moves with it');
  assert.strictEqual(tunnelRenders, 0, 'nothing is rendered — the host owns the DOM');
  assert.strictEqual(twm.getContentElement('tunnels'), null);
  assert.strictEqual(hostRenders, 0, 'the sitting tenant is not re-rendered either');
  assert.strictEqual(zoneEls.get('left-top')._contentEl.children.length, 1,
    'the destination zone still holds exactly its one docked panel');
  assert.strictEqual(twm.isVisible('ssh-sessions'), true,
    'a popped-out window must not evict the zone\'s docked window');
  assert.strictEqual(twm.getActiveZoneAssignments()['left-top'], 'ssh-sessions',
    'and the docked window keeps the zone\'s one open-window slot');
}

// --- 14c. "Hide" on a popped-out window hides its host --------------------
// Reachable from the rail's context menu, which offers Hide for any id.
{
  const { twm, invoke, invokeCalls, zoneEls } = loadManager();
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { renders += 1; } });
  twm.setViewMode('tunnels', 'window');
  renders = 0;
  invokeCalls.length = 0;

  const hideItem = Array.from(twm.buildContextMenuItems('tunnels'))
    .find((i) => i.label === 'Hide');
  hideItem.onSelect();

  assert.deepStrictEqual(plain(invokeCalls), [
    { cmd: 'hide_panel_host', args: { toolWindowId: 'tunnels' } },
  ], 'Hide must hide the host window, not close a zone that is not showing it');
  assert.strictEqual(twm.getViewModes().tunnels, 'window', 'hiding is not un-popping');
  assert.strictEqual(twm.isVisible('tunnels'), false, 'the rail button goes dark');
  assert.strictEqual(renders, 0, 'nothing docks back on a hide');
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 0);
}

// --- 14d. Unregistering a popped-out window DESTROYS its host --------------
// What a plugin removal does. Destroy, not hide: every line of unregister()
// deletes the bookkeeping that could bring this window back, so a hide would
// strand a live, invisible webview still running the removed plugin's panel,
// with no rail entry and no summon path left to reach it.
{
  const { twm, invoke, invokeCalls, zoneEls } = loadManager();
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('plugin:fake', { title: 'Fake', renderFn: () => { renders += 1; } });
  twm.setViewMode('plugin:fake', 'window');
  renders = 0;
  invokeCalls.length = 0;

  twm.unregister('plugin:fake');

  assert.deepStrictEqual(plain(invokeCalls), [
    { cmd: 'dock_panel_host', args: { toolWindowId: 'plugin:fake' } },
  ], 'an orphaned host must be destroyed, not hidden');
  assert.strictEqual(twm.getRegistration('plugin:fake'), null);
  assert.deepStrictEqual(plain(twm.getViewModes()), {},
    'the id\'s view-mode bookkeeping goes with the registration');
  assert.deepStrictEqual(Array.from(twm.getWindowsInZone('right-bottom')), []);

  // The docked echo Rust sends back is doubly inert: the id fails
  // notifyHostDocked's toolWindows.has() guard, and there is no registration
  // left to supply a renderFn even in principle.
  twm.notifyHostDocked('plugin:fake', 1);
  assert.strictEqual(renders, 0, 'an unregistered id has nothing to remount');
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 0);
}

// --- 14e. A STALE docked echo must not remount over a live second host -----
// The reachable sequence: pick Dock, then pick Window again before the first
// dock's IPC has round-tripped. The echo then lands with the tool window back
// in window mode, so the mode guard alone waves it through and the panel is
// remounted into the zone while the SECOND host window is showing it — the
// exact "two live instances of one stateful panel" the parent-dock ruling set
// out to kill. The generation token (`reqId`, minted by Rust and returned by
// open_panel_host) is what tells the two apart.
{
  let nextReqId = 1;
  const { twm, invoke, invokeCalls, zoneEls } = loadManager({
    invokeHandlers: { open_panel_host: () => Promise.resolve(nextReqId++) },
  });
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { renders += 1; } });

  twm.setViewMode('tunnels', 'window');   // host generation 1
  await tick();
  renders = 0;
  invokeCalls.length = 0;

  viewModeItems(twm.buildContextMenuItems('tunnels'))[0].onSelect(); // Dock
  assert.strictEqual(renders, 1, 'the dock remounts once');
  twm.setViewMode('tunnels', 'window');   // re-pop, generation 2 still in flight

  twm.notifyHostDocked('tunnels', 1);     // generation 1's echo, arriving late

  assert.strictEqual(renders, 1, 'the stale echo must NOT remount a second time');
  assert.strictEqual(twm.getViewModes().tunnels, 'window',
    'and must not drag the window out of the mode the user just re-picked');
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 0,
    'nothing is in the zone while a host is live');

  await tick();                            // generation 2 is minted
  twm.notifyHostDocked('tunnels', 2);      // the genuine dock-back

  assert.strictEqual(renders, 2, 'the CURRENT generation still docks normally');
  assert.strictEqual(twm.getViewModes().tunnels, 'dock');
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 1);
}

// --- 14e2. An open resolving OUT OF ORDER must not resurrect its generation -
// The three-toggle variant of 14e, and the reason the pending marker carries a
// per-attempt issue number rather than being one shared sentinel: Window (open
// A, unanswered) → Dock → Window again (open B, unanswered) → A answers LATE.
// A guard that only asked "is this id still in window mode?" would say yes —
// open B put it there — and write generation 1 over B's pending marker, at
// which point generation 1's stale echo matches and remounts a panel that is
// live inside B's host window.
{
  const openResolvers = [];
  const { twm, invoke, zoneEls } = loadManager({
    invokeHandlers: {
      open_panel_host: () => new Promise((resolve) => { openResolvers.push(resolve); }),
    },
  });
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { renders += 1; } });

  twm.setViewMode('tunnels', 'window');                              // open A
  renders = 0; // discount the render registration into a visible zone did
  viewModeItems(twm.buildContextMenuItems('tunnels'))[0].onSelect(); // Dock
  assert.strictEqual(renders, 1, 'the dock remounts once');
  twm.setViewMode('tunnels', 'window');                              // open B
  assert.strictEqual(openResolvers.length, 2, 'two opens are genuinely in flight');

  openResolvers[0](1); // A answers, out of order, long after B was issued
  await tick();

  twm.notifyHostDocked('tunnels', 1); // generation 1's echo

  assert.strictEqual(renders, 1,
    "an abandoned attempt's generation must not be resurrected for a stale echo to match");
  assert.strictEqual(twm.getViewModes().tunnels, 'window');
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 0);

  // B is still the live attempt, and still docks normally when it answers.
  openResolvers[1](2);
  await tick();
  twm.notifyHostDocked('tunnels', 2);
  assert.strictEqual(renders, 2, 'the attempt that actually owns the slot still docks');
  assert.strictEqual(twm.getViewModes().tunnels, 'dock');
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 1);
}

// --- 14f. An echo with no generation still docks (pre-reqId fallback) ------
// Rust always stamps the event now, but a manager that dropped a legitimate
// dock-back because a payload lacked the field would be worse than the race
// it guards. Scenarios 8/9 exercise the same fallback implicitly; this states
// it.
{
  const { twm, invoke, zoneEls } = loadManager({
    invokeHandlers: { open_panel_host: () => Promise.resolve(41) },
  });
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('tunnels', { ...TUNNELS, renderFn: () => { renders += 1; } });
  twm.setViewMode('tunnels', 'window');
  await tick();
  renders = 0;

  twm.notifyHostDocked('tunnels');

  assert.strictEqual(renders, 1, 'no generation to compare — dock as before');
  assert.strictEqual(zoneEls.get('right-bottom')._contentEl.children.length, 1);
}

// --- 14g. A dock-back with NO stored generation is accepted ---------------
// The other accept branch of dockedEchoIsStale, and the one real path that
// reaches it: after a relaunch the view mode is restored from the saved
// layout and summonWindowHost's focus_panel_host SUCCEEDS, so no open attempt
// is ever issued and the id has no entry in the generation map at all. A
// genuine dock-back from that host must still be honoured — there is nothing
// to disagree with, and dropping it would strand the panel in no window.
{
  const { twm, invoke, invokeCalls, zoneEls } = loadManager({
    invokeHandlers: { focus_panel_host: () => Promise.resolve() },
  });
  let renders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.setPersistedZones({ 'ssh-sessions': 'right-top' });
  twm.setPersistedViewModes({ 'ssh-sessions': 'window' });
  twm.setPersistedActiveZoneWindows({ 'right-top': 'ssh-sessions' });
  twm.setPersistedPanelVisibility({ left: true, right: true, bottom: true });
  twm.register('ssh-sessions', { ...HOSTS, renderFn: () => { renders += 1; } });
  twm.summonPendingWindowHosts();
  await tick();

  assert.deepStrictEqual(invokeCalls.map((c) => c.cmd), ['focus_panel_host'],
    'precondition: the host was summoned by focus alone — no open, so no generation');
  assert.strictEqual(renders, 0);

  twm.notifyHostDocked('ssh-sessions', 7);

  assert.strictEqual(renders, 1, 'a dock-back with nothing to compare against is accepted');
  assert.strictEqual(twm.getViewModes()['ssh-sessions'], 'dock');
  assert.strictEqual(zoneEls.get('right-top')._contentEl.children.length, 1);
}

// --- 14h. moveTo's OLD-zone fallback must not promote a popped sibling -----
// F1 (branch review): a zone holding A (docked, active) and B (docked).
// B pops out — it keeps its slot in the zone's window list (that IS its
// remembered dock target) but has no DOM and no claim on activeId. Moving A
// to a different zone empties the old activeId slot; the fallback used to
// grab zone.windows[0] raw, which is now B, and rendered+activated it into a
// zone whose host window (B's) is simultaneously live on screen — two live
// instances of B, plus a rail/persisted-slot mismatch. It must instead skip
// B and land on nothing (there is no other dockable window left).
{
  const { twm, invoke, zoneEls } = loadManager();
  let aRenders = 0;
  let bRenders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('a', { ...HOSTS, defaultZone: 'right-top', renderFn: () => { aRenders += 1; } });
  twm.register('b', { ...TUNNELS, defaultZone: 'right-top', renderFn: () => { bRenders += 1; } });
  assert.strictEqual(twm.getActiveZoneAssignments()['right-top'], 'a',
    'precondition: a is docked and active, b is docked but not active');

  twm.setViewMode('b', 'window');
  aRenders = 0;
  bRenders = 0;

  twm.moveTo('a', 'left-top');

  assert.strictEqual(bRenders, 0,
    'the popped-out sibling left behind in the old zone must NOT be rendered');
  assert.strictEqual(zoneEls.get('right-top')._contentEl.children.length, 0,
    'nothing was mounted into the vacated zone');
  assert.strictEqual(twm.getViewModes().b, 'window',
    'and it must not be dragged back into dock mode by the fallback');
  assert.ok(Array.from(twm.getWindowsInZone('right-top')).includes('b'),
    'b keeps its remembered dock target — moveTo only re-aimed a, not b');
  assert.strictEqual(aRenders, 0,
    'a is reparented, not re-rendered — it already had a live element from registration');
  assert.strictEqual(zoneEls.get('left-top')._contentEl.children.length, 1,
    'a landed in its new zone as an ordinary docked move does');
}

// --- 14i. unregister's fallback must not promote a popped sibling ----------
// F1's second call site: same zone shape as 14h, but the docked-active
// window (a) is REMOVED (a plugin uninstall) instead of moved. The raw
// zone.windows[0] fallback would again pick the popped sibling b and render
// it into the zone while its host window is still showing it live.
{
  const { twm, invoke, zoneEls } = loadManager();
  let aRenders = 0;
  let bRenders = 0;
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('a', { ...HOSTS, defaultZone: 'right-top', renderFn: () => { aRenders += 1; } });
  twm.register('b', { ...TUNNELS, defaultZone: 'right-top', renderFn: () => { bRenders += 1; } });
  assert.strictEqual(twm.getActiveZoneAssignments()['right-top'], 'a',
    'precondition: a is docked and active, b is docked but not active');

  twm.setViewMode('b', 'window');
  aRenders = 0;
  bRenders = 0;

  twm.unregister('a');

  assert.strictEqual(bRenders, 0,
    'the popped-out sibling must NOT be promoted/rendered when its docked-active sibling is unregistered');
  assert.strictEqual(zoneEls.get('right-top')._contentEl.children.length, 0,
    'nothing was mounted into the zone a vacated');
  assert.strictEqual(twm.getViewModes().b, 'window',
    'b\'s view mode is untouched by a\'s removal');
  assert.ok(Array.from(twm.getWindowsInZone('right-top')).includes('b'),
    'b is still registered in the zone, just not promoted to active');
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
    // Added for the task-6 review's F3 project-boot-visibility coverage: a
    // project window's boot path calls activate('file-explorer') directly
    // (not through the legacy toggle path Part 1 exercises), which no
    // existing Part 2 test reached before — a fake manager without this
    // would throw "activate is not a function" the first time that path ran.
    activate: record('activate'),
    deactivate: record('deactivate'),
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

async function loadRuntime(savedLayout, opts) {
  const options = opts || {};
  const twm = makeFakeManager();
  const invokeCalls = [];
  const listeners = new Map();
  const transferPanelInits = [];
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
  sandbox.toast = {};
  // task-6 review, F3/F5(iv): create(deps) reads global.termlabProjectMode
  // once, at create() time, to resolve this window's projectRoot — only set
  // when a scenario asks for it, so every other Part 2 test keeps exercising
  // the plain (non-project) window path unchanged.
  if (options.projectRoot) {
    sandbox.termlabProjectMode = { root: () => options.projectRoot };
  }
  // The EFFECTIVE zen decision startup-runtime.js would have already
  // computed and published by the time tool-window-runtime.js's init() runs
  // — read as window.__termlabEffectiveZen (== sandbox.__termlabEffectiveZen,
  // since sandbox.window === sandbox here).
  if (options.effectiveZen) {
    sandbox.__termlabEffectiveZen = true;
  }
  sandbox.termlabTransferRuntime = {
    ensureStarted(options) {
      twm.calls.push({ name: 'transferEnsureStarted', args: [options] });
      return Promise.resolve();
    },
  };
  sandbox.transferCenterPanel = {
    init(options) { transferPanelInits.push(options); },
  };
  // Only wired up when a scenario cares about the localStorage-backed
  // migration flag (see the SFTP-bottom-zone migration regression below);
  // every other caller keeps the prior behaviour of global.localStorage
  // being undefined, which the migration's try/catch already tolerates.
  if (options.localStorage) {
    sandbox.localStorage = options.localStorage;
  }
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
  return { twm, invokeCalls, listeners, emit, result, transferPanelInits };
}

// --- 14j. Main-window transfer state starts before panels can consume it ---
{
  const { twm } = await loadRuntime({});
  const transferStartIdx = twm.calls.findIndex((call) => call.name === 'transferEnsureStarted');
  const firstRegisterIdx = twm.calls.findIndex((call) => call.name === 'register');
  assert.ok(transferStartIdx >= 0, 'main tool-window init must start the transfer runtime');
  assert.ok(transferStartIdx < firstRegisterIdx, 'transfer listeners/snapshot must start before built-in panel registration');
  const options = twm.calls[transferStartIdx].args[0];
  assert.equal(typeof options.invoke, 'function');
  assert.equal(typeof options.listen, 'function');
  assert.ok(options.toast && typeof options.toast === 'object');
}

// --- 14k. Transfers registers after SFTP and mounts through one renderFn ---
{
  const { twm, transferPanelInits } = await loadRuntime({});
  const registrations = twm.calls.filter((call) => call.name === 'register');
  const ids = registrations.map((call) => call.args[0]);
  assert.strictEqual(ids[0], 'file-explorer',
    'SFTP remains the first/default active bottom window for existing layouts');
  assert.strictEqual(ids[1], 'transfer-center',
    'Transfers registers immediately after SFTP without taking the default slot');

  const registration = registrations[1].args[1];
  assert.deepStrictEqual(
    plain({ title: registration.title, icon: registration.icon, type: registration.type, defaultZone: registration.defaultZone }),
    { title: 'Transfers', icon: null, type: 'built-in', defaultZone: 'bottom' },
  );
  const container = makeElement('div');
  registration.renderFn(container);
  assert.strictEqual(container.children[0].id, 'transfer-center-panel');
  assert.strictEqual(transferPanelInits.length, 1);
  assert.strictEqual(transferPanelInits[0].panelEl, container.children[0]);
  assert.strictEqual(typeof transferPanelInits[0].invoke, 'function');
  assert.strictEqual(typeof transferPanelInits[0].listen, 'function');
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

// --- 16a. A project window boot always reveals the Files tool window
// (task-6 review, F3/F5(iv)) -------------------------------------------
// This is deliberately given a layout where active_tool_windows already
// records a 'bottom' zone window (mirroring how save_window_layout writes
// active_tool_windows on essentially every save) — the ORIGINAL, buggy
// `knowsBottom` guard would have read this as "the layout already knows
// about bottom" and skipped the reveal entirely, which is exactly the no-op
// bug F3 found. The controller's ruling dropped that guard: the reveal is
// now unconditional whenever this window has a project.
{
  const { twm } = await loadRuntime(
    // 'bottom-left', not 'right-top': this is the realistic case — an
    // existing install's saved layout already has file-explorer (or
    // whatever else lives there) recorded in a bottom-prefixed zone, simply
    // because that IS file-explorer's own default zone. The old
    // `knowsBottom` guard treated this as "the layout already knows about
    // bottom, leave it alone" and skipped the reveal — which is exactly the
    // near-universal no-op F3 found. A key that does NOT start with
    // 'bottom' would not have reproduced the bug (the guard was already
    // passing in that case), so this is deliberately NOT a 'right-top' key.
    { active_tool_windows: { 'bottom-left': 'file-explorer' } },
    { projectRoot: '/repo' },
  );
  const reveal = twm.calls.find((c) => c.name === 'setPanelVisibility' && c.args[0] === 'bottom' && c.args[1] === true);
  const activateCall = twm.calls.find((c) => c.name === 'activate' && c.args[0] === 'file-explorer');
  assert.ok(reveal, 'a project window boot must reveal the bottom zone, even when the layout already records one');
  assert.ok(activateCall, 'a project window boot must activate file-explorer');
  const registration = twm.calls.find((c) => c.name === 'register' && c.args[0] === 'file-explorer');
  assert.strictEqual(registration.args[1].title, 'Project', 'file-explorer is titled Project, not SFTP, in a project window');
}

// --- 16b. ...but a zen-effective project window does NOT end up revealed
// (task-6 review, F3/F5(iv)) -------------------------------------------
// The reveal above still fires unconditionally (it must — the zen-effective
// check runs strictly after it in source order), but the zen block that
// follows it re-hides 'left'/'right'/'bottom' whenever
// window.__termlabEffectiveZen is true, and that re-hide must be the LAST
// word on 'bottom' visibility for this scenario.
{
  const { twm } = await loadRuntime(
    {},
    { projectRoot: '/repo', effectiveZen: true },
  );
  const bottomVisibilityCalls = twm.calls.filter(
    (c) => c.name === 'setPanelVisibility' && c.args[0] === 'bottom',
  );
  assert.ok(bottomVisibilityCalls.length >= 2,
    'both the project reveal and the zen re-hide must have run against bottom');
  const last = bottomVisibilityCalls[bottomVisibilityCalls.length - 1];
  assert.strictEqual(last.args[1], false,
    'the zen-effective hide must be the LAST setPanelVisibility(bottom, ...) call — zen wins over the project reveal');
  // activate('file-explorer') still fires (the reveal block ran), but that's
  // fine: a hidden zone's active tab is inert, and this is what
  // summonPendingWindowHosts / a later zen-off toggle restores correctly.
  assert.ok(twm.calls.some((c) => c.name === 'activate' && c.args[0] === 'file-explorer'));
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

// --- 17b. The docked event forwards its generation token ------------------
{
  const { twm, emit } = await loadRuntime({});
  emit('panel-host-docked', { toolWindowId: 'tunnels', reqId: 12 });

  const call = twm.calls.find((c) => c.name === 'notifyHostDocked');
  assert.deepStrictEqual(plain(call.args), ['tunnels', 12],
    'reqId must reach the manager, or the stale-echo guard has nothing to compare');
}

// --- 17c. Regression: a post-split bottom-right layout must not be re-migrated
//
// The one-time "SFTP moved into the bottom zone" migration (init(), just
// above the setPersistedZones/setPersistedActiveZoneWindows calls exercised
// here) used to detect "this layout already knows about the bottom zone" by
// checking for the exact legacy value 'bottom'. getZoneAssignments() (and
// therefore every layout saved by a build with the bottom-left/bottom-right
// split) never emits that legacy value again, so a user whose SFTP already
// lived in 'bottom-right' — with the migration's localStorage flag unset,
// e.g. a fresh profile or one where the flag was cleared — would get the
// migration re-fired on next boot: rewritten to 'bottom-left' and the
// bottom-right active entry deleted, silently undoing their arrangement.
{
  const store = new Map();
  const fakeLocalStorage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => { store.set(key, String(value)); },
  };
  const { twm } = await loadRuntime({
    tool_window_zones: { 'file-explorer': 'bottom-right' },
    active_tool_windows: { 'bottom-right': 'file-explorer' },
  }, { localStorage: fakeLocalStorage });

  const zonesCall = twm.calls.find((c) => c.name === 'setPersistedZones');
  assert.ok(zonesCall, 'tool_window_zones must still be handed to the manager');
  assert.strictEqual(zonesCall.args[0]['file-explorer'], 'bottom-right',
    'a layout that already knows about a bottom-* zone must not be force-migrated to bottom-left');

  const activeCall = twm.calls.find((c) => c.name === 'setPersistedActiveZoneWindows');
  assert.ok(activeCall, 'active_tool_windows must still be handed to the manager');
  assert.strictEqual(activeCall.args[0]['bottom-right'], 'file-explorer',
    'the bottom-right active-window entry must survive untouched');
  assert.ok(!('bottom-left' in activeCall.args[0]),
    'no bottom-left key should appear when the migration correctly does not fire');

  assert.strictEqual(store.has('termlab.migration.sftpBottomZone'), false,
    'the one-time migration flag must stay unset when the migration does not fire');
}

// ===========================================================================
// Part 3 — the label-prefix branch in app/main-runtime.js
// ===========================================================================

// main-runtime.js is a classic script whose whole body is an async `start()`
// handed to termlabBootstrap.run. The harness supplies exactly the globals
// the code touches BEFORE the branch, and throwing canaries for everything
// after it: if the branch ever stops returning, one of them fires.
function loadMainRuntime(label, opts) {
  const options = opts || {};
  const bootCalls = [];
  const statusMessages = [];
  const reached = [];
  let bootstrapPromise = null;

  const canary = (name) => () => {
    throw new Error(`${name} must never run in a panel host window`);
  };

  const currentWindow = { close: () => { reached.push('currentWindow.close'); } };
  const tauriClient = {
    invoke: (cmd) => {
      reached.push('invoke:' + cmd);
      if (cmd === 'current_window_label') return Promise.resolve(label);
      return Promise.resolve(undefined);
    },
    listen: () => Promise.resolve(() => {}),
    listenOnCurrentWindow: () => Promise.resolve(() => {}),
    currentWindow,
  };

  const sandbox = {
    console: { log() {}, warn() {}, error() {} },
    setTimeout,
    clearTimeout,
    Promise,
    Math,
    Number,
    Object,
    JSON,
    Error,
    String,
    requestAnimationFrame: (fn) => setTimeout(fn, 0),
    document: {
      body: makeElement('body'),
      createElement: (t) => makeElement(t),
      getElementById: () => null,
      querySelector: () => null,
      querySelectorAll: () => [],
      addEventListener() {},
      fonts: { load: () => Promise.resolve(), ready: Promise.resolve() },
    },
    __TAURI__: { core: {}, event: {}, window: {} },
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;

  // The startup runtime's config/theme loaders sit between the branch and the
  // compose runtime, so they are canaries for the HOST case and benign
  // resolvers for the fall-through case (scenario 19), whose sentinel is the
  // compose runtime itself.
  const startupLoader = options.composeCreate
    ? () => Promise.resolve({})
    : null;
  sandbox.termlabStartupRuntime = {
    create: () => ({
      initStatusController: () => ({
        showStatus: (m) => statusMessages.push(m),
        hideStatus: () => {},
      }),
      ensureRuntimeDependencies: () => true,
      loadTerminalConfig: startupLoader || canary('startupRuntime.loadTerminalConfig'),
      applyAppConfig: startupLoader || canary('startupRuntime.applyAppConfig'),
      loadTheme: startupLoader || canary('startupRuntime.loadTheme'),
    }),
  };
  sandbox.termlabBootstrap = {
    run: (startFn) => {
      bootstrapPromise = Promise.resolve().then(() => startFn());
      return bootstrapPromise;
    },
  };
  sandbox.termlabTauriClient = { create: () => tauriClient };
  sandbox.termlabLayoutService = { create: () => ({}) };

  // Everything main-runtime.js builds AFTER the branch.
  sandbox.termlabComposeRuntime = {
    create: options.composeCreate || canary('composeRuntime.create'),
  };
  sandbox.termlabManagerComposeRuntime = { create: canary('managerComposeRuntime.create') };
  sandbox.termlabLayoutRuntime = { create: canary('layoutRuntime.create') };
  sandbox.termlabBridgeRuntime = { create: canary('bridgeRuntime.create') };
  sandbox.termlabEventWiringRuntime = { create: canary('eventWiringRuntime.create') };
  sandbox.termlabOrchestrationRuntime = { create: canary('orchestrationRuntime.create') };
  sandbox.termlabWindowSize = { rendererCellSize: canary('windowSize.rendererCellSize') };
  sandbox.splitPane = { renderTree: canary('splitPane.renderTree') };

  sandbox.termlabPanelHostRuntime = {
    boot: (deps) => {
      bootCalls.push(deps);
      return Promise.resolve({ status: 'mounted' });
    },
  };

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MAIN_RUNTIME_PATH, 'utf8'), sandbox, {
    filename: MAIN_RUNTIME_PATH,
  });

  return { sandbox, bootCalls, statusMessages, reached, tauriClient, currentWindow,
    settled: () => bootstrapPromise };
}

// --- 18. A panelhost-* label hands off and returns ------------------------
{
  const { bootCalls, statusMessages, settled, tauriClient, currentWindow } =
    loadMainRuntime('panelhost-window-1-7');
  await settled();

  assert.strictEqual(bootCalls.length, 1,
    'a panelhost-* label must hand off to termlabPanelHostRuntime.boot exactly once');
  const deps = bootCalls[0];
  assert.strictEqual(deps.tauriClient, tauriClient, 'the shared client is passed through');
  assert.strictEqual(deps.currentWindow, currentWindow);
  for (const key of ['invoke', 'listen', 'listenOnCurrentWindow']) {
    assert.strictEqual(typeof deps[key], 'function', `boot must receive ${key}`);
  }
  assert.deepStrictEqual(statusMessages, [],
    'the hand-off is the normal path, not an error path');
  // The canaries above did the real work: reaching compose, the manager
  // composer, the event wiring, the orchestration runtime or createTab would
  // have thrown out of start() and rejected the bootstrap promise.
}

// --- 19. An ordinary label does NOT take the host path -------------------
{
  const composeReached = [];
  const STOP = new Error('stop-after-branch');
  const { bootCalls, settled } = loadMainRuntime('window-1', {
    composeCreate: () => { composeReached.push(1); throw STOP; },
  });
  await settled().catch((err) => {
    assert.strictEqual(err, STOP, 'the run must stop at the compose sentinel, not earlier');
  });

  assert.deepStrictEqual(bootCalls, [],
    'a main window must never boot the panel host runtime');
  assert.strictEqual(composeReached.length, 1,
    'and must fall through to the compose runtime as before');
}

// ===========================================================================
// Part 4 — app/panel-host-runtime.js (the host boot)
// ===========================================================================

function makeHostSandbox(config) {
  const cfg = config || {};
  const timeline = [];
  const invokeCalls = [];
  const listens = { app: new Map(), window: new Map() };
  const closeCalls = [];
  const pluginWidgetsInits = [];
  const appearanceApplies = [];
  const themeCssApplies = [];
  const uiConfigApplies = [];
  const appearanceSyncs = [];
  const panelInits = [];
  const transferRuntimeStarts = [];
  const panelDestroys = [];
  const windowLifecycleListeners = new Map();

  const body = makeElement('body');
  const appEl = makeElement('div');
  const zoneEls = new Map();
  for (const z of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right']) {
    zoneEls.set(z, makeZoneEl(z));
  }
  const byId = new Map([
    ['app', appEl],
    ['left-strip', makeElement('div')],
    ['right-strip', makeElement('div')],
    ['bottom-strip', makeElement('div')],
    ['left-sidebar', makeElement('div')],
    ['right-sidebar', makeElement('div')],
    ['bottom-zone-wrap', makeElement('div')],
    ['bottom-zone-resize', makeElement('div')],
    ['bottom-zone-divider', makeElement('div')],
  ]);

  const warnCalls = [];
  const sandbox = {
    console: { log() {}, warn: (...args) => { warnCalls.push(args); }, error() {} },
    setTimeout,
    clearTimeout,
    Promise,
    Math,
    Number,
    Object,
    JSON,
    Error,
    String,
    Array,
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
    addEventListener(name, handler) {
      if (!windowLifecycleListeners.has(name)) windowLifecycleListeners.set(name, []);
      windowLifecycleListeners.get(name).push(handler);
    },
    removeEventListener(name, handler) {
      const handlers = windowLifecycleListeners.get(name) || [];
      const index = handlers.indexOf(handler);
      if (index >= 0) handlers.splice(index, 1);
    },
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;

  // The five built-in panels, stubbed at their init boundary so a render is
  // observable without dragging the real panels into the harness.
  const filesPanelOnTabChangedCalls = [];
  const panelStub = (name) => ({
    init: (opts) => {
      timeline.push('render:' + name);
      panelInits.push({ name, opts });
    },
  });
  sandbox.filesPanel = panelStub('file-explorer');
  // Task 5's dispatch target: a real filesPanel.onTabChanged is exercised
  // end to end in Part 5 (against the real files-panel.js); here it is just
  // a recorder, so Part 4's boot-focused scenarios don't have to care that
  // every boot() now installs a live event sink (see scenario 26).
  sandbox.filesPanel.onTabChanged = (payload) => { filesPanelOnTabChangedCalls.push(payload); };
  sandbox.sshPanel = panelStub('ssh-sessions');
  sandbox.tunnelsPanel = panelStub('tunnels');
  sandbox.notificationsPanel = panelStub('notifications');
  sandbox.transferCenterPanel = {
    init: (opts) => {
      timeline.push('render:transfer-center');
      panelInits.push({ name: 'transfer-center', opts });
      return { destroy: () => panelDestroys.push(1) };
    },
  };
  sandbox.termlabTransferRuntime = {
    ensureStarted(options) {
      transferRuntimeStarts.push(options);
      return Promise.resolve();
    },
  };

  sandbox.pluginWidgets = {
    init: (opts) => { pluginWidgetsInits.push(opts); },
    renderWidgets: () => {},
  };
  sandbox.termlabAppearance = {
    apply: (mode) => { appearanceApplies.push(mode); },
    current: () => 'light',
  };
  sandbox.termlabConfigService = {
    applyThemeCss: (tc) => { themeCssApplies.push(tc); },
    applyUiConfig: (cfgIn) => { uiConfigApplies.push(cfgIn); },
  };
  sandbox.termlabAppearanceSync = {
    create: (deps) => {
      appearanceSyncs.push(deps);
      return { init: () => {} };
    },
  };

  // titlebar.js IS loaded by index.html, so it is reachable from a host — and
  // an UNINITIALIZED `refresh()` skips rendering but still publishes the
  // app-menu accelerator table into the keyboard router. These two stubs make
  // both halves observable: the refresh call itself, and any router handler it
  // would have registered.
  const titlebarRefreshes = [];
  const routerRegistrations = [];
  sandbox.titlebar = {
    refresh: () => {
      titlebarRefreshes.push(1);
      // What the real one does at this point (titlebar.js's
      // registerAccelerators): ~18 dead bindings at priority 115.
      sandbox.termlabKeyboardRouter.register('titlebar-accelerators', () => true, 115);
      return Promise.resolve();
    },
  };
  sandbox.termlabKeyboardRouter = {
    register: (id, handler, priority) => {
      routerRegistrations.push({ id, priority });
      return () => {};
    },
  };

  vm.createContext(sandbox);
  // Loaded for real, not stubbed: the host's dispatch table reads
  // BRIDGE_EVENTS / ACTIVE_PANE_CHANGED_EVENT from this module rather than a
  // second literal, so the harness has to supply the genuine article for
  // that guard to mean anything.
  vm.runInContext(fs.readFileSync(BRIDGE_PATH, 'utf8'), sandbox, { filename: BRIDGE_PATH });
  vm.runInContext(fs.readFileSync(MANAGER_PATH, 'utf8'), sandbox, { filename: MANAGER_PATH });
  vm.runInContext(fs.readFileSync(RUNTIME_PATH, 'utf8'), sandbox, { filename: RUNTIME_PATH });
  vm.runInContext(fs.readFileSync(HOST_RUNTIME_PATH, 'utf8'), sandbox, {
    filename: HOST_RUNTIME_PATH,
  });

  // The hard canary for "registrations only": a host must never wire the
  // manager to zone/sidebar/strip DOM.
  sandbox.toolWindowManager.init = () => {
    throw new Error('toolWindowManager.init must not run in a panel host window');
  };

  const answers = Object.assign(
    {
      get_panel_host_request: () => Promise.resolve({
        reqId: 7,
        toolWindowId: 'ssh-sessions',
        parentLabel: 'window-1',
        title: 'Hosts',
      }),
      GET_APP_CONFIG: () => Promise.resolve({ appearance_mode: 'light', platform: 'macos' }),
      GET_THEME_COLORS: () => Promise.resolve({ bg: '#000' }),
      get_plugin_panels: () => Promise.resolve([]),
    },
    cfg.answers || {},
  );

  const invoke = (cmd, args) => {
    timeline.push('invoke:' + cmd);
    invokeCalls.push({ cmd, args });
    const answer = answers[cmd];
    if (typeof answer === 'function') return Promise.resolve().then(() => answer(args));
    return Promise.resolve(undefined);
  };
  const subscribe = (table) => (name, fn) => {
    if (!table.has(name)) table.set(name, []);
    table.get(name).push(fn);
    return Promise.resolve(() => {});
  };
  const windowControlCalls = [];
  let maximizedState = false;
  const currentWindow = {
    close: () => { closeCalls.push(1); timeline.push('close'); },
    minimize: () => { windowControlCalls.push('minimize'); },
    maximize: () => { windowControlCalls.push('maximize'); maximizedState = true; },
    unmaximize: () => { windowControlCalls.push('unmaximize'); maximizedState = false; },
    isMaximized: () => Promise.resolve(maximizedState),
  };

  const bootDeps = {
    invoke,
    listen: subscribe(listens.app),
    listenOnCurrentWindow: subscribe(listens.window),
    currentWindow,
    tauriClient: {},
  };

  return {
    sandbox,
    bootDeps,
    timeline,
    invokeCalls,
    listens,
    closeCalls,
    pluginWidgetsInits,
    appearanceApplies,
    themeCssApplies,
    uiConfigApplies,
    appearanceSyncs,
    panelInits,
    transferRuntimeStarts,
    panelDestroys,
    titlebarRefreshes,
    routerRegistrations,
    warnCalls,
    filesPanelOnTabChangedCalls,
    windowControlCalls,
    body,
    appEl,
    zoneEls,
    byId,
    emitWindow: (name, payload) => {
      for (const fn of listens.window.get(name) || []) fn({ payload });
    },
    fireWindowLifecycle: (name) => {
      for (const fn of windowLifecycleListeners.get(name) || []) fn({ type: name });
    },
    // DFS over the stub DOM tree rooted at <body> for the first element
    // carrying `cls` — the composite layout's only observable shape (Part 4,
    // scenario 26c/26d) is the class names buildChrome/boot hang off the
    // elements they create, so this is the minimal query surface those
    // scenarios need. Accepts either 'panel-host-bottom' or '.panel-host-
    // bottom'.
    query: (sel) => {
      const cls = String(sel || '').replace(/^\./, '');
      const visit = (el) => {
        if (!el) return null;
        const classes = String(el.className || '').split(/\s+/).filter(Boolean);
        if (classes.includes(cls)) return el;
        for (const child of el.children || []) {
          const found = visit(child);
          if (found) return found;
        }
        return null;
      };
      return visit(body);
    },
  };
}

// --- 20. The happy path: chrome, mount, ready ----------------------------
{
  const host = makeHostSandbox();
  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(result.status, 'mounted');
  assert.strictEqual(host.body.classList.contains('tl-panelhost-window'), true,
    'the body class is what stands the terminal shell (#app) down');

  // Chrome: the title comes from the REQUEST (Rust carries the title the
  // parent popped out with), and the dock-back button is wired.
  assert.strictEqual(result.titleEl.textContent, 'Hosts');
  assert.strictEqual(result.rootEl.className, 'tl-panelhost');
  assert.strictEqual(result.headerEl.className, 'tl-panelhost__header');
  assert.ok(host.body.children.includes(result.rootEl), 'the chrome is mounted on <body>');

  const beforeDock = host.invokeCalls.length;
  result.dockButtonEl._fire('click');
  assert.deepStrictEqual(plain(host.invokeCalls.slice(beforeDock)), [
    { cmd: 'dock_panel_host', args: { toolWindowId: 'ssh-sessions' } },
  ], 'the dock-back button is the host-caller side of dock_panel_host');

  // The mount: the requested registration's renderFn gets the content root,
  // in the same element pair a docked panel would have been given.
  assert.strictEqual(result.panelEl.className, 'tool-window-content');
  assert.strictEqual(result.panelEl.dataset.toolWindow, 'ssh-sessions');
  assert.strictEqual(result.renderRootEl.className, 'tool-window-scroll-viewport');
  assert.strictEqual(result.renderRootEl.parentNode, result.panelEl);
  assert.strictEqual(result.panelEl.parentNode, result.contentRootEl);
  const sshInit = host.panelInits.find((p) => p.name === 'ssh-sessions');
  assert.ok(sshInit, 'the ssh-sessions renderFn ran');
  assert.strictEqual(sshInit.opts.panelEl.parentNode, result.renderRootEl,
    'renderFn rendered into the host content root, not into a zone');

  // Ordering: ready comes AFTER the mount, so the window is never shown empty.
  const readyIdx = host.timeline.indexOf('invoke:panel_host_ready');
  const renderIdx = host.timeline.indexOf('render:ssh-sessions');
  assert.ok(readyIdx >= 0, 'panel_host_ready must be called');
  assert.ok(renderIdx >= 0 && renderIdx < readyIdx,
    'panel_host_ready must come after the panel is mounted');

  // Appearance mirrors settings.html: appearance resolves first, the theme
  // fetch carries the resolved value, and a config-changed sync is installed.
  assert.deepStrictEqual(plain(host.appearanceApplies), ['light']);
  assert.strictEqual(host.uiConfigApplies.length, 1,
    'applyUiConfig runs, as it does in settings.html');
  assert.strictEqual(host.themeCssApplies.length, 1);
  const themeCall = host.invokeCalls.find((c) => c.cmd === 'GET_THEME_COLORS');
  assert.deepStrictEqual(plain(themeCall.args), { resolvedAppearance: 'light' });
  assert.strictEqual(host.appearanceSyncs.length, 1);
  assert.strictEqual(host.appearanceSyncs[0].applyUiConfig, true);
  assert.strictEqual(host.appearanceSyncs[0].listen, host.bootDeps.listenOnCurrentWindow,
    'the sync listens on THIS window, like every other secondary window');
}

// --- 20b. macOS: no window-controls cluster is built at all ---------------
// The default `GET_APP_CONFIG` answer in makeHostSandbox already reports
// `platform: 'macos'` (native traffic lights), so scenario 20's result is
// reused here to pin the gate's negative side explicitly rather than only
// implying it.
{
  const host = makeHostSandbox();
  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(result.windowControlsEl, null,
    'macOS must never get the cluster — native decorations already cover it');
  assert.strictEqual(result.minimizeButtonEl, null);
  assert.strictEqual(result.maximizeButtonEl, null);
  assert.strictEqual(result.closeButtonEl, null);
}

// --- 20c. Windows: the cluster renders, and each button is wired ----------
{
  const host = makeHostSandbox({
    answers: { GET_APP_CONFIG: () => Promise.resolve({ appearance_mode: 'light', platform: 'windows' }) },
  });
  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.ok(result.windowControlsEl, 'Windows must get the custom cluster (no native decorations there)');
  assert.ok(host.body.children.includes(result.rootEl), 'the cluster ships inside the same chrome root');
  assert.ok(result.headerEl.children.includes(result.windowControlsEl),
    'the cluster lives in the header, alongside the dock action');

  result.minimizeButtonEl._fire('click');
  assert.deepStrictEqual(host.windowControlCalls, ['minimize']);

  result.maximizeButtonEl._fire('click');
  await tick();
  assert.deepStrictEqual(host.windowControlCalls, ['minimize', 'maximize'],
    'not maximized yet, so the maximize button maximizes');

  result.maximizeButtonEl._fire('click');
  await tick();
  assert.deepStrictEqual(host.windowControlCalls, ['minimize', 'maximize', 'unmaximize'],
    'maximized already, so the same button un-maximizes — the settings.html toggle pattern');

  // Close does NOT call the host's own close/abort helpers — it goes through
  // `currentWindow.close()`, the SAME path a native close button would take,
  // which src/panel_host.rs's CloseRequested hook converts into a hide.
  const closeCallsBefore = host.closeCalls.length;
  result.closeButtonEl._fire('click');
  assert.strictEqual(host.closeCalls.length, closeCallsBefore + 1,
    'the cluster close button routes through the normal window-close path, not a host-specific one');
  assert.deepStrictEqual(host.invokeCalls.filter((c) => c.cmd === 'dock_panel_host'), [],
    'closing via the cluster must not also dock — those are two different affordances');
}

// --- 20d. Linux gets the same cluster as Windows ---------------------------
{
  const host = makeHostSandbox({
    answers: { GET_APP_CONFIG: () => Promise.resolve({ appearance_mode: 'light', platform: 'linux' }) },
  });
  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.ok(result.windowControlsEl, 'Linux must get the cluster too — same custom-titlebar gate as Windows');
}

// --- 21. plugin-widgets is initialized WITHOUT terminal callbacks --------
{
  const host = makeHostSandbox();
  await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(host.pluginWidgetsInits.length, 1);
  const opts = host.pluginWidgetsInits[0];
  assert.strictEqual(typeof opts.invoke, 'function');
  assert.strictEqual(typeof opts.listen, 'function');
  for (const key of ['createTab', 'renameActiveTab', 'renameTabById', 'focusTabById', 'writeToActivePty']) {
    assert.strictEqual(opts[key], undefined,
      `${key} must be absent so a plugin's tab/pty actions are inert in a host`);
  }
}

// --- 22. Registrations only: all five built-ins, no zone/strip DOM --------
{
  const host = makeHostSandbox();
  await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);
  const twm = host.sandbox.toolWindowManager;

  for (const id of ['file-explorer', 'transfer-center', 'ssh-sessions', 'tunnels', 'notifications']) {
    assert.ok(twm.getRegistration(id), `${id} must be registered in a host too`);
  }
  // toolWindowManager.init is a throwing canary in this harness, so simply
  // getting here proves the host never wired the manager to the DOM. These
  // assert the visible consequence.
  for (const [zoneName, zoneEl] of host.zoneEls) {
    assert.strictEqual(zoneEl._contentEl.children.length, 0,
      `zone ${zoneName} must stay empty — a host builds no zones`);
  }
  for (const stripId of ['left-strip', 'right-strip', 'bottom-strip']) {
    assert.strictEqual(host.byId.get(stripId).children.length, 0,
      `${stripId} must stay empty — a host builds no rails`);
  }
  // Only the ONE requested panel rendered; the other four registrations are
  // inert bookkeeping.
  assert.deepStrictEqual(host.panelInits.map((p) => p.name), ['ssh-sessions']);

  // The plugin half of the registration pass: the replay plus both live
  // subscriptions.
  assert.ok(host.invokeCalls.some((c) => c.cmd === 'get_plugin_panels'),
    'already-loaded plugin panels are replayed');
  assert.ok(host.listens.app.has('plugin-panel-registered'));
  assert.ok(host.listens.app.has('plugin-panels-removed'));
}

// --- 22b. A host starts and mounts the same Transfer Center registration --
{
  const host = makeHostSandbox({
    answers: {
      get_panel_host_request: () => Promise.resolve({
        reqId: 8,
        toolWindowId: 'transfer-center',
        parentLabel: 'window-1',
        title: 'Transfers',
      }),
    },
  });
  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(result.status, 'mounted');
  assert.strictEqual(host.transferRuntimeStarts.length, 1,
    'the host window starts its own idempotent transfer projection exactly once');
  assert.strictEqual(host.transferRuntimeStarts[0].listen, host.bootDeps.listenOnCurrentWindow,
    'host transfer events listen on that host window, not app-global');
  assert.deepStrictEqual(host.panelInits.map((item) => item.name), ['transfer-center']);
  const init = host.panelInits[0].opts;
  assert.strictEqual(init.panelEl.id, 'transfer-center-panel');
  assert.strictEqual(init.panelEl.parentNode, result.renderRootEl,
    'the shared renderFn mounts into the panel host content root');
  host.fireWindowLifecycle('beforeunload');
  assert.strictEqual(host.panelDestroys.length, 1,
    'destroying the host window must dispose its mounted panel controller');
  host.fireWindowLifecycle('beforeunload');
  assert.strictEqual(host.panelDestroys.length, 1,
    'host mount disposal is exactly once even if lifecycle delivery repeats');
}

// --- 23. A plugin panel that is already loaded registers and can mount ---
{
  const host = makeHostSandbox({
    answers: {
      get_panel_host_request: () => Promise.resolve({
        reqId: 9,
        toolWindowId: 'plugin:demo',
        parentLabel: 'window-1',
        title: 'Demo',
      }),
      get_plugin_panels: () => Promise.resolve([
        { handle: 'h1', plugin_name: 'demo', panel_name: 'Demo', location: 'right' },
      ]),
      request_plugin_render: () => Promise.resolve('[]'),
    },
  });
  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(result.status, 'mounted',
    'the replay must finish BEFORE the registration lookup, or a plugin host aborts');
  assert.strictEqual(result.titleEl.textContent, 'Demo');
  assert.strictEqual(result.panelEl.dataset.toolWindow, 'plugin:demo');
}

// --- 23b. A host registers NO keyboard handlers, plugins or not ----------
// The brief's scoped-keys constraint, pinned where it actually leaked: the
// plugin registration path refreshes the app titlebar, and an uninitialized
// titlebar's refresh() still publishes ~18 app-menu accelerators (Cmd/Ctrl+W
// among them) at router priority 115 with a null action handler. In a main
// window shortcut-runtime outranks them at 120; a host has no shortcut-runtime
// at all, so they would win outright and turn those combos into dead keys.
{
  const host = makeHostSandbox({
    answers: {
      get_plugin_panels: () => Promise.resolve([
        { handle: 'h1', plugin_name: 'demo', panel_name: 'Demo', location: 'right' },
      ]),
      request_plugin_render: () => Promise.resolve('[]'),
    },
  });
  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(result.status, 'mounted');
  assert.ok(host.sandbox.toolWindowManager.getRegistration('plugin:demo'),
    'precondition: the plugin panel really did register, so the path was taken');
  assert.deepStrictEqual(host.titlebarRefreshes, [],
    'a host has no app titlebar — refreshing one only installs a dead menu table');
  assert.deepStrictEqual(plain(host.routerRegistrations), [],
    'ZERO keyboard-router handlers in a panel host');
}

// --- 24. Unknown tool-window id: abort, do not close ---------------------
{
  const host = makeHostSandbox({
    answers: {
      get_panel_host_request: () => Promise.resolve({
        reqId: 3,
        toolWindowId: 'plugin:gone',
        parentLabel: 'window-1',
        title: 'Gone',
      }),
    },
  });
  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(result.status, 'aborted');
  assert.ok(host.invokeCalls.some((c) => c.cmd === 'abort_panel_host'),
    'a host with no registration to mount aborts itself');
  assert.deepStrictEqual(host.closeCalls, [],
    'abort_panel_host destroys the window — a plain close() would be '
    + 'intercepted into a hide for a REGISTERED host');
  assert.strictEqual(host.body.children.length, 0, 'no chrome is built for an abort');
}

// --- 25. No pending request: abort best-effort, then close ---------------
{
  const host = makeHostSandbox({
    answers: {
      get_panel_host_request: () => Promise.reject(new Error('no pending panel host request')),
      // No entry exists, so the abort fails too — the close fallback is what
      // actually gets an UNREGISTERED host off the screen.
      abort_panel_host: () => Promise.reject(new Error('no panel host entry for this window')),
    },
  });
  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(result.status, 'no-request');
  assert.deepStrictEqual(host.invokeCalls.map((c) => c.cmd),
    ['get_panel_host_request', 'abort_panel_host'],
    'nothing else is booted when there is no request to host');
  assert.strictEqual(host.closeCalls.length, 1, 'the window closes itself');
  assert.deepStrictEqual(host.pluginWidgetsInits, []);
  assert.strictEqual(host.body.classList.contains('tl-panelhost-window'), false);
}

// --- 26. panel-host-event is subscribed, and boot() installs the real ------
// dispatcher in the same tick (Task 5) — so in a genuine boot the pending
// queue is never actually the path taken. The queue itself (receive-before-
// a-sink-exists) is still real, general-purpose machinery: scenario 26b
// exercises it directly, independent of boot's own wiring choice, so a
// future boot that installs its sink later (or not at all) cannot silently
// stop being covered.
{
  const host = makeHostSandbox();
  await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);
  const runtime = host.sandbox.termlabPanelHostRuntime;

  assert.ok(host.listens.window.has('panel-host-event'),
    'the broadcast lands on THIS window (emit_to the host label), not app-wide');
  assert.deepStrictEqual(plain(runtime.getPendingEvents()), [],
    'boot() already installed a live sink — nothing is left pending after it resolves');

  host.emitWindow('panel-host-event', { event: 'active-pane-changed', payload: { type: 'local', paneId: 3 } });
  assert.deepStrictEqual(plain(host.filesPanelOnTabChangedCalls), [{ type: 'local', paneId: 3 }],
    'a known event reaches filesPanel.onTabChanged immediately — no queuing needed once booted');
  assert.deepStrictEqual(plain(runtime.getPendingEvents()), []);
}

// --- 26b. The raw seam (receive-before-a-sink-exists) still queues --------
// Same mechanism scenario 26 used to exercise via boot() before Task 5
// existed; pinned directly against the module now that boot() no longer
// leaves the gap open in practice.
{
  const host = makeHostSandbox();
  const runtime = host.sandbox.termlabPanelHostRuntime;

  host.emitWindow('panel-host-event', { event: 'config-changed', payload: { a: 1 } });
  assert.deepStrictEqual(plain(runtime.getPendingEvents()), [],
    'nothing is listening yet — boot() has not even run, so this is a precondition check');

  await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);
  // boot() itself installs a sink synchronously, so by the time it resolves
  // there is nothing left pending; the queue's drain-in-order behaviour is
  // what scenario 26 already demonstrates for the real dispatcher. This
  // scenario instead pins the primitive directly, bypassing boot(), so the
  // seam stays covered even if boot's own wiring choice ever changes.
  runtime.setEventSink(null);
  const seen = [];
  host.emitWindow('panel-host-event', { event: 'queued-one', payload: { x: 1 } });
  host.emitWindow('panel-host-event', { event: 'queued-two', payload: { x: 2 } });
  assert.deepStrictEqual(plain(runtime.getPendingEvents()), [
    { event: 'queued-one', payload: { x: 1 } },
    { event: 'queued-two', payload: { x: 2 } },
  ], 'events arriving with no sink installed are queued, not dropped');

  runtime.setEventSink((p) => seen.push(p));
  assert.deepStrictEqual(plain(seen), [
    { event: 'queued-one', payload: { x: 1 } },
    { event: 'queued-two', payload: { x: 2 } },
  ], 'installing a sink drains the queue in arrival order');
  assert.deepStrictEqual(plain(runtime.getPendingEvents()), []);

  host.emitWindow('panel-host-event', { event: 'queued-three', payload: {} });
  assert.strictEqual(seen.length, 3, 'and later events go straight through');
}

// --- 26c. Composite: request.companionIds mounts main + companion behind --
// a divider, and BOTH disposers run on teardown. Registered as fresh,
// harness-owned tool windows (rather than reusing the real 'file-explorer'/
// 'transfer-center' registrations) so the disposer assertion is meaningful:
// the real file-explorer renderFn (tool-window-runtime.js) never returns a
// disposer at all — that is a pre-existing property of that registration,
// unrelated to what this scenario is pinning, which is boot()'s OWN
// mount/dispose wiring for a companion pair.
{
  const host = makeHostSandbox({
    answers: {
      get_panel_host_request: () => Promise.resolve({
        reqId: 9,
        toolWindowId: 'composite-main',
        parentLabel: 'window-1',
        title: 'SFTP',
        companionIds: ['composite-companion'],
      }),
    },
  });
  const renderCalls = {};
  const disposeCalls = {};
  const bump = (table, key) => { table[key] = (table[key] || 0) + 1; };
  host.sandbox.toolWindowManager.register('composite-main', {
    title: 'Main',
    renderFn: () => {
      bump(renderCalls, 'composite-main');
      return () => bump(disposeCalls, 'composite-main');
    },
  });
  host.sandbox.toolWindowManager.register('composite-companion', {
    title: 'Companion',
    renderFn: () => {
      bump(renderCalls, 'composite-companion');
      return () => bump(disposeCalls, 'composite-companion');
    },
  });

  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(result.status, 'mounted');
  assert.strictEqual(renderCalls['composite-main'], 1, 'main tool rendered');
  assert.strictEqual(renderCalls['composite-companion'], 1, 'companion rendered');
  assert.ok(host.query('panel-host-bottom'), 'bottom section exists');
  assert.ok(host.query('panel-host-divider'), 'divider exists');

  host.fireWindowLifecycle('beforeunload');
  assert.strictEqual(disposeCalls['composite-main'], 1, 'main disposer ran');
  assert.strictEqual(disposeCalls['composite-companion'], 1, 'companion disposer ran');
  host.fireWindowLifecycle('beforeunload');
  assert.strictEqual(disposeCalls['composite-main'], 1, 'main disposal is exactly once');
  assert.strictEqual(disposeCalls['composite-companion'], 1, 'companion disposal is exactly once');
}

// --- 26d. Degrade: an unresolvable companion id falls back to the solo -----
// layout — the main tool still mounts, but no bottom section is built.
{
  const host = makeHostSandbox({
    answers: {
      get_panel_host_request: () => Promise.resolve({
        reqId: 10,
        toolWindowId: 'solo-main',
        parentLabel: 'window-1',
        title: 'SFTP',
        companionIds: ['ghost-tool'],
      }),
    },
  });
  const renderCalls = {};
  host.sandbox.toolWindowManager.register('solo-main', {
    title: 'Solo',
    renderFn: () => { renderCalls['solo-main'] = (renderCalls['solo-main'] || 0) + 1; },
  });

  const result = await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.strictEqual(result.status, 'mounted');
  assert.strictEqual(renderCalls['solo-main'], 1, 'main tool still mounts');
  assert.ok(!host.query('panel-host-bottom'), 'no bottom section for an unresolvable companion');
  assert.ok(!host.query('panel-host-divider'), 'no divider either');
}

// ===========================================================================
// Part 5 — the parent-state event bridge (Task 5)
// ===========================================================================

// --- 27. BRIDGE_EVENTS is the single source; publish relays a listed event
{
  const sandbox = { console };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(BRIDGE_PATH, 'utf8'), sandbox, { filename: BRIDGE_PATH });

  assert.deepStrictEqual(Array.from(sandbox.termlabPanelHostBridge.BRIDGE_EVENTS), ['active-pane-changed']);
  assert.strictEqual(sandbox.termlabPanelHostBridge.ACTIVE_PANE_CHANGED_EVENT, 'active-pane-changed');

  const invokeCalls = [];
  const bridge = sandbox.termlabPanelHostBridge.create({
    invoke: (cmd, args) => { invokeCalls.push({ cmd, args }); return Promise.resolve(); },
  });

  bridge.publish('active-pane-changed', { type: 'ssh', spawned: true, paneId: 3 });
  assert.deepStrictEqual(plain(invokeCalls), [
    {
      cmd: 'panel_host_broadcast',
      args: { event: 'active-pane-changed', payload: { type: 'ssh', spawned: true, paneId: 3 } },
    },
  ], 'a listed event relays through panel_host_broadcast with the exact payload');
}

// --- 28. publish THROWS synchronously on an unlisted event name -----------
{
  const sandbox = { console };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(BRIDGE_PATH, 'utf8'), sandbox, { filename: BRIDGE_PATH });

  const invokeCalls = [];
  const bridge = sandbox.termlabPanelHostBridge.create({
    invoke: (cmd, args) => { invokeCalls.push({ cmd, args }); return Promise.resolve(); },
  });

  assert.throws(() => bridge.publish('some-other-event', {}), /unlisted event/);
  assert.deepStrictEqual(invokeCalls, [], 'a rejected publish must never reach invoke');
}

// --- 29/30/30b. Both parent call sites in manager-compose-runtime.js publish
// Scenario 29 drives paneManager's onTerminalFocused (always a PANE-shaped
// target: paneId, type, spawned — pane-manager.js). Scenario 30 drives
// tabManager's onTabChanged with a PANE-shaped target too — this is the
// COMMON case reaching that delegate: tab-manager.js:384's
// `onTabChanged(pane || tab)` resolves a real pane on every ordinary tab
// switch, and tab-manager.js:848 (post-SSH-connect) always passes a pane.
// Scenario 30b drives the same delegate with a TAB-shaped target, the
// narrower fallback tab-manager.js:384 takes only when a tab has no live
// focused pane (e.g. an empty tab) — this used to be the ONLY shape tested
// here, mischaracterized as tabManager's shape rather than its fallback.
// All three must still call filesPanel.onTabChanged with the RAW target
// (unchanged docked behaviour) and ALSO publish the primitives to the
// bridge.
function loadManagerComposeRuntime() {
  const invokeCalls = [];
  const filesPanelCalls = [];
  let paneManagerDeps = null;
  let tabManagerDeps = null;

  const sandbox = { console, setTimeout, clearTimeout, Promise, Math };
  sandbox.window = sandbox;
  sandbox.global = sandbox;

  sandbox.filesPanel = { onTabChanged: (target) => { filesPanelCalls.push(target); } };
  // Stand-ins for the real pane/tab managers: manager-compose-runtime.js only
  // needs to hand them an options bag and get something back with a truthy
  // shape — the options bag itself is exactly what this harness is after.
  sandbox.termlabPaneManager = { create: (deps) => { paneManagerDeps = deps; return {}; } };
  sandbox.termlabTabManager = { create: (deps) => { tabManagerDeps = deps; return {}; } };

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(BRIDGE_PATH, 'utf8'), sandbox, { filename: BRIDGE_PATH });
  vm.runInContext(fs.readFileSync(MANAGER_COMPOSE_PATH, 'utf8'), sandbox, { filename: MANAGER_COMPOSE_PATH });

  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    return Promise.resolve(undefined);
  };

  sandbox.termlabManagerComposeRuntime.create({
    invoke,
    tabs: new Map(),
    panes: new Map(),
  });

  return { paneManagerDeps, tabManagerDeps, invokeCalls, filesPanelCalls };
}

function broadcastCalls(invokeCalls) {
  return plain(invokeCalls.filter((c) => c.cmd === 'panel_host_broadcast'));
}

{
  const { paneManagerDeps, tabManagerDeps, invokeCalls, filesPanelCalls } = loadManagerComposeRuntime();
  assert.strictEqual(typeof paneManagerDeps.onTerminalFocused, 'function');
  assert.strictEqual(typeof tabManagerDeps.onTabChanged, 'function');

  // Scenario 29: a PANE-shaped target.
  const fakePane = {
    paneId: 7, tabId: 't1', kind: 'terminal', type: 'local', spawned: true, term: { fake: true },
  };
  paneManagerDeps.onTerminalFocused(7, fakePane);

  assert.deepStrictEqual(filesPanelCalls, [fakePane],
    'filesPanel.onTabChanged must still receive the RAW pane object, unchanged');
  assert.deepStrictEqual(broadcastCalls(invokeCalls), [
    { cmd: 'panel_host_broadcast', args: { event: 'active-pane-changed', payload: { type: 'local', spawned: true, paneId: 7 } } },
  ], 'onTerminalFocused must ALSO publish, extracting only the primitives the pane carries');

  // Scenario 30: tabManager's onTabChanged, PANE-shaped target — the common
  // case (tab-manager.js:384's `pane || tab` when the tab has a live focused
  // pane, and tab-manager.js:848 which is always a pane).
  invokeCalls.length = 0;
  const fakeTabPane = {
    paneId: 21, tabId: 't2', kind: 'terminal', type: 'ssh', spawned: true, term: { fake: true },
  };
  tabManagerDeps.onTabChanged(fakeTabPane);

  assert.deepStrictEqual(filesPanelCalls, [fakePane, fakeTabPane],
    'the second call site must also still hand filesPanel the raw target');
  assert.deepStrictEqual(broadcastCalls(invokeCalls), [
    { cmd: 'panel_host_broadcast', args: { event: 'active-pane-changed', payload: { type: 'ssh', spawned: true, paneId: 21 } } },
  ], 'onTabChanged must publish a PANE target\'s primitives the same way onTerminalFocused does '
    + '(type/spawned/paneId, no id/focusedPaneId)');

  // Scenario 30b: tabManager's onTabChanged, TAB-shaped target — the
  // narrower fallback tab-manager.js:384 takes only when a tab has no live
  // focused pane.
  invokeCalls.length = 0;
  const fakeTab = {
    id: 'tab-9', type: 'ssh', focusedPaneId: 42, label: 'demo', button: {}, containerEl: {},
  };
  tabManagerDeps.onTabChanged(fakeTab);

  assert.deepStrictEqual(filesPanelCalls, [fakePane, fakeTabPane, fakeTab],
    'the fallback tab-shaped call must also still hand filesPanel the raw target');
  assert.deepStrictEqual(broadcastCalls(invokeCalls), [
    { cmd: 'panel_host_broadcast', args: { event: 'active-pane-changed', payload: { type: 'ssh', focusedPaneId: 42, id: 'tab-9' } } },
  ], 'the fallback must publish the TAB shape\'s primitives (id/focusedPaneId, no paneId/spawned)');
}

// --- 31. Host re-dispatch: active-pane-changed reaches filesPanel.onTabChanged
{
  const host = makeHostSandbox();
  await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  const payload = { type: 'ssh', spawned: true, paneId: 11 };
  host.emitWindow('panel-host-event', { event: 'active-pane-changed', payload });

  assert.deepStrictEqual(plain(host.filesPanelOnTabChangedCalls), [payload],
    'the SAME callback interface a docked panel gets (filesPanel.onTabChanged) must fire in a host too');
  assert.deepStrictEqual(host.warnCalls, [], 'a known event must not warn');
}

// --- 32. Host re-dispatch: an unlisted event warns and continues ----------
{
  const host = makeHostSandbox();
  await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);

  assert.doesNotThrow(() => {
    host.emitWindow('panel-host-event', { event: 'some-future-event', payload: { x: 1 } });
  }, 'a version-skewed parent broadcasting an unknown event must never throw in the host');

  assert.deepStrictEqual(host.filesPanelOnTabChangedCalls, [],
    'an unlisted event must not reach filesPanel');
  assert.strictEqual(host.warnCalls.length, 1, 'exactly one warning for the one unlisted event');
  assert.deepStrictEqual(host.warnCalls[0], ['panel host: ignoring unlisted panel-host-event', 'some-future-event']);
}

// --- 33. End-to-end chain: a popped SFTP panel issues sftp_* with the -----
// PARENT's active pane id. Loads the REAL files-panel.js (plus its real
// data-service.js and pane-store.js) — not a stub — so this is genuine
// evidence that Task 5's payload reaches Task 1/2's session resolver with
// the right pane identity, not just that a callback fired.
//
// crates/termlab_tauri/src/remote/sftp_commands.rs's sftp_list_dir (and
// sftp_realpath) resolve the caller's session key from `window.label()`
// through `session_caller_label` -> `effective_session_window_label`
// (Task 2): a panelhost-* window's own `invoke` resolves to its PARENT's
// label there. So all this harness has to prove on the frontend side is
// that the paneId this payload carries is the one that ends up on the
// sftp_realpath / sftp_list_dir invoke — Rust does the rest with the
// caller's real (host) window label, which the parent-keyed session already
// matches.
{
  const invokeCalls = [];
  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    if (cmd === 'sftp_realpath') return Promise.resolve('/home/demo');
    if (cmd === 'sftp_list_dir') return Promise.resolve([]);
    if (cmd === 'get_home_dir') return Promise.resolve('/home/demo');
    if (cmd === 'get_all_settings') return Promise.resolve({});
    if (cmd === 'local_list_dir') return Promise.resolve([]);
    return Promise.resolve(undefined);
  };

  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    // Stubbed, not real: files-panel.js's cwd polling would otherwise leave
    // live intervals running against this sandbox after the assertions
    // below finish, which is irrelevant to this test's question (does the
    // right paneId reach sftp_realpath/sftp_list_dir?) and would keep the
    // test process alive.
    setInterval: () => 0,
    clearInterval: () => {},
    Promise,
    Math,
    Array,
    JSON,
    Object,
    String,
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  sandbox.utils = { formatSize: () => '' };
  sandbox.toast = { error() {}, info() {} };
  sandbox.toolWindowManager = { isVisible: () => true, activate() {}, deactivate() {} };
  // The panel's own DOM rendering is irrelevant to whether the right
  // sftp_* invoke fires, so it is stubbed out rather than faked with a real
  // parser this harness does not have.
  sandbox.termlabFilesPaneView = { renderPane: () => {}, showColumnMenu: () => {}, showRowContextMenu: () => {} };
  sandbox.termlabFilesActions = {};
  sandbox.termlabFilesTransfers = {};

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(FILES_PANE_STORE_PATH, 'utf8'), sandbox, { filename: FILES_PANE_STORE_PATH });
  vm.runInContext(fs.readFileSync(FILES_DATA_SERVICE_PATH, 'utf8'), sandbox, { filename: FILES_DATA_SERVICE_PATH });
  vm.runInContext(fs.readFileSync(FILES_PANEL_PATH, 'utf8'), sandbox, { filename: FILES_PANEL_PATH });

  const panelEl = makeElement('div');
  const localRootEl = makeElement('div');
  const remoteRootEl = makeElement('div');
  panelEl.querySelector = (sel) => {
    if (sel === '#fp-local') return localRootEl;
    if (sel === '#fp-remote') return remoteRootEl;
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
  });
  await tick();
  invokeCalls.length = 0; // discard the local-pane boot traffic (getHomeDir, settings, local_list_dir)

  // This IS the payload shape Task 5's bridge/host would deliver: a live SSH
  // pane the parent just focused, spawned, carrying only paneId (the
  // PANE-shaped call site — onTerminalFocused).
  await sandbox.filesPanel.onTabChanged({ type: 'ssh', spawned: true, paneId: 11 });
  await tick();

  const realpathCall = invokeCalls.find((c) => c.cmd === 'sftp_realpath');
  const listDirCall = invokeCalls.find((c) => c.cmd === 'sftp_list_dir');
  assert.ok(realpathCall, 'onTabChanged must resolve the remote cwd via sftp_realpath');
  assert.strictEqual(realpathCall.args.paneId, 11,
    'sftp_realpath must carry the PARENT-supplied paneId from the bridge payload');
  assert.ok(listDirCall, 'onTabChanged must then list the remote directory via sftp_list_dir');
  assert.strictEqual(listDirCall.args.paneId, 11,
    'sftp_list_dir must carry the same paneId — this is what a popped-out SFTP '
    + 'panel needs so Rust\'s session_caller_label resolver (Task 2) finds the '
    + 'PARENT\'s session for that pane, not a session keyed to the host window '
    + 'itself (which has none)');
}

// ===========================================================================
// Part 6 — open-in-editor from a popped-out host
//
// The bug this closes: double-clicking a file in a POPPED-OUT SFTP/Files
// panel used to reach files-panel.js's openInEditor exactly as if the panel
// were docked, but a host window has no editor of its own —
// editor-service.js's createEditorTab throws unless manager-compose-
// runtime.js has composed THIS window, which it never does for a host (see
// files-panel.js's own comment on the new `__termlabCreateEditorTab` gate).
//
// Six scenarios: a host with no editor publishes instead of calling a
// nonexistent one, for both arg shapes (34 local, 35 remote); a composed
// (main) window's existing direct-call path stays byte-identical and NEVER
// publishes (36); the parent's `panel-host-action` router replays those
// SAME editor calls, fixture-compared against scenario 36's direct calls so
// the two paths cannot silently drift apart (37 local, 38 remote); and an
// unlisted action warns and is dropped at the parent, mirroring the forward
// bridge's own rule (39). Scenario 40 pins publishAction's own contract
// (HOST_ACTION_EVENTS is the source of truth; throws synchronously on an
// unlisted name) — the reverse-direction twin of scenarios 27/28.
// ===========================================================================

const settle6 = async (times = 4) => { for (let i = 0; i < times; i += 1) await tick(); };

// Builds a sandbox with the REAL pane-store/data-service/actions/bridge/
// panel modules — no reimplementation, same idiom as scenario 33 — with
// `composed` controlling whether this window looks like a main window
// (`window.__termlabCreateEditorTab` set, standing in for manager-compose-
// runtime.js having run in it) or a panel host (left unset).
// `editorSpies`, when given, stands in for window.termlabEditorService.
function makeOpenInEditorSandbox({ composed, editorSpies } = {}) {
  const invokeCalls = [];
  const toastErrors = [];
  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    if (cmd === 'get_home_dir') return Promise.resolve('/home/demo');
    if (cmd === 'get_all_settings') return Promise.resolve({});
    if (cmd === 'local_list_dir') return Promise.resolve([]);
    if (cmd === 'sftp_realpath') return Promise.resolve('/home/demo');
    if (cmd === 'sftp_list_dir') return Promise.resolve([]);
    if (cmd === 'current_window_label') return Promise.resolve('main');
    if (cmd === 'remote_get_sessions') {
      return Promise.resolve([
        { key: 'main:1000007', host: 'build.example.com', user: 'alice', port: 22 },
      ]);
    }
    if (cmd === 'panel_host_action') return Promise.resolve();
    return Promise.resolve(undefined);
  };

  const renderCalls = [];
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    setInterval: () => 0,
    clearInterval: () => {},
    Promise,
    Math,
    Array,
    JSON,
    Object,
    String,
    Number,
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  sandbox.utils = { formatSize: () => '', formatDate: () => '', esc: (v) => String(v == null ? '' : v), attr: (v) => String(v == null ? '' : v) };
  sandbox.toast = { error: (...args) => { toastErrors.push(args); }, info() {}, warn() {}, success() {} };
  sandbox.toolWindowManager = { isVisible: () => true, activate() {}, deactivate() {} };
  sandbox.termlabFilesPaneView = {
    renderPane: (pane, el, deps) => { renderCalls.push({ pane, el, deps }); },
    showColumnMenu: () => {},
    showRowContextMenu: () => {},
  };
  sandbox.termlabFilesTransfers = {};
  if (composed) {
    sandbox.__termlabCreateEditorTab = () => {};
  }
  if (editorSpies) {
    sandbox.termlabEditorService = editorSpies;
  }

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(FILES_PANE_STORE_PATH, 'utf8'), sandbox, { filename: FILES_PANE_STORE_PATH });
  vm.runInContext(fs.readFileSync(FILES_DATA_SERVICE_PATH, 'utf8'), sandbox, { filename: FILES_DATA_SERVICE_PATH });
  vm.runInContext(fs.readFileSync(FILES_ACTIONS_PATH, 'utf8'), sandbox, { filename: FILES_ACTIONS_PATH });
  vm.runInContext(fs.readFileSync(BRIDGE_PATH, 'utf8'), sandbox, { filename: BRIDGE_PATH });
  vm.runInContext(fs.readFileSync(FILES_PANEL_PATH, 'utf8'), sandbox, { filename: FILES_PANEL_PATH });

  const panelEl = makeElement('div');
  const localRootEl = makeElement('div');
  const remoteRootEl = makeElement('div');
  panelEl.querySelector = (sel) => {
    if (sel === '#fp-local') return localRootEl;
    if (sel === '#fp-remote') return remoteRootEl;
    return null;
  };

  return { sandbox, invoke, invokeCalls, toastErrors, renderCalls, panelEl };
}

function lastPaneCall6(renderCalls, prefix) {
  for (let i = renderCalls.length - 1; i >= 0; i -= 1) {
    if (renderCalls[i].pane.prefix === prefix) return renderCalls[i];
  }
  throw new Error(`no ${prefix}-pane renderPane call recorded yet`);
}

function publishActionCalls(invokeCalls) {
  return plain(invokeCalls.filter((c) => c.cmd === 'panel_host_action'));
}

// --- 34. Host mode, LOCAL file: openInEditor publishes instead of calling --
// a nonexistent editor, and the "editor unavailable" toast never fires. ----
let directLocalArgs = null; // filled in by scenario 36, fixture-compared by 37
{
  const h = makeOpenInEditorSandbox({ composed: false });
  h.sandbox.filesPanel.init({
    invoke: h.invoke,
    panelEl: h.panelEl,
    panelWrapEl: makeElement('div'),
    resizeHandleEl: makeElement('div'),
    layoutService: null,
    fitActiveTab: () => {},
    getActiveTab: () => null,
  });
  await tick();
  h.invokeCalls.length = 0;

  const { deps } = lastPaneCall6(h.renderCalls, 'local');
  await deps.onActivateEntry({ name: 'notes.txt', is_dir: false, size: 42 });
  await settle6();

  assert.deepStrictEqual(publishActionCalls(h.invokeCalls), [
    { cmd: 'panel_host_action', args: { event: 'open-in-editor', payload: { kind: 'local', path: '/home/demo/notes.txt' } } },
  ], 'a host with no editor must publish open-in-editor with the local path');
  assert.deepStrictEqual(h.toastErrors, [], 'the "editor unavailable" toast must never fire on the publish path');
}

// --- 35. Host mode, REMOTE file: publishes with the full session-derived --
// field set (paneId, remotePath, hostLabel, size) — the same fields
// editor.openRemoteFile takes directly in scenario 36. ----------------------
let directRemoteArgs = null; // filled in by scenario 36, fixture-compared by 38
{
  const h = makeOpenInEditorSandbox({ composed: false });
  h.sandbox.filesPanel.init({
    invoke: h.invoke,
    panelEl: h.panelEl,
    panelWrapEl: makeElement('div'),
    resizeHandleEl: makeElement('div'),
    layoutService: null,
    fitActiveTab: () => {},
    getActiveTab: () => null,
  });
  await tick();

  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle6();
  h.invokeCalls.length = 0;

  const { deps } = lastPaneCall6(h.renderCalls, 'remote');
  await deps.onActivateEntry({ name: 'config.yml', is_dir: false, size: 777 });
  await settle6();

  assert.deepStrictEqual(publishActionCalls(h.invokeCalls), [
    {
      cmd: 'panel_host_action',
      args: {
        event: 'open-in-editor',
        payload: {
          kind: 'remote',
          paneId: 1000007,
          remotePath: '/home/demo/config.yml',
          hostLabel: 'alice@build.example.com',
          size: 777,
        },
      },
    },
  ], 'a host with no editor must publish open-in-editor with the full remote descriptor');
}

// --- 36. Main-window mode: the existing direct-call path is byte-unchanged
// (editor.openLocalFile/openRemoteFile called with today's args) and NEVER
// publishes — zero panel_host_action invokes, for both local and remote. ---
{
  const openLocalCalls = [];
  const openRemoteCalls = [];
  const h = makeOpenInEditorSandbox({
    composed: true,
    editorSpies: {
      openLocalFile: (p) => { openLocalCalls.push(p); },
      openRemoteFile: (descriptor) => { openRemoteCalls.push(descriptor); },
    },
  });
  h.sandbox.filesPanel.init({
    invoke: h.invoke,
    panelEl: h.panelEl,
    panelWrapEl: makeElement('div'),
    resizeHandleEl: makeElement('div'),
    layoutService: null,
    fitActiveTab: () => {},
    getActiveTab: () => null,
  });
  await tick();

  const { deps: localDeps } = lastPaneCall6(h.renderCalls, 'local');
  await localDeps.onActivateEntry({ name: 'notes.txt', is_dir: false, size: 42 });
  await settle6();

  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle6();

  const { deps: remoteDeps } = lastPaneCall6(h.renderCalls, 'remote');
  await remoteDeps.onActivateEntry({ name: 'config.yml', is_dir: false, size: 777 });
  await settle6();

  assert.deepStrictEqual(plain(openLocalCalls), ['/home/demo/notes.txt'],
    'a composed (main) window must still call editor.openLocalFile directly, unchanged');
  assert.deepStrictEqual(plain(openRemoteCalls), [
    { paneId: 1000007, remotePath: '/home/demo/config.yml', hostLabel: 'alice@build.example.com', size: 777 },
  ], "a composed (main) window must still call editor.openRemoteFile directly, with today's exact fields");
  assert.deepStrictEqual(publishActionCalls(h.invokeCalls), [],
    'the main-window path must never publish — zero panel_host_action calls');

  directLocalArgs = plain(openLocalCalls[0]);
  directRemoteArgs = plain(openRemoteCalls[0]);
}

// --- Part 6b — the parent's panel-host-action router (tool-window-runtime.js)
function makeFakeManager6() {
  return {
    isPanelOpen: () => true,
    isPanelVisible: () => true,
    getZoneAssignments: () => ({}),
    getActiveZoneAssignments: () => ({}),
    getViewModes: () => ({}),
    getSplitRatios: () => ({ left: 0.5, right: 0.5 }),
    getSidebarWidths: () => ({ left: 240, right: 300 }),
    init: () => {},
    setPersistedZones: () => {},
    setPersistedActiveZoneWindows: () => {},
    setPersistedPanelVisibility: () => {},
    setPersistedViewModes: () => {},
    setSidebarWidth: () => {},
    setSplitRatio: () => {},
    setPanelVisibility: () => {},
    register: () => {},
    summonPendingWindowHosts: () => {},
    notifyHostShown: () => {},
    notifyHostHidden: () => {},
    notifyHostDocked: () => {},
    notifyHostAborted: () => {},
  };
}

// Loads the REAL app/core/panel-host-bridge.js + app/tool-window-runtime.js,
// with `editorSpies` standing in for window.termlabEditorService — this is
// the PARENT half of the reverse bridge, so it is the module that reads
// that global directly (routePanelHostAction), same as every other in-window
// caller (vim-mode's :w, the (cmd)O chooser).
async function loadRuntimeWithEditor(editorSpies) {
  const listeners = new Map();
  const warnCalls = [];
  const elements = new Map([
    ['bottom-zone-wrap', makeElement('div')],
    ['bottom-zone-resize', makeElement('div')],
  ]);

  const sandbox = {
    console: { ...console, warn: (...args) => { warnCalls.push(args); } },
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
  sandbox.toolWindowManager = makeFakeManager6();
  sandbox.termlabEditorService = editorSpies;

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(BRIDGE_PATH, 'utf8'), sandbox, { filename: BRIDGE_PATH });
  vm.runInContext(fs.readFileSync(RUNTIME_PATH, 'utf8'), sandbox, { filename: RUNTIME_PATH });

  const runtime = sandbox.termlabToolWindowRuntime.create({
    invoke: () => Promise.resolve(undefined),
    listen: () => Promise.resolve(() => {}),
    listenOnCurrentWindow: (name, fn) => {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
      return Promise.resolve(() => {});
    },
    layoutService: {
      getSavedLayout: () => Promise.resolve({}),
      saveLayout: () => {},
    },
    debouncedFitAndResize: () => {},
    getCurrentTab: () => null,
    getCurrentPane: () => null,
  });
  await runtime.init();

  const emit = (name, payload) => {
    for (const fn of listeners.get(name) || []) fn({ payload });
  };
  return { emit, warnCalls };
}

// --- 37. Parent router: open-in-editor (local) reaches termlabEditorService
// with EXACTLY the args files-panel.js's OWN direct call used (scenario 36).
{
  assert.ok(directLocalArgs, 'scenario 36 must have run first and captured its direct-call fixture');
  const openLocalCalls = [];
  const { emit } = await loadRuntimeWithEditor({
    openLocalFile: (p) => { openLocalCalls.push(p); },
    openRemoteFile: () => {},
  });

  emit('panel-host-action', {
    toolWindowId: 'file-explorer',
    event: 'open-in-editor',
    payload: { kind: 'local', path: directLocalArgs },
  });

  assert.deepStrictEqual(plain(openLocalCalls), [directLocalArgs],
    'the parent route must call openLocalFile with EXACTLY the path the direct path used');
}

// --- 38. Parent router: open-in-editor (remote) fixture-compares byte-for- -
// byte against scenario 36's direct editor.openRemoteFile call. ------------
{
  assert.ok(directRemoteArgs, 'scenario 36 must have run first and captured its direct-call fixture');
  const openRemoteCalls = [];
  const { emit } = await loadRuntimeWithEditor({
    openLocalFile: () => {},
    openRemoteFile: (descriptor) => { openRemoteCalls.push(descriptor); },
  });

  emit('panel-host-action', {
    toolWindowId: 'file-explorer',
    event: 'open-in-editor',
    payload: { kind: 'remote', ...directRemoteArgs },
  });

  assert.deepStrictEqual(plain(openRemoteCalls), [directRemoteArgs],
    'the parent route must call openRemoteFile with EXACTLY the object the direct path used');
}

// --- 39. An unlisted panel-host-action warns and is dropped at the parent —
// never reaches the editor service (mirrors the forward bridge's own rule,
// scenario 32). ---------------------------------------------------------------
{
  const openLocalCalls = [];
  const openRemoteCalls = [];
  const { emit, warnCalls } = await loadRuntimeWithEditor({
    openLocalFile: (p) => { openLocalCalls.push(p); },
    openRemoteFile: (d) => { openRemoteCalls.push(d); },
  });

  assert.doesNotThrow(() => {
    emit('panel-host-action', { toolWindowId: 'file-explorer', event: 'some-future-action', payload: { x: 1 } });
  }, 'a version-skewed host asking for an unknown action must never throw in the parent');

  assert.deepStrictEqual(openLocalCalls, []);
  assert.deepStrictEqual(openRemoteCalls, []);
  assert.strictEqual(warnCalls.length, 1, 'exactly one warning for the one unlisted action');
  assert.deepStrictEqual(warnCalls[0], ['panel host: ignoring unlisted panel-host-action', 'some-future-action']);
}

// --- 40. HOST_ACTION_EVENTS is the single source; publishAction relays a --
// listed action with the exact payload, and throws synchronously on an
// unlisted one (the reverse-direction twin of scenarios 27/28). ------------
{
  const sandbox = { console };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(BRIDGE_PATH, 'utf8'), sandbox, { filename: BRIDGE_PATH });

  assert.deepStrictEqual(Array.from(sandbox.termlabPanelHostBridge.HOST_ACTION_EVENTS), ['open-in-editor']);

  const invokeCalls = [];
  const bridge = sandbox.termlabPanelHostBridge.create({
    invoke: (cmd, args) => { invokeCalls.push({ cmd, args }); return Promise.resolve(); },
  });

  bridge.publishAction('open-in-editor', { kind: 'local', path: '/tmp/x.txt' });
  assert.deepStrictEqual(plain(invokeCalls), [
    { cmd: 'panel_host_action', args: { event: 'open-in-editor', payload: { kind: 'local', path: '/tmp/x.txt' } } },
  ], 'a listed action relays through panel_host_action with the exact payload');

  assert.throws(() => bridge.publishAction('some-future-action', {}), /unlisted action/);
  assert.deepStrictEqual(publishActionCalls(invokeCalls), [
    { cmd: 'panel_host_action', args: { event: 'open-in-editor', payload: { kind: 'local', path: '/tmp/x.txt' } } },
  ], 'the rejected publishAction call must never reach invoke a second time');
}

// --- 41. Host boot threads currentWindow into the tool-window runtime -------
// tool-window-runtime.js derives the files panel's native drag-drop channel
// (onDragDropEvent) from deps.currentWindow. A host boot that omits it makes
// Finder-to-pane and pane-to-pane drops silently dead in every popped-out
// window — the regression this scenario pins.
{
  const host = makeHostSandbox();
  const realCreate = host.sandbox.termlabToolWindowRuntime.create;
  let recordedDeps = null;
  host.sandbox.termlabToolWindowRuntime.create = (deps) => {
    recordedDeps = deps;
    return realCreate(deps);
  };
  await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);
  assert.ok(recordedDeps, 'boot created the tool-window runtime');
  assert.strictEqual(
    recordedDeps.currentWindow,
    host.bootDeps.currentWindow,
    'the host window itself must reach the runtime, or native drag-drop never subscribes',
  );
}

console.log('panel host: all assertions passed');
