// Run: node scripts/tests/test_settings_shortcut_unset_reset.mjs
//
// Shortcut bindings can be UNSET (empty string — the shortcut runtime and
// the native menu both treat that as "no binding") and the whole keymap can
// be RESET to the shipped defaults. Three seams:
//   - the recorder: Delete/Backspace while recording clears the binding
//   - the key box row: its clear (×) button unsets without recording
//   - the store: resetKeyboardToDefaults swaps the pending keymap for the
//     Rust-provided defaults and empties both override maps
import assert from 'node:assert';
import { readFileSync } from 'node:fs';

// --- element stub with enough DOM for makeShortcutKeyBox ------------------
function makeElement(tag) {
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'span').toUpperCase(),
    children: [],
    className: '',
    hidden: false,
    tabIndex: -1,
    type: '',
    title: '',
    _text: '',
    classList: {
      _set: new Set(),
      add(c) { this._set.add(c); },
      remove(c) { this._set.delete(c); },
      contains(c) { return this._set.has(c); },
    },
    appendChild(child) { el.children.push(child); return child; },
    addEventListener(name, handler) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(handler);
    },
    _fire(name, event = {}) {
      for (const handler of listeners.get(name) || []) {
        handler({ stopPropagation() {}, preventDefault() {}, ...event });
      }
    },
    setAttribute() {},
  };
  Object.defineProperty(el, 'textContent', {
    get() { return el._text; },
    set(v) { el._text = v; },
  });
  return el;
}

const window = {};
globalThis.window = window;
const dispatchedEvents = [];
const document = {
  createElement: (tag) => makeElement(tag),
  dispatchEvent: (event) => { dispatchedEvents.push(event.type); },
};
globalThis.document = document;
globalThis.CustomEvent = class CustomEvent { constructor(type) { this.type = type; } };
window.document = document;

// Router stub: the recorder registers one handler while recording.
let routerHandler = null;
window.termlabKeyboardRouter = {
  register(options) {
    routerHandler = options;
    return () => { routerHandler = null; };
  },
};

eval(readFileSync('crates/termlab_tauri/frontend/app/features/settings/renderers.js', 'utf8'));
eval(readFileSync('crates/termlab_tauri/frontend/app/features/settings/store.js', 'utf8'));

const createShortcutRecorder = window.termlabSettingsRenderers.createShortcutRecorder;

function makeRecorderHarness(initialValue) {
  const values = { key: initialValue };
  const recorder = createShortcutRecorder({
    getShortcutValue: () => values.key,
    setShortcutValue: (_ref, value) => { values.key = value; },
    registerGlobalKeyHandler: (name, onKeyDown, isActive, priority) => {
      routerHandler = { name, onKeyDown, isActive, priority };
      return () => { routerHandler = null; };
    },
  });
  return { recorder, values };
}

// Delete while recording unsets the binding and the box reads Unassigned.
{
  const { recorder, values } = makeRecorderHarness('cmd+t');
  const keyBox = makeElement('span');
  recorder.startRecording(keyBox, 'key');
  assert.ok(routerHandler, 'recording registers a key handler');
  const consumed = routerHandler.onKeyDown({
    key: 'Delete', preventDefault() {}, stopPropagation() {},
  });
  assert.equal(consumed, true);
  assert.equal(values.key, '', 'Delete clears the binding');
  assert.equal(keyBox.textContent, 'Unassigned');
  assert.ok(dispatchedEvents.includes('termlab-settings-changed'),
    'clearing marks the settings dirty');
  assert.equal(recorder.isRecording(), false);
}

// The key box row carries a clear button that unsets without recording, and
// it hides once nothing is bound.
{
  dispatchedEvents.length = 0;
  const { recorder, values } = makeRecorderHarness('cmd+t');
  const wrap = recorder.makeShortcutKeyBox('key');
  const [keyBox, clearButton] = wrap.children;
  assert.equal(keyBox.className, 'tl-settings__shortcut-key');
  assert.equal(clearButton.className, 'tl-settings__shortcut-clear');
  assert.equal(clearButton.hidden, false, 'a bound shortcut shows the clear button');

  clearButton._fire('click');
  assert.equal(values.key, '', 'the clear button unsets the binding');
  assert.equal(keyBox.textContent, 'Unassigned');
  assert.equal(clearButton.hidden, true, 'nothing left to clear');
  assert.ok(dispatchedEvents.includes('termlab-settings-changed'));

  // An unassigned row renders the button hidden from the start.
  const { recorder: fresh } = makeRecorderHarness('');
  const freshWrap = fresh.makeShortcutKeyBox('key');
  assert.equal(freshWrap.children[1].hidden, true);
}

// Recording a new combo re-shows the row's clear button (stopRecording's
// __syncClear hook — rows are not re-rendered after a recording lands).
{
  const { recorder, values } = makeRecorderHarness('');
  const wrap = recorder.makeShortcutKeyBox('key');
  const [keyBox, clearButton] = wrap.children;
  assert.equal(clearButton.hidden, true);
  recorder.startRecording(keyBox, 'key');
  routerHandler.onKeyDown({
    key: 't', metaKey: true, ctrlKey: false, altKey: false, shiftKey: false,
    preventDefault() {}, stopPropagation() {},
  });
  assert.equal(values.key, 'cmd+t');
  assert.equal(clearButton.hidden, false, 'the freshly recorded binding is clearable');
}

// The store swaps the pending keymap for the shipped defaults and empties
// both override maps.
{
  const store = window.termlabSettingsStore.create();
  store.applyLoadedSettingsData({
    settings: {
      termlab: {
        keyboard: {
          new_tab: 'ctrl+shift+t',
          close_tab: '',
          plugin_shortcuts: { 'demo:action': 'cmd+9' },
          tool_window_shortcuts: { files: 'cmd+8' },
        },
      },
    },
  });

  store.resetKeyboardToDefaults({
    new_tab: 'cmd+t',
    close_tab: 'cmd+w',
    plugin_shortcuts: {},
    tool_window_shortcuts: {},
  });

  const keyboard = store.getPendingKeyboardMap();
  assert.equal(keyboard.new_tab, 'cmd+t');
  assert.equal(keyboard.close_tab, 'cmd+w', 'an unset binding returns to its default');
  assert.deepStrictEqual(keyboard.plugin_shortcuts, {},
    'plugin overrides clear so manifest keybinds apply again');
  assert.deepStrictEqual(keyboard.tool_window_shortcuts, {});
  assert.equal(store.isDirty(), true, 'a reset is a pending edit, applied like any other');
}

console.log('settings shortcut unset/reset: all assertions passed');
