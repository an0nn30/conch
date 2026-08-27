// Run: node scripts/tests/test_tab_switcher_runtime.mjs
//
// The ctrl+tab MRU switcher's state machine: open on ctrl+tab (selection
// starts on the previous tab), cycle with repeated tab / shift+tab, commit on
// releasing ctrl, cancel on Escape, digits 1-5 jump-commit. The overlay
// itself is a seam (`view`) so this suite drives the machine with a recorder.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

function load({ tabs, activeId }) {
  const sandbox = { console };
  sandbox.window = sandbox;
  const windowListeners = new Map();
  sandbox.addEventListener = (name, fn) => { windowListeners.set(name, fn); };
  sandbox.CustomEvent = class { constructor(name, opts) { this.type = name; this.detail = opts && opts.detail; } };
  vm.createContext(sandbox);
  for (const file of ['features/tab-switcher/mru.js', 'features/tab-switcher/runtime.js']) {
    const p = path.join(APP, file);
    vm.runInContext(fs.readFileSync(p, 'utf8'), sandbox, { filename: p });
  }

  const handlers = new Map();
  sandbox.termlabKeyboardRouter = {
    register: (h) => { handlers.set(h.name, h); },
  };

  const calls = { activated: [], viewOpens: [], viewUpdates: [], viewCloses: 0 };
  const view = {
    open: (items, selectedIndex) => { calls.viewOpens.push({ items: JSON.parse(JSON.stringify(items)), selectedIndex }); },
    update: (items, selectedIndex) => { calls.viewUpdates.push({ selectedIndex }); },
    close: () => { calls.viewCloses += 1; },
  };

  const state = { tabs: [...tabs], activeId };
  const runtime = sandbox.termlabTabSwitcherRuntime.create({
    getTabItems: () => state.tabs.map((t) => ({ ...t })),
    activateTab: (id) => { calls.activated.push(id); state.activeId = id; },
    view,
  });
  runtime.init();

  // Feed activation history the way the app does: via the window event the
  // tab manager already dispatches.
  const fireActivated = (tabId) => {
    const fn = windowListeners.get('termlab-active-tab-changed');
    if (fn) fn({ detail: { tabId } });
  };

  calls.runtime = runtime;
  return { handlers, calls, state, fireActivated };
}

const key = (props) => ({
  metaKey: false, ctrlKey: false, altKey: false, shiftKey: false,
  key: '', preventDefault: () => {}, ...props,
});
const ctrlTab = (shift = false) => key({ ctrlKey: true, shiftKey: shift, key: 'Tab' });
const TABS = [
  { id: 1, label: 'zsh', kind: 'terminal' },
  { id: 2, label: 'vim notes.md', kind: 'editor' },
  { id: 3, label: 'dustin@web1', kind: 'ssh' },
];

// --- open on ctrl+tab: MRU order, selection starts on the previous tab ------
{
  const { handlers, calls, fireActivated } = load({ tabs: TABS, activeId: 3 });
  [1, 2, 3].forEach(fireActivated); // activation history: 3 most recent
  const down = handlers.get('tab-switcher').onKeyDown;

  assert.strictEqual(down(ctrlTab()), true, 'ctrl+tab is consumed');
  assert.strictEqual(calls.viewOpens.length, 1, 'overlay opened');
  assert.deepStrictEqual(
    calls.viewOpens[0].items.map((i) => i.id),
    [3, 2, 1],
    'rows in MRU order',
  );
  assert.strictEqual(calls.viewOpens[0].selectedIndex, 1, 'previous tab pre-selected');
}

// --- cycle forward, wrap, and reverse ---------------------------------------
{
  const { handlers, calls, fireActivated } = load({ tabs: TABS, activeId: 3 });
  [1, 2, 3].forEach(fireActivated);
  const down = handlers.get('tab-switcher').onKeyDown;
  down(ctrlTab());            // open, index 1
  down(ctrlTab());            // index 2
  assert.deepStrictEqual(calls.viewUpdates.map((u) => u.selectedIndex), [2]);
  down(ctrlTab());            // wraps to 0
  down(ctrlTab(true));        // shift steps back to 2
  assert.deepStrictEqual(calls.viewUpdates.map((u) => u.selectedIndex), [2, 0, 2]);
}

// --- releasing ctrl commits the selection ------------------------------------
{
  const { handlers, calls, fireActivated } = load({ tabs: TABS, activeId: 3 });
  [1, 2, 3].forEach(fireActivated);
  const h = handlers.get('tab-switcher');
  h.onKeyDown(ctrlTab());
  assert.strictEqual(h.onKeyUp(key({ key: 'Control' })), true, 'ctrl release consumed while open');
  assert.deepStrictEqual(calls.activated, [2], 'previous tab activated');
  assert.strictEqual(calls.viewCloses, 1, 'overlay closed');
}

// --- ctrl release when the switcher is closed is not consumed ----------------
{
  const { handlers, calls } = load({ tabs: TABS, activeId: 3 });
  const h = handlers.get('tab-switcher');
  assert.strictEqual(h.onKeyUp(key({ key: 'Control' })), false);
  assert.deepStrictEqual(calls.activated, []);
}

// --- Escape cancels without switching ----------------------------------------
{
  const { handlers, calls, fireActivated } = load({ tabs: TABS, activeId: 3 });
  [1, 2, 3].forEach(fireActivated);
  const h = handlers.get('tab-switcher');
  h.onKeyDown(ctrlTab());
  assert.strictEqual(h.onKeyDown(key({ key: 'Escape' })), true);
  assert.deepStrictEqual(calls.activated, [], 'no switch on cancel');
  assert.strictEqual(calls.viewCloses, 1);
  // ctrl release after a cancel is a no-op
  assert.strictEqual(h.onKeyUp(key({ key: 'Control' })), false);
}

// --- digits jump-commit -------------------------------------------------------
{
  const { handlers, calls, fireActivated } = load({ tabs: TABS, activeId: 3 });
  [1, 2, 3].forEach(fireActivated);
  const h = handlers.get('tab-switcher');
  h.onKeyDown(ctrlTab());
  assert.strictEqual(h.onKeyDown(key({ ctrlKey: true, key: '3' })), true);
  assert.deepStrictEqual(calls.activated, [1], 'third MRU row is tab 1');
  assert.strictEqual(calls.viewCloses, 1);
}

// --- ctrl+shift+tab from closed opens cycling backward ------------------------
{
  const { handlers, calls, fireActivated } = load({ tabs: TABS, activeId: 3 });
  [1, 2, 3].forEach(fireActivated);
  handlers.get('tab-switcher').onKeyDown(ctrlTab(true));
  assert.strictEqual(calls.viewOpens[0].selectedIndex, 2, 'backward entry selects the last row');
}

// --- fewer than two tabs: never opens, key not consumed -----------------------
{
  const { handlers, calls } = load({ tabs: [TABS[0]], activeId: 1 });
  assert.strictEqual(handlers.get('tab-switcher').onKeyDown(ctrlTab()), false);
  assert.strictEqual(calls.viewOpens.length, 0);
}

// --- other keys while open are swallowed, cmd+tab is left to the OS -----------
{
  const { handlers, calls, fireActivated } = load({ tabs: TABS, activeId: 3 });
  [1, 2, 3].forEach(fireActivated);
  const h = handlers.get('tab-switcher');
  assert.strictEqual(h.onKeyDown(key({ metaKey: true, key: 'Tab' })), false, 'cmd+tab ignored');
  h.onKeyDown(ctrlTab());
  assert.strictEqual(h.onKeyDown(key({ ctrlKey: true, key: 'a' })), true, 'stray keys swallowed while open');
  assert.deepStrictEqual(calls.activated, [], 'stray key does not commit');
}

console.log('test_tab_switcher_runtime: all assertions passed');

// --- clicking a row commits it (view calls commitIndex) -----------------------
{
  const { handlers, calls, fireActivated } = load({ tabs: TABS, activeId: 3 });
  [1, 2, 3].forEach(fireActivated);
  const h = handlers.get('tab-switcher');
  h.onKeyDown(ctrlTab());
  calls.runtime.commitIndex(2);
  assert.deepStrictEqual(calls.activated, [1], 'clicked row activates its tab');
  assert.strictEqual(calls.viewCloses, 1);
  assert.strictEqual(calls.runtime.commitIndex(0), undefined, 'commit when closed is a no-op');
  assert.deepStrictEqual(calls.activated, [1]);
}

console.log('test_tab_switcher_runtime: commitIndex assertions passed');
