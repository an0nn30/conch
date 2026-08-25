// Run: node scripts/tests/test_lsp_settings.mjs
//
// The settings and keyboard contracts must be usable before any LSP runtime
// exists. Exercise the real plain-IIFE modules in a small VM rather than
// asserting source text: the section must render and mutate its settings
// object, and the shortcut runtime must publish actions only in editor panes.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

function load(file, sandbox) {
  vm.runInContext(fs.readFileSync(path.join(APP, file), 'utf8'), sandbox, { filename: file });
}

function makeSandbox() {
  const dispatched = [];
  const sandbox = {
    console,
    document: { activeElement: null },
    CustomEvent: class CustomEvent {
      constructor(type, init = {}) {
        this.type = type;
        this.detail = init.detail;
      }
    },
    dispatchEvent(event) { dispatched.push(event); },
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  return { sandbox, dispatched };
}

// Editor settings expose every stable product-level LSP preference and mutate
// the pending settings object that Apply will serialize.
{
  const { sandbox } = makeSandbox();
  load('features/settings/sections-editor.js', sandbox);

  const rows = [];
  const pendingSettings = { editor: {} };
  const rendered = sandbox.termlabSettingsSectionsEditor.renderEditor({}, {
    pendingSettings,
    addSectionLabel() {},
    makeCheckbox(checked, onChange) { return { checked, onChange }; },
    addRow(_container, label, description, control) {
      const row = { label, description, control, targetId: null };
      rows.push(row);
      return row;
    },
    setRowTarget(row, targetId) { row.targetId = targetId; },
  });

  assert.equal(rendered, true);
  const labels = rows.map((row) => row.label);
  for (const label of [
    'Enable language services',
    'Suggestions while typing',
    'TypeScript / JavaScript',
    'JSON',
    'Python',
    'Rust',
    'Go',
    'C / C++',
    'Java',
  ]) {
    assert.ok(labels.includes(label), `Editor settings expose ${label}`);
  }
  const lspRow = rows.find((row) => row.label === 'Enable language services');
  lspRow.control.onChange(false);
  assert.equal(pendingSettings.editor.lsp.enabled, false, 'the master control changes the saved draft');
}

// Keyboard settings expose the seven editor actions but deliberately omit
// hover: it remains command-palette-only until multi-step chords exist.
{
  const { sandbox } = makeSandbox();
  load('features/settings/constants.js', sandbox);
  load('features/settings/store.js', sandbox);
  const store = sandbox.termlabSettingsStore.create();
  const labels = store.KEYBOARD_CORE_LABELS;
  const editorGroup = store.KEYBOARD_CORE_GROUPS.find((group) => group.label === 'Editor');

  const expectedKeys = [
    'editor_completion',
    'editor_signature_help',
    'editor_go_to_definition',
    'editor_navigate_back',
    'editor_navigate_forward',
    'editor_next_problem',
    'editor_previous_problem',
  ];
  for (const key of expectedKeys) {
    assert.equal(typeof labels[key], 'string', `${key} has a Keymap label`);
    assert.ok(editorGroup.keys.includes(key), `${key} is in the Editor keymap group`);
  }
  assert.ok(!Object.hasOwn(labels, 'editor_hover'), 'hover has no configurable shortcut');
}

// A configured editor-only LSP action is consumed in an editor and publishes
// the action event that future editor modules subscribe to.
{
  const { sandbox, dispatched } = makeSandbox();
  load('input-runtime.js', sandbox);
  load('shortcut-runtime.js', sandbox);

  const handlers = new Map();
  sandbox.termlabKeyboardRouter = { register(handler) { handlers.set(handler.name, handler); } };
  const runtime = sandbox.termlabShortcutRuntime.create({
    invoke: async (cmd) => {
      if (cmd === 'get_all_settings') {
        return { termlab: { keyboard: { editor_completion: 'ctrl+space' } } };
      }
      if (cmd === 'get_plugin_menu_items') return [];
      return null;
    },
    isMacPlatform: false,
    isTextInputTarget: sandbox.termlabInputRuntime.create().isTextInputTarget,
    handleMenuAction() {},
    shouldDebugKeyEvent: () => false,
    formatKeyEventForDebug: () => '',
    shortcutDebugEnabled: false,
    openCommandPalette() {},
    closeCommandPalette() {},
    isCommandPaletteOpen: () => false,
    getTabIds: () => [],
    activateTab() {},
    getCurrentPane: () => ({ kind: 'editor' }),
    writeTextToCurrentPane() {},
    getActiveTab: () => null,
    getFocusedPaneId: () => null,
    setFocusedPane() {},
    findAdjacentPane: () => null,
  });

  await runtime.init();
  const consumed = handlers.get('shortcut-fallbacks').onKeyDown({
    metaKey: false, ctrlKey: true, altKey: false, shiftKey: false,
    code: 'Space', key: ' ', target: { tagName: 'DIV', className: '' },
  });

  assert.equal(consumed, true);
  assert.deepEqual(dispatched.map((event) => event.type), ['termlab:editor-completion']);
}

// The standalone Settings bootstrap publishes the exact client it creates,
// so trusted-project management reaches Tauri without a terminal window's
// main-runtime having populated termlabServices first.
{
  const invoked = [];
  const { sandbox } = makeSandbox();
  sandbox.document.createElement = () => ({
    className: '', textContent: '', disabled: false,
    addEventListener() {},
  });
  sandbox.__TAURI__ = {
    core: {
      async invoke(command, args) {
        invoked.push([command, args]);
        if (command === 'lsp_trusted_projects') return [];
        return null;
      },
    },
    event: { listen: async () => () => {} },
    window: { getCurrentWindow: () => ({ listen: async () => () => {} }) },
  };
  load('core/tauri-client.js', sandbox);
  load('features/editor/lsp-state.js', sandbox);
  load('features/editor/lsp-bridge.js', sandbox);
  load('features/editor/project-context.js', sandbox);
  const realClient = sandbox.termlabTauriClient.create({ tauri: sandbox.__TAURI__ });
  sandbox.termlabTauriClient.publish(realClient);
  await sandbox.termlabProjectContext.renderTrustedProjects({}, {
    addSectionLabel() {},
    addRow() {},
  });
  assert.equal(sandbox.termlabServices.tauriClient, realClient);
  assert.deepEqual(invoked[0], ['lsp_trusted_projects', undefined]);
}

console.log('LSP settings contracts: all assertions passed');
