// Run: node scripts/tests/test_tool_window_closed_state.mjs
//
// Regression test: a tool window the user CLOSED used to reopen itself on the
// next launch.
//
// The Hide button on a zone header calls deactivate(), which sets
// zones[zone].activeId = null but deliberately leaves panel visibility alone
// (the panel is still "open", it just has nothing active in it). The layout is
// then serialised by getActiveZoneAssignments(), which only emitted zones whose
// activeId was non-null — so "the user closed the only window in this zone"
// serialised to exactly the same bytes as "this layout has never heard of this
// zone": an absent key. On the next boot register() read that absent key as
// "unconfigured", auto-activated the zone's default window, and reopened what
// the user had closed. Reported against SFTP in the bottom zone, but the same
// hole existed for every zone.
//
// The fix makes the closed state representable: a zone that has windows but no
// active one serialises as an empty-string value, which register() reads as
// "the user configured this zone closed — do not auto-activate". An absent key
// still means unconfigured, so existing saved layouts keep their current
// first-boot behaviour.
//
// No jsdom in this repo (see test_tl_dialog.mjs for the precedent). This stubs
// just enough of document for tool-window-manager.js to load and for register()
// to run — every DOM-touching helper it calls (updateZone/updateSidebar/
// updateBottomZone/updateStrips) is written defensively against missing
// elements, so the module runs for real against a document that returns null.
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

const BOTTOM_WINDOW = {
  title: 'SFTP',
  type: 'built-in',
  defaultZone: 'bottom',
  renderFn: () => {},
};

// --- 1. Closing the only window in a zone must be representable on save -----
{
  const twm = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {} });
  // Boot applies panel visibility before any window registers; the bottom
  // panel's in-memory default is hidden, so skipping this would test a state
  // the app never actually reaches.
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('file-explorer', BOTTOM_WINDOW);

  const openMap = twm.getActiveZoneAssignments();
  assert.strictEqual(
    openMap['bottom-left'],
    'file-explorer',
    'a freshly registered window should serialise as the bottom zone\'s active window',
  );

  // What the Hide button on the zone header does.
  twm.deactivate('file-explorer');

  const closedMap = twm.getActiveZoneAssignments();
  assert.ok(
    Object.prototype.hasOwnProperty.call(closedMap, 'bottom-left'),
    'after closing the only window in a zone, the saved layout must still record '
    + 'the zone — otherwise "closed" is indistinguishable from "never configured"',
  );
  assert.strictEqual(
    closedMap['bottom-left'],
    '',
    'a zone with windows but none active must serialise as the empty string',
  );
}

// --- 2. A layout saved as closed must stay closed on the next boot ----------
{
  const twm = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {} });

  // Boot restores exactly what scenario 1 saved: the panel itself is still
  // "visible" (Hide never touched panel visibility) but the zone is closed.
  twm.setPersistedZones({ 'file-explorer': 'bottom' });
  twm.setPersistedActiveZoneWindows({ bottom: '' });
  twm.setPersistedPanelVisibility({ left: true, right: true, bottom: true });
  twm.setPanelVisibility('bottom', true, { save: false });

  twm.register('file-explorer', BOTTOM_WINDOW);

  assert.strictEqual(
    twm.getActiveZoneAssignments()['bottom-left'],
    '',
    'registering a window into a zone the user closed must NOT auto-activate it',
  );
}

// --- 3. An unconfigured zone still auto-activates (no migration needed) -----
{
  const twm = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {} });

  // An existing saved layout from before this fix: no bottom key at all.
  twm.setPersistedZones({ 'file-explorer': 'bottom' });
  twm.setPersistedActiveZoneWindows({});
  twm.setPersistedPanelVisibility({ left: true, right: true, bottom: true });
  twm.setPanelVisibility('bottom', true, { save: false });

  twm.register('file-explorer', BOTTOM_WINDOW);

  assert.strictEqual(
    twm.getActiveZoneAssignments()['bottom-left'],
    'file-explorer',
    'an absent key still means "unconfigured", so the default window opens as before',
  );
}

// --- 4. The same hole existed on the sides, not just the bottom ------------
{
  const twm = loadManager();
  twm.init({ fitActiveTab: () => {}, saveLayout: () => {} });
  twm.register('ssh-sessions', { title: 'Hosts', type: 'built-in', defaultZone: 'right-top', renderFn: () => {} });
  twm.deactivate('ssh-sessions');

  assert.strictEqual(
    twm.getActiveZoneAssignments()['right-top'],
    '',
    'closing a side-zone window must be recorded the same way as a bottom-zone one',
  );
}

console.log('tool-window closed-state persistence: all assertions passed');
