// Run: node scripts/tests/test_window_close_answer.mjs
//
// The webview's half of the close handshake (event-wiring-runtime.js). Rust
// prevents the close and emits `window-close-requested` / `app-quit-requested`;
// this is the code that has to answer, every time, with a boolean — because a
// window whose frontend never answers is a window that can never be closed.
//
// Stubs: `invoke` records the command and args and resolves undefined, which
// is what the three void `#[tauri::command]`s in close_guard.rs return over
// the IPC; `listenOnCurrentWindow` resolves to an unlisten function, as
// @tauri-apps/api's Window.listen does. `confirmAllDirty` is stubbed to return
// a Promise<boolean> — the real signature; its own behaviour is covered by
// test_editor_close_guards.mjs.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const WIRING = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/event-wiring-runtime.js',
);

function makeHarness(confirmAllDirty) {
  const sandbox = { console, setTimeout, clearTimeout, Promise, Map, Set };
  sandbox.window = sandbox;
  sandbox.document = { addEventListener() {}, getElementById: () => null };
  sandbox.navigator = { platform: 'MacIntel' };
  if (confirmAllDirty) {
    sandbox.termlabEditorService = { confirmAllDirty };
  }

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(WIRING, 'utf8'), sandbox, { filename: WIRING });

  const invocations = [];
  const listeners = new Map();
  const statuses = [];

  const runtime = sandbox.termlabEventWiringRuntime.create({
    invoke: (command, args) => {
      invocations.push({ command, args });
      return Promise.resolve(undefined);
    },
    listen: () => Promise.resolve(() => {}),
    listenOnCurrentWindow: (name, handler) => {
      listeners.set(name, handler);
      return Promise.resolve(() => {});
    },
    currentWindowLabel: 'main',
    terminalHostEl: null,
    tabBarEl: null,
    tabs: new Map(),
    panes: new Map(),
    getActiveTabId: () => null,
    getFocusedPaneId: () => null,
    getCurrentPane: () => null,
    getCurrentTab: () => null,
    closeTab: () => {},
    createTab: () => {},
    closePane: () => {},
    splitPane: () => {},
    renameActiveTab: () => {},
    setFocusedPane: () => {},
    startTabRename: () => {},
    fitAndResizeTab: () => {},
    debouncedSaveLayout: () => {},
    showStatus: (m) => statuses.push(m),
    isTextInputTarget: () => false,
    writeTextToCurrentPane: () => {},
    pasteIntoCurrentPane: () => {},
    openCommandPalette: () => {},
    closeCommandPalette: () => {},
    isCommandPaletteOpen: () => false,
    refocusActiveTerminal: () => {},
    terminalRuntime: {},
    shortcutDebugEnabled: false,
    getZoom: () => 1,
    setZoom: () => {},
    getThemeState: () => ({}),
    setThemeState: () => {},
    getTermConfigState: () => ({}),
    setTermConfigState: () => {},
    fontFallbacks: [],
    activateTab: () => {},
  });

  return { runtime, invocations, listeners, statuses, sandbox };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const results = [];
const check = (name, fn) => results.push({ name, fn });

check('the window arms the guard once its listeners are live', async () => {
  const h = makeHarness(async () => true);
  await h.runtime.init();
  assert.ok(h.listeners.has('window-close-requested'), 'the close listener is registered');
  assert.ok(h.listeners.has('app-quit-requested'), 'the quit listener is registered');
  const arm = h.invocations.find((i) => i.command === 'window_close_guard_arm');
  assert.ok(arm, 'and only then is the guard armed');
});

check('a clean window answers the close request with allow: true', async () => {
  const h = makeHarness(async () => true);
  await h.runtime.init();
  h.listeners.get('window-close-requested')({});
  await tick();
  const answer = h.invocations.find((i) => i.command === 'confirm_window_close');
  assert.ok(answer, 'an answer was sent');
  assert.strictEqual(answer.args.allow, true);
});

check('a cancelled prompt answers with allow: false', async () => {
  const h = makeHarness(async () => false);
  await h.runtime.init();
  h.listeners.get('window-close-requested')({});
  await tick();
  const answer = h.invocations.find((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answer.args.allow, false, 'the window stays open');
});

check('quit is answered on its own channel', async () => {
  const h = makeHarness(async () => false);
  await h.runtime.init();
  h.listeners.get('app-quit-requested')({});
  await tick();
  const answer = h.invocations.find((i) => i.command === 'quit_vote');
  assert.strictEqual(answer.args.allow, false, 'a cancelled quit votes no');
  assert.ok(
    !h.invocations.some((i) => i.command === 'confirm_window_close'),
    'and does not also answer the window-close channel',
  );
});

check('a second close request while the prompt is up opens no second prompt', async () => {
  let resolvePrompt;
  let prompts = 0;
  const h = makeHarness(() => { prompts += 1; return new Promise((r) => { resolvePrompt = r; }); });
  await h.runtime.init();

  h.listeners.get('window-close-requested')({});
  await tick();
  h.listeners.get('window-close-requested')({});   // user clicks the X again
  h.listeners.get('window-close-requested')({});
  await tick();

  assert.strictEqual(prompts, 1, 'one prompt, not a stack of dialogs');
  const early = h.invocations.filter((i) => i.command === 'confirm_window_close');
  assert.strictEqual(early.length, 2, 'the swallowed requests are refused, not dropped');
  assert.ok(early.every((i) => i.args.allow === false), 'and refused safely');

  resolvePrompt(true);
  await tick();
  const answers = h.invocations.filter((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answers.length, 3);
  assert.strictEqual(answers[2].args.allow, true, 'the real answer still gets through');
});

// Regression: one shared latch used to make this drop the quit request
// entirely. Rust's quit poll then waits forever on a vote that never comes,
// and request_quit early-returns while a quit is pending — so Cmd+Q is dead
// for the rest of the session.
check('a quit request arriving while a close prompt is up is still answered', async () => {
  let resolvePrompt;
  let calls = 0;
  const h = makeHarness(() => {
    calls += 1;
    // Only the first prompt is held open; later ones answer straight away, so
    // the "does Cmd+Q still work afterwards" half of this check is about the
    // latch and not about a promise the test forgot to resolve.
    return calls === 1 ? new Promise((r) => { resolvePrompt = r; }) : Promise.resolve(false);
  });
  await h.runtime.init();

  h.listeners.get('window-close-requested')({});   // X clicked, prompt opens
  await tick();
  h.listeners.get('app-quit-requested')({});       // then cmd+Q
  await tick();

  const votes = h.invocations.filter((i) => i.command === 'quit_vote');
  assert.strictEqual(votes.length, 1, 'the quit poll got a reply rather than silence');
  assert.strictEqual(votes[0].args.allow, false, 'and it is the safe one');

  // The user then cancels the close prompt; the window stays, and a later
  // quit must still work.
  resolvePrompt(false);
  await tick();
  h.listeners.get('app-quit-requested')({});
  await tick();
  assert.strictEqual(
    h.invocations.filter((i) => i.command === 'quit_vote').length,
    2,
    'cmd+Q still works afterwards',
  );
});

check('a close request arriving while a quit prompt is up is answered too', async () => {
  let resolvePrompt;
  const h = makeHarness(() => new Promise((r) => { resolvePrompt = r; }));
  await h.runtime.init();

  h.listeners.get('app-quit-requested')({});
  await tick();
  h.listeners.get('window-close-requested')({});
  await tick();

  const answers = h.invocations.filter((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answers.length, 1, 'answered on its own channel');
  assert.strictEqual(answers[0].args.allow, false, 'the window stays open');
  resolvePrompt(true);
  await tick();
});

check('a later close request is served after the first finishes', async () => {
  const h = makeHarness(async () => false);
  await h.runtime.init();
  h.listeners.get('window-close-requested')({});
  await tick();
  h.listeners.get('window-close-requested')({});
  await tick();
  const answers = h.invocations.filter((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answers.length, 2, 'the guard is not a one-shot');
});

check('a thrown prompt answers "no" rather than leaving Rust waiting', async () => {
  const h = makeHarness(async () => { throw new Error('pane access exploded'); });
  await h.runtime.init();
  h.listeners.get('window-close-requested')({});
  await tick();
  const answers = h.invocations.filter((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answers.length, 1, 'an answer was still sent');
  assert.strictEqual(answers[0].args.allow, false, 'and it is the safe one');
  assert.ok(h.statuses.some((s) => /unsaved changes/i.test(s)), 'the user is told why');
});

check('no editor service means nothing to lose, so the close goes ahead', async () => {
  // A window that refused here could never be closed at all.
  const h = makeHarness(null);
  await h.runtime.init();
  h.listeners.get('window-close-requested')({});
  await tick();
  const answer = h.invocations.find((i) => i.command === 'confirm_window_close');
  assert.strictEqual(answer.args.allow, true);
});

let failed = 0;
for (const { name, fn } of results) {
  try {
    await fn();
    console.log(`  ok   ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${error && error.message}`);
  }
}
if (failed) {
  console.log(`window close answer: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`window close answer: all ${results.length} checks passed`);
}
