// Run: node scripts/tests/test_shortcut_pane_navigation.mjs
//
// The default pane-navigation bindings ship from termlab_core as
// "cmd+alt+left" / "right" / "up" / "down", but a real arrow keydown carries
// key "ArrowLeft" — so the event-side combo is "cmd+alt+arrowleft" and the
// config-side combo never matched: the default shortcuts were dead. Both
// normalizers must agree on one canonical arrow spelling, and configs written
// by the settings recorder (which stores the "arrowleft" spelling) must keep
// working too.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

function loadRuntime({ keyboard }) {
  const sandbox = { console, document: { activeElement: null } };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const file of ['input-runtime.js', 'shortcut-runtime.js']) {
    const p = path.join(APP, file);
    vm.runInContext(fs.readFileSync(p, 'utf8'), sandbox, { filename: p });
  }

  const calls = { focusedPanes: [], adjacentAsks: [], menuActions: [] };

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

  const tab = { containerEl: { name: 'container' } };
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
    getTabIds: () => [],
    activateTab: () => {},
    getCurrentPane: () => ({ kind: 'terminal' }),
    writeTextToCurrentPane: () => {},
    getActiveTab: () => tab,
    getFocusedPaneId: () => 1,
    setFocusedPane: (id) => { calls.focusedPanes.push(id); },
    findAdjacentPane: (paneId, dir, containerEl) => {
      calls.adjacentAsks.push({ paneId, dir, containerEl });
      return 2;
    },
  });

  return { runtime, handlers, calls };
}

// An arrow keydown as it arrives from a focused terminal (xterm's helper
// textarea target, which input-runtime does not count as a text input).
function arrowEvent(name, mods = { metaKey: true, altKey: true }) {
  return {
    metaKey: !!mods.metaKey, ctrlKey: !!mods.ctrlKey,
    altKey: !!mods.altKey, shiftKey: !!mods.shiftKey,
    code: name, key: name,
    target: { tagName: 'TEXTAREA', className: 'xterm-helper-textarea' },
    preventDefault: () => {},
  };
}

// --- the shipped defaults: "cmd+alt+left" etc. must move focus -------------
{
  const defaults = {
    navigate_pane_up: 'cmd+alt+up',
    navigate_pane_down: 'cmd+alt+down',
    navigate_pane_left: 'cmd+alt+left',
    navigate_pane_right: 'cmd+alt+right',
  };
  const { runtime, handlers, calls } = loadRuntime({ keyboard: defaults });
  await runtime.init();
  const fallbacks = handlers.get('shortcut-fallbacks');
  assert.ok(fallbacks, 'shortcut-fallbacks handler registered');

  const cases = [
    ['ArrowUp', 'up'], ['ArrowDown', 'down'], ['ArrowLeft', 'left'], ['ArrowRight', 'right'],
  ];
  for (const [code, dir] of cases) {
    const consumed = fallbacks.onKeyDown(arrowEvent(code));
    assert.strictEqual(consumed, true, `cmd+alt+${code} is consumed with default config`);
    assert.strictEqual(
      calls.adjacentAsks[calls.adjacentAsks.length - 1].dir, dir,
      `cmd+alt+${code} asks for the ${dir} neighbour`,
    );
  }
  assert.deepStrictEqual(calls.focusedPanes, [2, 2, 2, 2], 'each hit moves focus to the neighbour');
}

// --- recorder spelling: "cmd+alt+arrowleft" in config keeps working --------
{
  const recorded = { navigate_pane_left: 'cmd+alt+arrowleft' };
  const { runtime, handlers, calls } = loadRuntime({ keyboard: recorded });
  await runtime.init();
  const consumed = handlers.get('shortcut-fallbacks').onKeyDown(arrowEvent('ArrowLeft'));
  assert.strictEqual(consumed, true, 'recorder-spelled binding still matches');
  assert.deepStrictEqual(calls.focusedPanes, [2]);
}

// --- a plain cmd+arrow (no alt) must NOT navigate ---------------------------
{
  const defaults = { navigate_pane_left: 'cmd+alt+left' };
  const { runtime, handlers, calls } = loadRuntime({ keyboard: defaults });
  await runtime.init();
  const consumed = handlers.get('shortcut-fallbacks')
    .onKeyDown(arrowEvent('ArrowLeft', { metaKey: true }));
  assert.strictEqual(consumed, false, 'cmd+left alone does not hit the binding');
  assert.deepStrictEqual(calls.focusedPanes, [], 'focus untouched');
}

console.log('test_shortcut_pane_navigation: all assertions passed');
