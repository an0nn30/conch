// Run: node scripts/tests/test_bottom_zone_split.mjs
//
// The bottom tool-window zone is being split into a left/right pair
// (bottom-left, bottom-right), mirroring the existing left/right sidebars'
// top/bottom pairs. 'bottom' predates the split and is not going away as a
// name: it lives on in old state.toml zone values, plugin location strings,
// and defaultZone registrations, all of which must keep landing somewhere
// sensible rather than erroring out. normalizeZoneName() is the single place
// that decides where: 'bottom' aliases to 'bottom-left', a known zone name
// passes through unchanged, and anything else is rejected.
//
// This suite (Task 1) covers the zone list and the aliasing surface: the
// setters that ingest persisted state, register()'s defaultZone handling,
// and moveTo(). Task 2 appends pair-layout coverage (divider drag, split
// ratios) to this same file.
//
// No jsdom in this repo (see test_tl_dialog.mjs for the precedent). This
// stubs just enough of document for tool-window-manager.js to load and for
// register() to run — every DOM-touching helper it calls is written
// defensively against missing elements, so the module runs for real against
// a document that returns null.
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
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    dataset: {},
    className: '',
    title: '',
    textContent: '',
    classList: {
      _set: new Set(),
      add(...c) { c.forEach((x) => this._set.add(x)); },
      remove(...c) { c.forEach((x) => this._set.delete(x)); },
      toggle(c, force) { if (force === undefined) { this._set.has(c) ? this._set.delete(c) : this._set.add(c); } else if (force) { this._set.add(c); } else { this._set.delete(c); } },
      contains(c) { return this._set.has(c); },
    },
    appendChild(child) { this.children.push(child); return child; },
    removeChild(child) { const i = this.children.indexOf(child); if (i >= 0) this.children.splice(i, 1); return child; },
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener() {},
    removeEventListener() {},
    setAttribute(n, v) { attrs.set(n, String(v)); },
    getAttribute(n) { return attrs.has(n) ? attrs.get(n) : null; },
  };
  // The bottom-strip renderer clears the strip with `stripEl.innerHTML = ''`
  // before rebuilding it; mirror that against the stubbed children array.
  Object.defineProperty(el, 'innerHTML', {
    set(v) { if (v === '') this.children.length = 0; },
    get() { return ''; },
  });
  return el;
}

// Load the module fresh for each scenario so zone state never leaks between them.
function loadManager() {
  const body = makeElement('body');
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    document: {
      body,
      createElement: (t) => makeElement(t),
      // Every element lookup misses: the manager's update* helpers all bail
      // early on a null element, which is exactly the surface we want here.
      getElementById: () => null,
      querySelector: () => null,
      querySelectorAll: () => [],
      addEventListener() {},
    },
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });
  const twm = sandbox.toolWindowManager;
  assert.ok(twm, 'tool-window-manager.js must expose window.toolWindowManager');
  return twm;
}

// moveTo() has a pre-existing guard — unrelated to this task — that requires
// the target zone's contentEl to be populated (i.e. init() has run against a
// real `[data-zone]` element), or the move silently no-ops. The bare
// loadManager() harness above always returns null from querySelector, so it
// can never satisfy that guard. This loader is loadManager() plus a
// querySelector that hands init() a real (stubbed) element per zone, purely
// so moveTo() has somewhere to land — scenario 5 below is the only user.
function loadManagerWithZoneDom() {
  const body = makeElement('body');
  const zoneEls = new Map();
  for (const z of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right']) {
    const zoneEl = makeElement('div');
    const contentEl = makeElement('div');
    zoneEl.querySelector = (sel) => (sel === '.zone-content' ? contentEl : null);
    // The base makeElement() (copied verbatim per the harness precedent) has
    // no insertBefore — updateZone()'s header-insertion path needs one.
    zoneEl.insertBefore = (child) => zoneEl.appendChild(child);
    zoneEls.set(z, zoneEl);
  }
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    document: {
      body,
      createElement: (t) => makeElement(t),
      getElementById: () => null,
      querySelector: (sel) => {
        const m = /^\[data-zone="([^"]+)"\]$/.exec(sel);
        return m ? (zoneEls.get(m[1]) || null) : null;
      },
      querySelectorAll: () => [],
      addEventListener() {},
    },
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });
  const twm = sandbox.toolWindowManager;
  assert.ok(twm, 'tool-window-manager.js must expose window.toolWindowManager');
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {} });
  return twm;
}

// --- 1. normalizeZoneName aliasing ------------------------------------------
{
  const twm = loadManager();
  assert.strictEqual(twm.normalizeZoneName('bottom'), 'bottom-left', 'legacy alias');
  assert.strictEqual(twm.normalizeZoneName('bottom-left'), 'bottom-left');
  assert.strictEqual(twm.normalizeZoneName('bottom-right'), 'bottom-right');
  assert.strictEqual(twm.normalizeZoneName('left-top'), 'left-top');
  assert.strictEqual(twm.normalizeZoneName('attic'), null, 'junk is rejected');
}

// --- 2. register() with legacy defaultZone lands in bottom-left --------------
{
  const twm = loadManager();
  twm.register('file-explorer', { title: 'SFTP', type: 'builtin', defaultZone: 'bottom', renderFn: () => null });
  assert.strictEqual(twm.getZoneForWindow('file-explorer'), 'bottom-left');
}

// --- 3. persisted legacy zone value and active key both migrate --------------
{
  const twm = loadManager();
  twm.setPersistedZones({ 'file-explorer': 'bottom' });
  twm.setPersistedActiveZoneWindows({ bottom: 'file-explorer' });
  twm.setPersistedPanelVisibility({ left: true, right: true, bottom: true });
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('file-explorer', { title: 'SFTP', type: 'builtin', defaultZone: 'bottom', renderFn: () => null });
  assert.strictEqual(twm.getZoneForWindow('file-explorer'), 'bottom-left');
  assert.strictEqual(twm.isVisible('file-explorer'), true, 'saved-active window restores active');
  const active = JSON.parse(JSON.stringify(twm.getActiveZoneAssignments()));
  assert.strictEqual(active['bottom-left'], 'file-explorer', 'saves emit only new keys');
  assert.ok(!('bottom' in active), 'legacy key never re-emitted');
}

// --- 4. closed-state marker survives migration --------------------------------
{
  const twm = loadManager();
  twm.setPersistedZones({ 'file-explorer': 'bottom' });
  twm.setPersistedActiveZoneWindows({ bottom: '' });
  twm.setPersistedPanelVisibility({ left: true, right: true, bottom: true });
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('file-explorer', { title: 'SFTP', type: 'builtin', defaultZone: 'bottom', renderFn: () => null });
  assert.strictEqual(twm.isVisible('file-explorer'), false, 'closed-while-bottom stays closed');
  assert.strictEqual(
    JSON.parse(JSON.stringify(twm.getActiveZoneAssignments()))['bottom-left'], '',
    'closed marker re-serialises under the new key',
  );
}

// loadManagerWithZoneDom() gives every zone a real element but stubs
// document.getElementById() to return null unconditionally, so
// bottomZoneWrapEl / bottomZoneDividerEl never get wired up and updateBottomZone()
// / initZoneDivider('bottom') bail on their null-element guards. This loader is
// that harness plus real (stubbed) elements for the two bottom-zone ids, so
// pair-layout and horizontal-divider behaviour has real DOM to act on. Built on
// the same per-zone querySelector pattern as loadManagerWithZoneDom() rather
// than duplicated from scratch.
function loadManagerWithBottomDom() {
  const body = makeElement('body');
  const zoneEls = {};
  for (const z of ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right']) {
    const zoneEl = makeElement('div');
    const contentEl = makeElement('div');
    const tabStripEl = makeElement('div');
    zoneEl.querySelector = (sel) => {
      if (sel === '.zone-content') return contentEl;
      if (sel === '.zone-tab-strip') return tabStripEl;
      return null;
    };
    // Same insertBefore shim as loadManagerWithZoneDom() — updateZone()'s
    // header-insertion path needs one and the base makeElement() has none.
    zoneEl.insertBefore = (child) => zoneEl.appendChild(child);
    zoneEls[z] = zoneEl;
  }
  const wrapEl = makeElement('div');
  const dividerEl = makeElement('div');
  const stripEl = makeElement('div');
  const idEls = {
    'bottom-zone-wrap': wrapEl,
    'bottom-zone-divider': dividerEl,
    'bottom-strip': stripEl,
  };
  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    document: {
      body,
      createElement: (t) => makeElement(t),
      getElementById: (id) => idEls[id] || null,
      querySelector: (sel) => {
        const m = /^\[data-zone="([^"]+)"\]$/.exec(sel);
        return m ? (zoneEls[m[1]] || null) : null;
      },
      querySelectorAll: () => [],
      addEventListener() {},
    },
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });
  const twm = sandbox.toolWindowManager;
  assert.ok(twm, 'tool-window-manager.js must expose window.toolWindowManager');
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {} });
  return { twm, zoneEls, dividerEl, wrapEl, stripEl };
}

// --- 5. moveTo accepts both new names -----------------------------------------
{
  const twm = loadManagerWithZoneDom();
  twm.register('a', { title: 'A', type: 'builtin', defaultZone: 'bottom-left', renderFn: () => null });
  twm.register('b', { title: 'B', type: 'builtin', defaultZone: 'left-top', renderFn: () => null });
  twm.moveTo('b', 'bottom-right');
  assert.strictEqual(twm.getZoneForWindow('b'), 'bottom-right');
  twm.moveTo('b', 'bottom');           // legacy alias still routes
  assert.strictEqual(twm.getZoneForWindow('b'), 'bottom-left');
}

console.log('test_bottom_zone_split: zone aliasing assertions passed');

// --- 6. pair layout: lone window takes full width, both restore ratio --------
{
  const { twm, zoneEls, dividerEl, wrapEl } = loadManagerWithBottomDom();
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('a', { title: 'A', type: 'builtin', defaultZone: 'bottom-left', renderFn: () => null });
  assert.strictEqual(zoneEls['bottom-left'].style.flex, '1', 'lone left window gets all space');
  assert.strictEqual(zoneEls['bottom-right'].style.flex, '0');
  assert.ok(dividerEl.classList.contains('hidden'), 'no divider with one section');
  assert.ok(!wrapEl.classList.contains('hidden'), 'bar visible');

  twm.register('b', { title: 'B', type: 'builtin', defaultZone: 'bottom-right', renderFn: () => null });
  twm.activate('b');
  assert.ok(!dividerEl.classList.contains('hidden'), 'divider appears with both sections');
  const lf = parseFloat(zoneEls['bottom-left'].style.flex);
  const rf = parseFloat(zoneEls['bottom-right'].style.flex);
  assert.ok(Math.abs(lf - 0.5) < 0.01 && Math.abs(rf - 0.5) < 0.01, 'both active defaults to 50/50');

  twm.deactivate('b');
  assert.strictEqual(zoneEls['bottom-left'].style.flex, '1', 'closing right gives left all space');
  assert.ok(dividerEl.classList.contains('hidden'));

  twm.deactivate('a');
  assert.ok(wrapEl.classList.contains('hidden'), 'bar hides when both sections empty');
}

// --- 7. split ratio API grows a bottom arm ------------------------------------
{
  const { twm } = loadManagerWithBottomDom();
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('a', { title: 'A', type: 'builtin', defaultZone: 'bottom-left', renderFn: () => null });
  twm.register('b', { title: 'B', type: 'builtin', defaultZone: 'bottom-right', renderFn: () => null });
  twm.activate('b'); twm.activate('a');
  twm.setSplitRatio('bottom', 0.3);
  const ratios = JSON.parse(JSON.stringify(twm.getSplitRatios()));
  assert.ok(Math.abs(ratios.bottom - 0.3) < 0.01, 'bottom ratio round-trips');
}

console.log('test_bottom_zone_split: pair layout assertions passed');

// --- 8. strip splits into left/right ends -------------------------------------
{
  const { twm, stripEl } = loadManagerWithBottomDom(); // extend the loader to expose bottom-strip
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('a', { title: 'A', type: 'builtin', defaultZone: 'bottom-left', renderFn: () => null });
  twm.register('b', { title: 'B', type: 'builtin', defaultZone: 'bottom-right', renderFn: () => null });
  const sections = stripEl.children.filter((c) => String(c.className).includes('strip-section'));
  assert.strictEqual(sections.length, 2, 'two strip sections');
  assert.ok(String(sections[1].className).includes('strip-section--end'), 'second section is the right end');
  assert.strictEqual(sections[0].children.length, 1, 'left end holds the bottom-left window');
  assert.strictEqual(sections[1].children.length, 1, 'right end holds the bottom-right window');
}

console.log('test_bottom_zone_split: strip assertions passed');
