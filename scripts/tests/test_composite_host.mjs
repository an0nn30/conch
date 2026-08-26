// Run: node scripts/tests/test_composite_host.mjs
//
// Composite panel host, Task 2: the parent-side companion suppression core.
//
// A "composite host" is a popped-out tool window (the SFTP file browser, say)
// that carries one or more COMPANION tool windows along for the ride inside
// its own OS window (the transfer center, say) rather than leaving them
// docked behind in the main window. While that ride is live the companion's
// docked presence must disappear — it has no DOM to show, the host is
// rendering it — and must come back exactly the way it left when the host
// closes.
//
// This suite pins the suppression bookkeeping in
// crates/termlab_tauri/frontend/app/layout/tool-window-manager.js:
// `companionSuppressions`, `isCompanionSuppressed`, `companionIdsFor`,
// `suppressCompanionsFor`/`liftCompanionsFor`, and `register()`'s
// `opts.companions` handling — WITHOUT any host-side (Task 4/5) or Rust-side
// (Task 1) code. `open_panel_host`'s `companionIds` argument is asserted on
// here only as what the manager SENDS; Task 1 already made Rust accept it.
//
// Harness: the same no-jsdom vm idiom as test_tool_window_closed_state.mjs
// (lines 39-95) and test_panel_host.mjs — document is stubbed just enough
// for the module to load and run for real. Deliberately does NOT set
// `sandbox.global`: the module's IIFE binds `exports`/`window`, never
// `global` (see test_dock_highlight.mjs's source guard) — a harness that
// defines `sandbox.global` would hide a bare `global.` regression.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/layout/tool-window-manager.js',
);

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
    removeEventListener() {},
    // Test-only: invoke every listener stored for `name` (scenario 8 needs to
    // simulate a real click on a strip button without a real DOM/event loop).
    fire(name, evt) {
      const fns = listeners.get(name) || [];
      const e = evt || { preventDefault() {}, stopPropagation() {} };
      fns.slice().forEach((fn) => fn(e));
    },
    click() { this.fire('click'); },
    setAttribute(n, v) { attrs.set(n, String(v)); },
    getAttribute(n) { return attrs.has(n) ? attrs.get(n) : null; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }; },
  };
  // updateStrips() clears each strip with `stripEl.innerHTML = ''` before
  // rebuilding it (same idiom test_bottom_zone_split.mjs's makeElement
  // mirrors) — without this, repeated updateStrips() calls (register,
  // activate, setViewMode, ...) would just keep appending duplicate sections.
  Object.defineProperty(el, 'innerHTML', {
    set(v) { if (v === '') this.children.length = 0; },
    get() { return ''; },
  });
  return el;
}

// A real zone element for `zoneName` so activate()'s ensureWindowElement()
// has somewhere to mount, and isVisible()/tw.el.style.display are observable.
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

// Load the module fresh for each scenario so zone state never leaks between
// them. Records every `panelHostInvoke` call — the manager's only channel to
// Rust — as { cmd, args }, resolving each with a fake, distinct reqId so the
// generation-token bookkeeping (markHostRequested/markHostOpened) never
// stalls waiting on a real Tauri round trip.
function loadManager() {
  const body = makeElement('body');
  const zoneEls = new Map();
  for (const z of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right']) {
    zoneEls.set(z, makeZoneEl(z));
  }

  const calls = { invokes: [] };
  let nextReqId = 1;
  const invoke = (cmd, args) => {
    calls.invokes.push({ cmd, args });
    return Promise.resolve(nextReqId++);
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
    tlMenu: { open: () => {} },
  };
  // Deliberately no `sandbox.global` — see the module comment above.
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

  const twm = sandbox.toolWindowManager;
  assert.ok(twm, 'tool-window-manager.js must expose window.toolWindowManager');
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  return { twm, calls, zoneEls };
}

// loadManager() plus real (stubbed) strip elements, the way
// test_bottom_zone_split.mjs's loadManagerWithBottomDom() adds a real
// 'bottom-strip' element to its base loader — needed here so updateStrips()
// actually populates strip-section children instead of bailing on a null
// stripEl, giving scenario 8 real button elements to click.
function loadManagerWithStrips() {
  const body = makeElement('body');
  const zoneEls = new Map();
  for (const z of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right']) {
    zoneEls.set(z, makeZoneEl(z));
  }
  const stripEls = {
    left: makeElement('div'),
    right: makeElement('div'),
    bottom: makeElement('div'),
  };
  const idEls = {
    'left-strip': stripEls.left,
    'right-strip': stripEls.right,
    'bottom-strip': stripEls.bottom,
  };

  const calls = { invokes: [] };
  let nextReqId = 1;
  const invoke = (cmd, args) => {
    calls.invokes.push({ cmd, args });
    return Promise.resolve(nextReqId++);
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
      getElementById: (id) => idEls[id] || null,
      querySelector: (sel) => {
        const m = /^\[data-zone="([^"]+)"\]$/.exec(sel);
        if (m) return zoneEls.get(m[1]) || null;
        return null;
      },
      querySelectorAll: () => [],
      addEventListener() {},
    },
    tlMenu: { open: () => {} },
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

  const twm = sandbox.toolWindowManager;
  assert.ok(twm, 'tool-window-manager.js must expose window.toolWindowManager');
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {}, invoke });
  return { twm, calls, stripEls };
}

// Strip DOM is section elements (strip-section[--row/--end]) each holding
// button children with dataset.toolWindow set by makeStripBtn(); walk every
// strip's sections looking for the one carrying `windowId`.
function findStripBtn(stripEls, windowId) {
  for (const stripEl of Object.values(stripEls)) {
    for (const section of stripEl.children) {
      for (const child of section.children) {
        if (child && child.dataset && child.dataset.toolWindow === windowId) return child;
      }
    }
  }
  return null;
}

const HOST = {
  title: 'SFTP',
  type: 'builtin',
  defaultZone: 'bottom-left',
  renderFn: () => null,
  companions: [{ id: 'transfer-center', position: 'bottom' }],
};
const COMPANION = { title: 'Transfers', type: 'builtin', defaultZone: 'bottom-right', renderFn: () => null };

// --- 1. pop-out suppresses an ACTIVE companion; open carries companionIds ---
{
  const { twm, calls } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.activate('transfer-center');
  twm.setViewMode('file-explorer', 'window');          // enterWindowMode path
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), true);
  assert.strictEqual(twm.isVisible('transfer-center'), false, 'companion left its zone');
  const open = calls.invokes.find((c) => c.cmd === 'open_panel_host');
  assert.ok(open, 'enterWindowMode must call open_panel_host');
  assert.deepStrictEqual(JSON.parse(JSON.stringify(open.args.companionIds)), ['transfer-center']);
}

// --- 2. dock-back restores the companion, active state preserved -------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.activate('transfer-center');
  twm.setViewMode('file-explorer', 'window');
  twm.setViewMode('file-explorer', 'dock');            // dockFromWindowMode path
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), false);
  assert.strictEqual(twm.isVisible('transfer-center'), true, 'was active -> active again');
}

// --- 3. a companion CLOSED before pop-out stays closed after dock ------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  // registered but never activated (or deactivate it if register auto-activates)
  if (twm.isVisible('transfer-center')) twm.deactivate('transfer-center');
  twm.setViewMode('file-explorer', 'window');
  twm.setViewMode('file-explorer', 'dock');
  assert.strictEqual(twm.isVisible('transfer-center'), false, 'was closed -> stays closed');
}

// --- 4. activate() is inert while suppressed (auto-open guard) ---------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('file-explorer', 'window');
  twm.activate('transfer-center');
  assert.strictEqual(twm.isVisible('transfer-center'), false, 'suppressed id cannot activate');
}

// --- 5. hide does NOT lift; abort does ---------------------------------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('file-explorer', 'window');
  twm.toggle('file-explorer');                          // hides the host window
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), true, 'hide keeps suppression');
  twm.notifyHostAborted('file-explorer');
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), false, 'abort lifts');
}

// --- 6. companion popped out solo first gets absorbed -------------------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('transfer-center', 'window');
  twm.setViewMode('file-explorer', 'window');
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), true);
  assert.strictEqual(twm.getViewMode ? twm.getViewMode('transfer-center') : 'dock', 'dock',
    'solo host docked before suppression');
}

// --- 7. registration-order restore: host registers first, companion later ----
{
  const { twm } = loadManager();
  twm.setPersistedViewModes({ 'file-explorer': 'window' });
  twm.setPersistedZones({ 'file-explorer': 'bottom-left', 'transfer-center': 'bottom-right' });
  twm.setPersistedActiveZoneWindows({ 'bottom-left': 'file-explorer' });
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), true,
    'companion registering after the popped-out host is suppressed on arrival');
}

// --- 8. suppressed companion strip button: away class, click focuses host ----
{
  const { twm, calls, stripEls } = loadManagerWithStrips();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('file-explorer', 'window');
  const btn = findStripBtn(stripEls, 'transfer-center');
  assert.ok(btn, 'suppressed companion still gets a strip button');
  assert.ok(String(btn.className).includes('strip-btn--away'), 'away styling');
  assert.ok(!String(btn.className).includes('active'), 'never lit while away');
  btn.click();
  const focus = calls.invokes.find((c) => c.cmd === 'focus_panel_host');
  assert.ok(focus, 'click issues focus_panel_host');
  assert.strictEqual(focus.args.toolWindowId, 'file-explorer', 'click targets the HOST id, not the companion');
}

// --- 9. context menu: move/view/hide disabled while suppressed ----------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('file-explorer', 'window');
  const items = twm.buildContextMenuItems('transfer-center');
  assert.ok(items, 'buildContextMenuItems returns entries for a suppressed companion');
  const actionable = items.filter((i) => !i.separator);
  assert.ok(actionable.every((i) => i.disabled === true),
    'every actionable entry disabled while suppressed');
}

console.log('test_composite_host: all assertions passed');
