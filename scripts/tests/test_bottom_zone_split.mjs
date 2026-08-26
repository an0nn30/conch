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
