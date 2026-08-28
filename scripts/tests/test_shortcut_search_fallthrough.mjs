// Run: node scripts/tests/test_shortcut_search_fallthrough.mjs
//
// cmd+shift+f (search-in-project) outside a project window. Same shape as
// test_shortcut_save_fallthrough.mjs's cmd+s case, because the hazard is the
// same: the Search tool window is only ever registered when
// termlabProjectMode.isActive() (tool-window-runtime.js), so claiming the
// combo in every window would make it a dead keystroke — or worse, silently
// swallow a combo the user (or a plugin) bound to something real — in every
// plain terminal window.
//
//   1. The core `search-in-project` action must NOT run outside a project
//      window.
//   2. A tool-window or plugin action bound to the same combo must still run.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

function loadRuntime({ hasProject, toolWindowCombo, pluginCombo }) {
  const sandbox = { console, document: { activeElement: null } };
  sandbox.window = sandbox;
  sandbox.termlabProjectMode = { isActive: () => !!hasProject };
  vm.createContext(sandbox);
  for (const file of ['input-runtime.js', 'shortcut-runtime.js']) {
    const p = path.join(APP, file);
    vm.runInContext(fs.readFileSync(p, 'utf8'), sandbox, { filename: p });
  }

  const calls = { menuActions: [], toolWindowToggles: [], pluginActions: [] };

  const handlers = new Map();
  sandbox.termlabKeyboardRouter = {
    register: (h) => { handlers.set(h.name, h); },
  };
  sandbox.toolWindowManager = {
    listWindows: () => (toolWindowCombo ? [{ id: 'project-search' }] : []),
    toggle: (id) => { calls.toolWindowToggles.push(id); },
  };

  const settings = {
    termlab: {
      keyboard: {
        search_in_project: 'cmd+shift+f',
        tool_window_shortcuts: toolWindowCombo ? { 'project-search': toolWindowCombo } : {},
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
    getCurrentPane: () => null,
    writeTextToCurrentPane: () => {},
    getActiveTab: () => null,
    getFocusedPaneId: () => null,
    setFocusedPane: () => {},
    findAdjacentPane: () => null,
  });

  return { runtime, handlers, calls };
}

// A keydown as it arrives from a focused terminal: the target is xterm's
// helper textarea, which input-runtime deliberately does not count as a text
// input.
const cmdShiftF = () => ({
  metaKey: true, ctrlKey: false, altKey: false, shiftKey: true,
  code: 'KeyF', key: 'f',
  target: { tagName: 'TEXTAREA', className: 'xterm-helper-textarea' },
});

// --- 1. A tool window bound to cmd+shift+f still opens outside a project. ---
{
  const { runtime, handlers, calls } = loadRuntime({ hasProject: false, toolWindowCombo: 'cmd+shift+f' });
  await runtime.init();
  const consumed = handlers.get('shortcut-fallbacks').onKeyDown(cmdShiftF());

  assert.deepStrictEqual(
    calls.toolWindowToggles, ['project-search'],
    'a tool window bound to cmd+shift+f must still toggle outside a project window',
  );
  assert.strictEqual(consumed, true, 'and the keystroke is consumed by that tool window');
  assert.deepStrictEqual(
    calls.menuActions, [],
    'while the core search-in-project action must not run without a project',
  );
}

// --- 2. Same for a plugin action. -------------------------------------------
{
  const { runtime, handlers, calls } = loadRuntime({ hasProject: false, pluginCombo: 'cmd+shift+f' });
  await runtime.init();
  handlers.get('shortcut-fallbacks').onKeyDown(cmdShiftF());

  assert.deepStrictEqual(
    calls.pluginActions.map((a) => `${a.pluginName}:${a.action}`), ['demo:do-thing'],
    'a plugin action bound to cmd+shift+f must still fire outside a project window',
  );
  assert.deepStrictEqual(calls.menuActions, [], 'and still no search-in-project');
}

// --- 3. With nothing else bound, cmd+shift+f passes through untouched. -----
{
  const { runtime, handlers, calls } = loadRuntime({ hasProject: false });
  await runtime.init();
  const consumed = handlers.get('shortcut-fallbacks').onKeyDown(cmdShiftF());

  assert.strictEqual(consumed, false, 'cmd+shift+f must pass through outside a project window');
  assert.deepStrictEqual(calls.menuActions, []);
  assert.deepStrictEqual(calls.toolWindowToggles, []);
}

// --- 4. Inside a project window, cmd+shift+f runs search-in-project and ----
//        wins over a tool window bound to the same combo.
{
  const { runtime, handlers, calls } = loadRuntime({ hasProject: true, toolWindowCombo: 'cmd+shift+f' });
  await runtime.init();
  const consumed = handlers.get('shortcut-fallbacks').onKeyDown(cmdShiftF());

  assert.deepStrictEqual(calls.menuActions, ['search-in-project'], 'a project window runs the action');
  assert.strictEqual(consumed, true);
  assert.deepStrictEqual(
    calls.toolWindowToggles, [],
    'and search-in-project wins over a tool window bound to the same combo',
  );
}

// --- 5. No termlabProjectMode at all is treated as "no project". -----------
{
  const sandbox = { console, document: { activeElement: null } };
  sandbox.window = sandbox;
  // Deliberately no sandbox.termlabProjectMode at all.
  vm.createContext(sandbox);
  for (const file of ['input-runtime.js', 'shortcut-runtime.js']) {
    const p = path.join(APP, file);
    vm.runInContext(fs.readFileSync(p, 'utf8'), sandbox, { filename: p });
  }
  const handlers = new Map();
  sandbox.termlabKeyboardRouter = { register: (h) => { handlers.set(h.name, h); } };
  sandbox.toolWindowManager = { listWindows: () => [], toggle: () => {} };
  const calls = { menuActions: [] };
  const settings = { termlab: { keyboard: { search_in_project: 'cmd+shift+f' } } };
  const runtime = sandbox.termlabShortcutRuntime.create({
    invoke: async (cmd) => (cmd === 'get_all_settings' ? settings : (cmd === 'get_plugin_menu_items' ? [] : null)),
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
    getCurrentPane: () => null,
    writeTextToCurrentPane: () => {},
    getActiveTab: () => null,
    getFocusedPaneId: () => null,
    setFocusedPane: () => {},
    findAdjacentPane: () => null,
  });
  await runtime.init();
  handlers.get('shortcut-fallbacks').onKeyDown(cmdShiftF());
  assert.deepStrictEqual(calls.menuActions, [], 'a window with no project-mode global at all is not a project window');
}

console.log('shortcut search fallthrough: all assertions passed');
