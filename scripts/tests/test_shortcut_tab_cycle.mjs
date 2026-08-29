// Run: node scripts/tests/test_shortcut_tab_cycle.mjs
//
// cmd+shift+{ / cmd+shift+} select the tab to the left / right of the active
// one, wrapping at both ends. The keycap types "{" but the router normalizes
// by KeyboardEvent.code, so the shipped config spells "cmd+shift+[" — these
// tests drive the real shortcut runtime with events shaped like the browser
// delivers them (code "BracketLeft", key "{").
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

function loadRuntime({ keyboard, tabIds, activeTabId }) {
  const sandbox = { console, document: { activeElement: null } };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const file of ['input-runtime.js', 'shortcut-runtime.js']) {
    const p = path.join(APP, file);
    vm.runInContext(fs.readFileSync(p, 'utf8'), sandbox, { filename: p });
  }

  const calls = { activated: [], menuActions: [] };
  const handlers = new Map();
  sandbox.termlabKeyboardRouter = {
    register: (h) => { handlers.set(h.name, h); },
  };
  sandbox.toolWindowManager = { listWindows: () => [], toggle: () => {} };

  const invoke = async (cmd) => {
    if (cmd === 'get_all_settings') return { termlab: { keyboard } };
    if (cmd === 'get_plugin_menu_items') return [];
    return null;
  };

  const runtime = sandbox.termlabShortcutRuntime.create({
    invoke,
    isMacPlatform: true,
    isTextInputTarget: sandbox.termlabInputRuntime.create().isTextInputTarget,
    handleMenuAction: (action) => { calls.menuActions.push(action); },
    shouldDebugKeyEvent: () => false,
    formatKeyEventForDebug: () => '',
    shortcutDebugEnabled: false,
    openCommandPalette: () => {},
    closeCommandPalette: () => {},
    isCommandPaletteOpen: () => false,
    getTabIds: () => tabIds,
    activateTab: (id) => { calls.activated.push(id); },
    getCurrentPane: () => ({ kind: 'terminal' }),
    writeTextToCurrentPane: () => {},
    getActiveTab: () => ({ id: activeTabId, containerEl: {} }),
    getFocusedPaneId: () => 1,
    setFocusedPane: () => {},
    findAdjacentPane: () => null,
  });

  return { runtime, handlers, calls };
}

// A keydown as the browser delivers cmd+shift+[ / cmd+shift+] from a focused
// terminal: the shifted key is "{" / "}", the physical code is Bracket*.
function bracketEvent(side) {
  return {
    metaKey: true, ctrlKey: false, altKey: false, shiftKey: true,
    code: side === 'left' ? 'BracketLeft' : 'BracketRight',
    key: side === 'left' ? '{' : '}',
    target: { tagName: 'TEXTAREA', className: 'xterm-helper-textarea' },
    preventDefault: () => {},
  };
}

const DEFAULTS = {
  select_tab_left: 'cmd+shift+[',
  select_tab_right: 'cmd+shift+]',
};

// Left and right select the adjacent tab, and the keystroke is consumed.
{
  const { runtime, handlers, calls } = loadRuntime({
    keyboard: DEFAULTS, tabIds: ['a', 'b', 'c'], activeTabId: 'b',
  });
  await runtime.init();
  const fallbacks = handlers.get('shortcut-fallbacks');

  assert.strictEqual(fallbacks.onKeyDown(bracketEvent('left')), true, 'consumed');
  assert.strictEqual(fallbacks.onKeyDown(bracketEvent('right')), true, 'consumed');
  assert.deepStrictEqual(calls.activated, ['a', 'c']);
}

// Both directions wrap at the ends.
{
  const first = loadRuntime({ keyboard: DEFAULTS, tabIds: ['a', 'b', 'c'], activeTabId: 'a' });
  await first.runtime.init();
  first.handlers.get('shortcut-fallbacks').onKeyDown(bracketEvent('left'));
  assert.deepStrictEqual(first.calls.activated, ['c'], 'left from the first tab wraps to the last');

  const last = loadRuntime({ keyboard: DEFAULTS, tabIds: ['a', 'b', 'c'], activeTabId: 'c' });
  await last.runtime.init();
  last.handlers.get('shortcut-fallbacks').onKeyDown(bracketEvent('right'));
  assert.deepStrictEqual(last.calls.activated, ['a'], 'right from the last tab wraps to the first');
}

// A single tab has nowhere to cycle; nothing activates but the shortcut is
// still consumed rather than leaking "{" into the terminal.
{
  const { runtime, handlers, calls } = loadRuntime({
    keyboard: DEFAULTS, tabIds: ['only'], activeTabId: 'only',
  });
  await runtime.init();
  assert.strictEqual(handlers.get('shortcut-fallbacks').onKeyDown(bracketEvent('left')), true);
  assert.deepStrictEqual(calls.activated, []);
}

// The bindings are configuration, not hardcoded keys: a rebound combo works
// and the old default no longer fires.
{
  const { runtime, handlers, calls } = loadRuntime({
    keyboard: { select_tab_left: 'ctrl+shift+h', select_tab_right: 'ctrl+shift+l' },
    tabIds: ['a', 'b'], activeTabId: 'a',
  });
  await runtime.init();
  const fallbacks = handlers.get('shortcut-fallbacks');
  assert.strictEqual(fallbacks.onKeyDown(bracketEvent('right')), false, 'unbound default falls through');
  const custom = {
    metaKey: false, ctrlKey: true, altKey: false, shiftKey: true,
    code: 'KeyL', key: 'L',
    target: { tagName: 'TEXTAREA', className: 'xterm-helper-textarea' },
    preventDefault: () => {},
  };
  assert.strictEqual(fallbacks.onKeyDown(custom), true);
  assert.deepStrictEqual(calls.activated, ['b']);
}

console.log('shortcut tab cycle: all assertions passed');
