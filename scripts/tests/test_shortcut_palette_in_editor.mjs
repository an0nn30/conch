// Run: node scripts/tests/test_shortcut_palette_in_editor.mjs
//
// cmd+shift+p (ctrl+shift+p off macOS) must reach the command palette from
// EVERY surface in the app, including a focused editor pane.
//
// The palette toggle used to share `isTextInputTarget(event.target)` with the
// plugin/tool-window fallback tables, and that guard is wrong for this one
// binding. CodeMirror's content element is `contenteditable`, so with an
// editor pane focused the target IS a text input and the chord was dropped —
// the palette simply did not open while you were typing in a file. The same
// guard also killed the toggle-closed half: the palette focuses its own
// <input> on open, so pressing the chord again hit an INPUT target and never
// reached `closeCommandPalette()`.
//
// A chord this heavily modified cannot collide with typed text, which is what
// the guard exists to protect. The fallback tables below it still keep theirs
// (see test_shortcut_save_fallthrough.mjs) — bare and lightly-modified plugin
// combos genuinely do need it.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

function loadRuntime({ isMacPlatform = true, paletteOpen = false } = {}) {
  const sandbox = { console, document: { activeElement: null } };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const file of ['input-runtime.js', 'shortcut-runtime.js']) {
    const p = path.join(APP, file);
    vm.runInContext(fs.readFileSync(p, 'utf8'), sandbox, { filename: p });
  }

  const handlers = new Map();
  sandbox.termlabKeyboardRouter = {
    register: (h) => { handlers.set(h.name, h); },
  };

  const calls = { opened: 0, closed: 0 };

  const runtime = sandbox.termlabShortcutRuntime.create({
    invoke: async (cmd) => (cmd === 'get_all_settings' ? { termlab: { keyboard: {} } } : []),
    isMacPlatform,
    isTextInputTarget: sandbox.termlabInputRuntime.create().isTextInputTarget,
    handleMenuAction: () => {},
    shouldDebugKeyEvent: () => false,
    formatKeyEventForDebug: () => '',
    shortcutDebugEnabled: false,
    openCommandPalette: () => { calls.opened += 1; },
    closeCommandPalette: () => { calls.closed += 1; },
    isCommandPaletteOpen: () => paletteOpen,
    getTabIds: () => [],
    activateTab: () => {},
    getCurrentPane: () => ({ kind: 'editor' }),
    writeTextToCurrentPane: () => {},
    getActiveTab: () => null,
    getFocusedPaneId: () => null,
    setFocusedPane: () => {},
    findAdjacentPane: () => null,
  });

  return { runtime, handlers, calls };
}

// CodeMirror's editable surface: a contenteditable div, which is exactly what
// `isTextInputTarget` reports true for.
const cmContent = { tagName: 'DIV', className: 'cm-content', isContentEditable: true };
// xterm's helper textarea — input-runtime deliberately exempts it already.
const xtermTextarea = { tagName: 'TEXTAREA', className: 'xterm-helper-textarea' };
// The palette's own search field, focused for as long as the palette is up.
const paletteInput = { tagName: 'INPUT', type: 'text' };

const chord = (target, over = {}) => ({
  metaKey: true, ctrlKey: false, altKey: false, shiftKey: true,
  code: 'KeyP', key: 'P', target, ...over,
});

// --- 1. A focused editor pane must not swallow the chord. --------------------
{
  const { runtime, handlers, calls } = loadRuntime();
  await runtime.init();
  const consumed = handlers.get('shortcut-palette-toggle').onKeyDown(chord(cmContent));

  assert.strictEqual(
    calls.opened, 1,
    'cmd+shift+p must open the palette while a CodeMirror editor pane is focused',
  );
  assert.strictEqual(consumed, true, 'and the keystroke is consumed rather than falling through');
}

// --- 2. The toggle-closed half works from the palette's own input. -----------
{
  const { runtime, handlers, calls } = loadRuntime({ paletteOpen: true });
  await runtime.init();
  const consumed = handlers.get('shortcut-palette-toggle').onKeyDown(chord(paletteInput));

  assert.strictEqual(
    calls.closed, 1,
    'pressing the chord again with the palette input focused must close the palette',
  );
  assert.strictEqual(calls.opened, 0, 'and must not re-open it');
  assert.strictEqual(consumed, true, 'the keystroke is consumed');
}

// --- 3. A terminal pane keeps working (no regression). ----------------------
{
  const { runtime, handlers, calls } = loadRuntime();
  await runtime.init();
  handlers.get('shortcut-palette-toggle').onKeyDown(chord(xtermTextarea));
  assert.strictEqual(calls.opened, 1, 'cmd+shift+p still opens the palette from a terminal pane');
}

// --- 4. Off macOS the chord is ctrl+shift+p, and cmd+shift+p is not it. ------
{
  const { runtime, handlers, calls } = loadRuntime({ isMacPlatform: false });
  await runtime.init();
  handlers.get('shortcut-palette-toggle')
    .onKeyDown(chord(cmContent, { metaKey: false, ctrlKey: true }));
  assert.strictEqual(calls.opened, 1, 'ctrl+shift+p opens the palette off macOS, editor focused');
}

// --- 5. Near misses stay unclaimed. -----------------------------------------
{
  const { runtime, handlers, calls } = loadRuntime();
  await runtime.init();
  const toggle = handlers.get('shortcut-palette-toggle');

  assert.strictEqual(
    toggle.onKeyDown(chord(cmContent, { shiftKey: false })), false,
    'cmd+p without shift is not the palette chord',
  );
  assert.strictEqual(
    toggle.onKeyDown(chord(cmContent, { metaKey: false })), false,
    'shift+p alone is not the palette chord — it is a capital P in the editor',
  );
  assert.strictEqual(calls.opened, 0, 'and neither near miss opened the palette');
}

console.log('ok - command palette chord reaches the palette from a focused editor pane');
