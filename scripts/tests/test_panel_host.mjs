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

// --- 14d. Unregistering a popped-out window takes its host with it ---------
// What a plugin removal does: the host would otherwise stay on screen with
// nothing to host.
{
  const { twm, invoke, invokeCalls } = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  twm.register('plugin:fake', { title: 'Fake', renderFn: () => {} });
  twm.setViewMode('plugin:fake', 'window');
  invokeCalls.length = 0;

  twm.unregister('plugin:fake');

  assert.deepStrictEqual(plain(invokeCalls), [
    { cmd: 'hide_panel_host', args: { toolWindowId: 'plugin:fake' } },
  ], 'an orphaned host must not be left on screen');
  assert.strictEqual(twm.getRegistration('plugin:fake'), null);
  assert.deepStrictEqual(plain(twm.getViewModes()), {},
    'the id\'s view-mode bookkeeping goes with the registration');
  assert.deepStrictEqual(Array.from(twm.getWindowsInZone('right-bottom')), []);
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

  const body = makeElement('body');
  const appEl = makeElement('div');
  const zoneEls = new Map();
  for (const z of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom']) {
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
  ]);

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
    addEventListener() {},
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;

  // The four built-in panels, stubbed at their init boundary so a render is
  // observable without dragging the real panels into the harness.
  const panelStub = (name) => ({
    init: (opts) => {
      timeline.push('render:' + name);
      panelInits.push({ name, opts });
    },
  });
  sandbox.filesPanel = panelStub('file-explorer');
  sandbox.sshPanel = panelStub('ssh-sessions');
  sandbox.tunnelsPanel = panelStub('tunnels');
  sandbox.notificationsPanel = panelStub('notifications');

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

  vm.createContext(sandbox);
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
  const currentWindow = { close: () => { closeCalls.push(1); timeline.push('close'); } };

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
    body,
    appEl,
    zoneEls,
    byId,
    emitWindow: (name, payload) => {
      for (const fn of listens.window.get(name) || []) fn({ payload });
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

// --- 22. Registrations only: all four built-ins, no zone/strip DOM -------
{
  const host = makeHostSandbox();
  await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);
  const twm = host.sandbox.toolWindowManager;

  for (const id of ['file-explorer', 'ssh-sessions', 'tunnels', 'notifications']) {
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
  // Only the ONE requested panel rendered; the other three registrations are
  // inert bookkeeping.
  assert.deepStrictEqual(host.panelInits.map((p) => p.name), ['ssh-sessions']);

  // The plugin half of the registration pass: the replay plus both live
  // subscriptions.
  assert.ok(host.invokeCalls.some((c) => c.cmd === 'get_plugin_panels'),
    'already-loaded plugin panels are replayed');
  assert.ok(host.listens.app.has('plugin-panel-registered'));
  assert.ok(host.listens.app.has('plugin-panels-removed'));
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

// --- 26. panel-host-event is subscribed and queued for Task 5 ------------
{
  const host = makeHostSandbox();
  await host.sandbox.termlabPanelHostRuntime.boot(host.bootDeps);
  const runtime = host.sandbox.termlabPanelHostRuntime;

  assert.ok(host.listens.window.has('panel-host-event'),
    'the broadcast lands on THIS window (emit_to the host label), not app-wide');

  host.emitWindow('panel-host-event', { event: 'config-changed', payload: { a: 1 } });
  assert.deepStrictEqual(plain(runtime.getPendingEvents()), [
    { event: 'config-changed', payload: { a: 1 } },
  ], 'an event arriving before Task 5 installs a sink is QUEUED, not dropped');

  const seen = [];
  runtime.setEventSink((p) => seen.push(p));
  assert.deepStrictEqual(plain(seen), [{ event: 'config-changed', payload: { a: 1 } }],
    'installing a sink drains the queue in arrival order');
  assert.deepStrictEqual(plain(runtime.getPendingEvents()), []);

  host.emitWindow('panel-host-event', { event: 'later', payload: {} });
  assert.strictEqual(seen.length, 2, 'and later events go straight through');
}

console.log('panel host: all assertions passed');
