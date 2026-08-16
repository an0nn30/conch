// Run: node scripts/tests/test_settings_shortcut_recorder_escape.mjs
//
// Regression test for phase 5b final-review finding 2: Escape while
// recording a keyboard shortcut used to close the whole Settings modal and
// discard every unsaved edit, instead of just cancelling the recording.
//
// Root cause: app/panels/settings.js registers all settings key handlers
// (including the shortcut recorder, via createShortcutRecorder's
// registerGlobalKeyHandler dep) at keyboard-router priority 210. Task 1 of
// this phase deleted the settings modal's own priority-210 Escape
// registration (whose job was `if (isRecording()) return false;`), and
// tl-dialog now owns Escape for the modal at priority 225
// (app/ui/tl-dialog.js's registerEscape()) with no knowledge of recording.
// Since higher priority runs first (app/core/keyboard-router.js's
// toSortedHandlers() sorts descending), tl-dialog's Escape handler fired
// before the recorder's and closed the dialog outright.
//
// The fix (app/features/settings/renderers.js's startRecording(), and the
// `priority` parameter threaded through app/panels/settings.js's
// registerGlobalKeyHandler) registers the recorder's key handler at
// priority 230 — above tl-dialog's 225 — the same "register above
// tl-dialog's Escape" pattern command-palette-runtime.js already uses at
// priority 260 (see its comment there). Because the recorder's handler is
// only isActive while actually recording, this is a no-op the rest of the
// time and tl-dialog's Escape still closes the modal normally.
//
// No jsdom in this repo (see test_tl_dialog.mjs for the precedent). This
// loads the REAL app/features/settings/renderers.js (createShortcutRecorder
// is the module that actually owns the fix: passing priority 230) against a
// router stub that reproduces core/keyboard-router.js's priority-descending,
// registration-order-tiebreak dispatch semantics — the same technique
// test_tl_dialog.mjs's "Escape priority" block uses to exercise tl-menu vs.
// tl-dialog. Two things are deliberately NOT loaded, and are instead
// mirrored inline with a citation, because pulling them in for real would
// require reconstructing most of the settings feature's store/section-
// renderer wiring for no additional coverage of THIS fix:
//   - registerGlobalKeyHandler itself (app/panels/settings.js:30-39) — a
//     five-line, side-effect-free wrapper around keyboardRouter.register()
//     that just forwards its priority argument (or defaults to 210); the
//     mirror below matches it exactly, including the default.
//   - tl-dialog's registerEscape() (app/ui/tl-dialog.js:129-149) — only its
//     externally-observable contract matters here (priority 225, isActive
//     while the dialog is topmost, Escape closes it), not its DOM/focus/
//     stacking internals, which test_tl_dialog.mjs already covers directly.
import assert from 'node:assert';

// --- router stub: mirrors app/core/keyboard-router.js's dispatch() --------
function makeRouterStub() {
  let nextId = 1;
  const handlers = new Map();
  return {
    register(options) {
      const id = nextId++;
      handlers.set(id, {
        order: id,
        priority: options && typeof options.priority === 'number' ? options.priority : 0,
        isActive: options && typeof options.isActive === 'function' ? options.isActive : null,
        onKeyDown: options && typeof options.onKeyDown === 'function' ? options.onKeyDown : null,
      });
      return () => handlers.delete(id);
    },
    dispatchEscape() {
      const sorted = Array.from(handlers.values()).sort((a, b) => {
        if (a.priority !== b.priority) return b.priority - a.priority;
        return a.order - b.order;
      });
      for (const entry of sorted) {
        if (entry.isActive && !entry.isActive()) continue;
        if (!entry.onKeyDown) continue;
        if (entry.onKeyDown({ key: 'Escape', preventDefault() {}, stopPropagation() {} }) === true) return true;
      }
      return false;
    },
  };
}

// --- minimal element stub for the shortcut-recorder key box ---------------
function makeElement(tag) {
  const el = {
    tagName: String(tag || 'span').toUpperCase(),
    _text: '',
    classList: {
      _set: new Set(),
      add(c) { this._set.add(c); },
      remove(c) { this._set.delete(c); },
      contains(c) { return this._set.has(c); },
    },
    addEventListener() {},
    setAttribute() {},
  };
  Object.defineProperty(el, 'textContent', {
    get() { return el._text; },
    set(v) { el._text = v; },
  });
  return el;
}

// --- window/document stubs (renderers.js does no DOM work at load time) ---
const window = {};
globalThis.window = window;
const document = { createElement: (tag) => makeElement(tag) };
globalThis.document = document;
window.document = document;
window.termlabKeyboardRouter = makeRouterStub();

const { readFileSync } = await import('node:fs');
eval(readFileSync('crates/termlab_tauri/frontend/app/features/settings/renderers.js', 'utf8'));

const createShortcutRecorder = window.termlabSettingsRenderers.createShortcutRecorder;
assert.equal(typeof createShortcutRecorder, 'function', 'renderers.js must export createShortcutRecorder');

// --- mirror of app/panels/settings.js:30-39 (post-fix) --------------------
function registerGlobalKeyHandler(name, onKeyDown, isActive, priority) {
  return window.termlabKeyboardRouter.register({
    name: name || 'settings-key-handler',
    priority: typeof priority === 'number' ? priority : 210,
    isActive: typeof isActive === 'function' ? isActive : null,
    onKeyDown: (event) => onKeyDown(event) === true,
  });
}

// --- mirror of app/ui/tl-dialog.js's registerEscape() contract ------------
let dialogOpen = true;
let dialogCloseCount = 0;
window.termlabKeyboardRouter.register({
  name: 'tl-dialog',
  priority: 225,
  isActive: () => dialogOpen,
  onKeyDown: (event) => {
    if (event.key !== 'Escape') return false;
    dialogOpen = false;
    dialogCloseCount += 1;
    return true;
  },
});

// --- recorder under test ----------------------------------------------------
const shortcutValues = { 'terminal:new-tab': null };
const recorder = createShortcutRecorder({
  getShortcutValue: (ref) => shortcutValues[ref],
  setShortcutValue: (ref, value) => { shortcutValues[ref] = value; },
  registerGlobalKeyHandler,
});

// --- baseline: Escape with no recording in progress still closes the dialog
{
  assert.equal(recorder.isRecording(), false, 'recorder should not be recording yet');
  const consumed = window.termlabKeyboardRouter.dispatchEscape();
  assert.equal(consumed, true, 'Escape should be consumed by someone');
  assert.equal(dialogOpen, false, 'with no recording active, Escape must still close the dialog normally');
  assert.equal(dialogCloseCount, 1);
  console.log('baseline: Escape closes the dialog when nothing is recording: ok');
}

// reset for the actual bug scenario
dialogOpen = true;
dialogCloseCount = 0;

// --- the bug scenario: Escape while recording must cancel the recording,
// not close the dialog ------------------------------------------------------
{
  const keyBox = makeElement('span');
  recorder.startRecording(keyBox, 'terminal:new-tab');
  assert.equal(recorder.isRecording(), true, 'recorder should be recording after startRecording()');
  assert.equal(keyBox.classList.contains('is-recording'), true);
  assert.equal(keyBox.textContent, 'Press keys...');

  const consumed = window.termlabKeyboardRouter.dispatchEscape();
  assert.equal(consumed, true, 'Escape should still be consumed by someone (the recorder)');
  assert.equal(dialogOpen, true, 'Escape while recording must NOT close the dialog (finding 2)');
  assert.equal(dialogCloseCount, 0, 'the dialog close handler must never have run');
  assert.equal(recorder.isRecording(), false, 'Escape must cancel the in-progress recording');
  assert.equal(keyBox.classList.contains('is-recording'), false, 'the key box must leave recording state');
  assert.equal(keyBox.textContent, 'Unassigned', 'the key box must revert to showing the (unchanged) shortcut value');
  assert.equal(shortcutValues['terminal:new-tab'], null, 'cancelling via Escape must not write a shortcut value');
  console.log('Escape while recording cancels the recording and leaves the dialog open: ok');
}

// --- after the cancelled recording, Escape goes back to closing the dialog
// (the recorder's handler must have unregistered itself, not left a stale
// always-active registration behind) ----------------------------------------
{
  const consumed = window.termlabKeyboardRouter.dispatchEscape();
  assert.equal(consumed, true);
  assert.equal(dialogOpen, false, 'once recording is cancelled, a later Escape must close the dialog again');
  assert.equal(dialogCloseCount, 1);
  console.log('Escape after a cancelled recording closes the dialog normally: ok');
}

console.log('ok');
