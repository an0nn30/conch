// Run: node scripts/tests/test_shortcut_save_fallthrough.mjs
//
// cmd+s outside an editor pane. Two properties have to hold at once and they
// pull against each other:
//
//   1. The core `save-file` action must NOT run — in a terminal the keystroke
//      belongs to the shell.
//   2. A tool-window or plugin action the user bound to the same combo MUST
//      still run — suppressing the save must not suppress everything else.
//
// The runtime keeps both fallback tables keyed by combo and pushes every core
// binding into the function-key table as well, so a suppression that only
// clears the core table moves the save one table down instead of removing it.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

function loadRuntime({ pane, toolWindowCombo, pluginCombo }) {
  const sandbox = { console, document: { activeElement: null } };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const file of ['input-runtime.js', 'shortcut-runtime.js']) {
    const p = path.join(APP, file);
    vm.runInContext(fs.readFileSync(p, 'utf8'), sandbox, { filename: p });
  }

  const calls = { menuActions: [], toolWindowToggles: [], pluginActions: [] };

  // A stand-in keyboard router that just keeps the registered handlers so the
  // test can invoke them the way the real router does.
  const handlers = new Map();
  sandbox.termlabKeyboardRouter = {
    register: (h) => { handlers.set(h.name, h); },
  };
  sandbox.toolWindowManager = {
    listWindows: () => (toolWindowCombo ? [{ id: 'terminal-output' }] : []),
    toggle: (id) => { calls.toolWindowToggles.push(id); },
  };

  const settings = {
    termlab: {
      keyboard: {
        save_file: 'cmd+s',
        tool_window_shortcuts: toolWindowCombo ? { 'terminal-output': toolWindowCombo } : {},
        plugin_shortcuts: pluginCombo ? { 'demo:do-thing': pluginCombo } : {},
      },
    },
  };

  const invoke = async (cmd, args) => {
    if (cmd === 'get_all_settings') return settings;
    if (cmd === 'get_plugin_menu_items') {
      return pluginCombo ? [{ plugin: 'demo', action: 'do-thing', keybind: pluginCombo }] : [];
    }
    if (cmd === 'trigger_plugin_menu_action') { calls.pluginActions.push(args); return null; }
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
    getTabIds: () => [],
    activateTab: () => {},
    getCurrentPane: () => pane,
    writeTextToCurrentPane: () => {},
    getActiveTab: () => null,
    getFocusedPaneId: () => null,
    setFocusedPane: () => {},
    findAdjacentPane: () => null,
  });

  return { runtime, handlers, calls };
}

// A keydown as it arrives from a focused terminal: the target is xterm's helper
// textarea, which input-runtime deliberately does not count as a text input.
const cmdS = () => ({
  metaKey: true, ctrlKey: false, altKey: false, shiftKey: false,
  code: 'KeyS', key: 's',
  target: { tagName: 'TEXTAREA', className: 'xterm-helper-textarea' },
});

const terminalPane = { kind: 'terminal' };
const editorPane = { kind: 'editor' };

// --- 1. A tool window bound to cmd+s still opens from a terminal pane. -------
{
  const { runtime, handlers, calls } = loadRuntime({
    pane: terminalPane, toolWindowCombo: 'cmd+s',
  });
  await runtime.init();
  const consumed = handlers.get('shortcut-fallbacks').onKeyDown(cmdS());

  assert.deepStrictEqual(
    calls.toolWindowToggles, ['terminal-output'],
    'a tool window bound to cmd+s must still toggle while a terminal is focused',
  );
  assert.strictEqual(consumed, true, 'and the keystroke is consumed by that tool window');
  assert.deepStrictEqual(
    calls.menuActions, [],
    'while the core save-file action must not run outside an editor pane',
  );
}

// --- 2. Same for a plugin action. -------------------------------------------
{
  const { runtime, handlers, calls } = loadRuntime({
    pane: terminalPane, pluginCombo: 'cmd+s',
  });
  await runtime.init();
  handlers.get('shortcut-fallbacks').onKeyDown(cmdS());

  assert.deepStrictEqual(
    calls.pluginActions.map((a) => `${a.pluginName}:${a.action}`), ['demo:do-thing'],
    'a plugin action bound to cmd+s must still fire while a terminal is focused',
  );
  assert.deepStrictEqual(calls.menuActions, [], 'and still no save');
}

// --- 3. With nothing else bound, cmd+s reaches the shell untouched. ----------
{
  const { runtime, handlers, calls } = loadRuntime({ pane: terminalPane });
  await runtime.init();
  const consumed = handlers.get('shortcut-fallbacks').onKeyDown(cmdS());

  assert.strictEqual(consumed, false, 'cmd+s must pass through to the terminal');
  assert.deepStrictEqual(calls.menuActions, [], 'and must not save');
  assert.deepStrictEqual(calls.toolWindowToggles, []);
}

// --- 4. Inside an editor pane, cmd+s is the save and consumes the key. -------
{
  const { runtime, handlers, calls } = loadRuntime({
    pane: editorPane, toolWindowCombo: 'cmd+s',
  });
  await runtime.init();
  const consumed = handlers.get('shortcut-fallbacks').onKeyDown(cmdS());

  assert.deepStrictEqual(calls.menuActions, ['save-file'], 'an editor pane saves');
  assert.strictEqual(consumed, true);
  assert.deepStrictEqual(
    calls.toolWindowToggles, [],
    'and the save wins over a tool window bound to the same combo',
  );
}

// --- 5. No pane at all is treated as "not an editor". ------------------------
{
  const { runtime, handlers, calls } = loadRuntime({ pane: null });
  await runtime.init();
  handlers.get('shortcut-fallbacks').onKeyDown(cmdS());
  assert.deepStrictEqual(calls.menuActions, []);
}

console.log('shortcut save fallthrough: all assertions passed');
