// Run: node scripts/tests/test_file_dialog_proxy.mjs
//
// `chooseFile` as a PROXY across the window boundary.
//
// The chooser no longer renders in this window: `chooseFile` raises a scrim,
// asks Rust to open a chooser window, and waits for the `chooser-resolved`
// event that carries the answer back. Everything below is about that hand-off
// and nothing about the chooser's own UI — the view has its own suite
// (test_file_dialog.mjs).
//
// Loaded: app/features/editor/file-dialog.js ALONE, over a scripted transport.
// That isolation is the point — the proxy must not need the view, the model,
// tl-dialog or a window label to do its job.
//
// The three things most likely to break, and why each is pinned:
//   1. THE SCRIM. It is raised BEFORE the invoke and lowered in the settle
//      latch's finally. A scrim that outlives its chooser is a locked app, so
//      it is asserted lowered on the resolution path AND on the invoke
//      rejection path, along with `inert`/`aria-hidden` on the app root.
//   2. THE SETTLE LATCH. Every failure route — a listen that rejects, an
//      invoke that rejects, anything thrown in the async body — must settle
//      the promise null and release `activeChoice`. The bridge this replaces
//      could throw inside a deferred `.then` and leave `activeChoice` claimed
//      forever with an unsettled promise: every later ⌘O then refused
//      silently.
//   3. THE REQ-ID RACE. The resolution event and the `open_file_chooser`
//      return both originate in Rust, and a fast cancel lets the event win.
//      Check 5 fires the resolution synchronously from INSIDE the invoke stub
//      to prove the one-event buffer catches it.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');

let failures = 0;
let ran = 0;
async function checkAsync(name, fn) {
  ran++;
  try {
    await fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}
function check(name, fn) {
  ran++;
  try {
    fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}

const tick = () => new Promise((resolve) => setImmediate(resolve));
const settle = async (n = 10) => { for (let i = 0; i < n; i++) await tick(); };

// A promise that never settles is the pre-fix failure mode for most of this
// file, and awaiting one directly hangs the run instead of failing it.
const TIMEOUT = Symbol('timeout');
async function settles(promise, what) {
  let timer = null;
  const raced = await Promise.race([
    promise.then((value) => ({ value })),
    new Promise((r) => { timer = setTimeout(() => r(TIMEOUT), 1000); }),
  ]);
  clearTimeout(timer);
  assert.notStrictEqual(raced, TIMEOUT, `${what} never settled`);
  return raced.value;
}

// ---------------------------------------------------------------------------
// Minimal DOM — only what the proxy touches: createElement, body append/remove
// and getElementById for the app root it marks inert.
// ---------------------------------------------------------------------------

function makeElement(tag) {
  const attrs = new Map();
  return {
    tagName: String(tag || 'div').toUpperCase(),
    className: '',
    children: [],
    parentNode: null,
    appendChild(child) { this.children.push(child); child.parentNode = this; return child; },
    removeChild(child) {
      const i = this.children.indexOf(child);
      if (i >= 0) this.children.splice(i, 1);
      child.parentNode = null;
      return child;
    },
    setAttribute(name, value) { attrs.set(name, String(value)); },
    removeAttribute(name) { attrs.delete(name); },
    getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
    hasAttribute(name) { return attrs.has(name); },
  };
}

// ---------------------------------------------------------------------------
// The transport: a scripted `invoke` and a capturable `chooser-resolved`
// listener, in place of Rust and the event bus.
// ---------------------------------------------------------------------------

function makeHarness(options) {
  const opts = options || {};
  const doc = { createElement: (tag) => makeElement(tag) };
  doc.body = makeElement('body');
  const appRoot = makeElement('div');
  doc.getElementById = (id) => (id === 'app' ? appRoot : null);

  const sandbox = { console, document: doc, setTimeout, clearTimeout, Promise };
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const log = [];                 // every observable step, in order
  const invokes = [];             // [command, args]
  const listeners = [];           // live chooser-resolved handlers
  const toasts = [];
  let unlistenCount = 0;

  sandbox.toast = {
    error: (title, body) => toasts.push({ title, body }),
    success: () => {},
  };

  const openCalls = [];
  sandbox.termlabEditorService = {
    openLocalFile: (p) => { openCalls.push(['openLocalFile', p]); return Promise.resolve(); },
    openRemoteFile: (d) => { openCalls.push(['openRemoteFile', d]); return Promise.resolve(); },
    saveAs: (pane, target) => { openCalls.push(['saveAs', target]); return Promise.resolve(); },
  };

  // Gates. `gateListen` / `gateOpen` leave those two promises PENDING until
  // the check releases them, which is the only way to stand inside the two
  // windows the proxy's cancel guards exist for: cancelled while the listener
  // is still attaching, and cancelled while the window is still being built.
  // FIFO, because a check can have two sessions in flight at once.
  const listenGates = [];
  const openGates = [];

  // Rust's registry, in miniature: at most one live chooser per parent.
  let live = null;                 // { reqId } once the entry has been inserted
  let nextReqId = 1;
  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        log.push(`invoke:${command}`);
        invokes.push([command, args]);
        if (opts.invoke) {
          const handled = opts.invoke(command, args, api);
          if (handled !== undefined) return handled;
        }
        if (command === 'open_file_chooser') {
          // Cancel-and-recreate, at command receipt — the registry mutation
          // happens before the window is built, exactly as it does in Rust
          // (`take_pending` -> emit the old session's cancel -> `open`). An
          // open against a live entry DISPLACES it: the displaced session is
          // resolved null under its own id, and the new chooser gets an id of
          // its own. It is never handed the live one's id.
          if (live) {
            const displaced = live.reqId;
            live = null;
            api.resolve(null, displaced);
          }
          const reqId = nextReqId++;
          live = { reqId };
          // Only the REPLY is gated: the entry exists from here on, the window
          // is what is still being built.
          if (opts.gateOpen) return new Promise((resolve) => { openGates.push(() => resolve(reqId)); });
          return Promise.resolve(reqId);
        }
        if (command === 'cancel_file_chooser') {
          // The registry's `cancel`, in miniature: `reqId` scopes it, and a
          // cancel that names a chooser which is no longer live resolves
          // nothing and leaves whatever IS live alone.
          const named = args && args.reqId != null ? args.reqId : null;
          if (!live) return Promise.resolve(null);
          if (named !== null && named !== live.reqId) return Promise.resolve(null);
          const reqId = live.reqId;
          live = null;
          api.resolve(null, reqId);          // Rust emits the answer back
          return Promise.resolve(null);
        }
        return Promise.resolve(null);
      },
      listenOnCurrentWindow(name, handler) {
        log.push(`listen:${name}`);
        if (opts.listen) {
          const handled = opts.listen(name, handler);
          if (handled !== undefined) return handled;
        }
        const entry = { name, handler };
        const unlisten = () => {
          unlistenCount += 1;
          const i = listeners.indexOf(entry);
          if (i >= 0) listeners.splice(i, 1);
        };
        if (opts.gateListen) {
          // Not live until released: a handler registered before the await
          // resolves would make the guard untestable.
          return new Promise((resolve) => {
            listenGates.push(() => { listeners.push(entry); resolve(unlisten); });
          });
        }
        listeners.push(entry);
        // `unlistenThrows` models an event bus that fails on release: the
        // handler stays registered AND the caller gets an exception, which is
        // the harshest shape of the failure.
        if (opts.unlistenThrows) {
          return Promise.resolve(() => { throw new Error('event bus went away'); });
        }
        return Promise.resolve(unlisten);
      },
    },
  };

  vm.runInContext(
    fs.readFileSync(path.join(APP, 'features/editor/file-dialog.js'), 'utf8'),
    sandbox,
    { filename: 'features/editor/file-dialog.js' },
  );

  const api = {
    sandbox,
    doc,
    appRoot,
    log,
    invokes,
    toasts,
    openCalls,
    fd: sandbox.termlabFileDialog,
    commands: () => invokes.map(([c]) => c),
    argsFor: (command) => (invokes.find(([c]) => c === command) || [])[1],
    unlistenCount: () => unlistenCount,
    listenerCount: () => listeners.length,
    // Release the oldest gated listen / open, in the order they were made.
    releaseListen: () => { (listenGates.shift() || (() => {}))(); },
    releaseOpen: () => { (openGates.shift() || (() => {}))(); },
    // The scrim, as the rest of the app would see it: an element on body.
    scrims: () => doc.body.children.filter((el) => el.className === 'tl-chooser-scrim'),
    scrimUp: () => doc.body.children.some((el) => el.className === 'tl-chooser-scrim'),
    // Rust answering. `reqId` defaults to the one live chooser's.
    resolve: (choice, reqId) => {
      const payload = { reqId: reqId === undefined ? 1 : reqId, choice: choice === undefined ? null : choice };
      if (live && live.reqId === payload.reqId) live = null;   // the entry is consumed
      log.push(`event:chooser-resolved:${payload.reqId}`);
      for (const entry of listeners.slice()) entry.handler({ payload });
    },
    // Which chooser the registry currently holds, if any.
    liveReqId: () => (live ? live.reqId : null),
    // The wire delivering a message that was dispatched earlier — the transport
    // called directly, bypassing the proxy.
    wire: (command, args) => sandbox.termlabServices.tauriClient.invoke(command, args),
  };
  return api;
}

// The remote scope the view puts in a choice, and the descriptor it must
// rehydrate into. Both halves are the contract test_editor_remote_transfer.mjs
// asserts downstream (`test_editor_remote_transfer.mjs:235-236`): paneId,
// remotePath, the CLEAN hostLabel and the size drive editor_temp_path, so a
// dropped or renamed field there splits one remote file across two tabs.
const REMOTE_CHOICE = {
  scope: {
    id: 'remote:4',
    kind: 'remote',
    label: 'me@example.com (pane 4)',   // the DISPLAY caption — must not route
    hostLabel: 'me@example.com',        // the identity string — must route
    paneId: 4,
    start: '/home/me',
  },
  path: '/home/me/notes.txt',
  entry: { name: 'notes.txt', is_dir: false, size: 120, modified: null },
};

// ---------------------------------------------------------------------------
// 1-2. The activeChoice guards, now with a focus call on the shared path
// ---------------------------------------------------------------------------

console.log('file dialog proxy: the one-chooser-at-a-time guards');

await checkAsync('a same-mode second call shares the ONE promise and focuses the window', async () => {
  const h = makeHarness({});
  const first = h.fd._chooseFile();
  await settle();
  assert.deepStrictEqual(h.commands(), ['open_file_chooser'], 'precondition: one chooser opened');

  const second = h.fd._chooseFile();
  assert.strictEqual(second, first, 'the SAME promise object, not a second chooser');
  await settle();
  assert.deepStrictEqual(h.commands(), ['open_file_chooser', 'focus_file_chooser'],
    'and the window already up is focused rather than a second one opened');
  assert.strictEqual(h.scrims().length, 1, 'one scrim, not two');

  h.resolve(null);
  await settles(first, 'the shared chooser');
});

await checkAsync('a cross-mode second call resolves null and invokes nothing', async () => {
  const h = makeHarness({});
  const saving = h.fd._chooseFile({ mode: 'save', filename: 'Untitled' });
  await settle();
  const before = h.commands().slice();

  const opening = h.fd._chooseFile();               // ⌘O over a save chooser
  assert.strictEqual(await settles(opening, 'the refused ⌘O'), null, 'refused, not shared');
  await settle();
  assert.deepStrictEqual(h.commands(), before,
    'no open, no focus — the window on screen is the one being answered');
  assert.strictEqual(h.scrims().length, 1);

  h.resolve(null);
  await settles(saving, 'the save chooser');
});

// ---------------------------------------------------------------------------
// 3. The scrim
// ---------------------------------------------------------------------------

console.log('file dialog proxy: the scrim');

await checkAsync('the scrim is up BEFORE open_file_chooser is invoked', async () => {
  let scrimAtInvoke = null;
  let inertAtInvoke = null;
  const h = makeHarness({
    invoke: (command, args, api) => {
      if (command !== 'open_file_chooser') return undefined;
      scrimAtInvoke = api.scrimUp();
      inertAtInvoke = api.appRoot.hasAttribute('inert');
      return undefined;
    },
  });
  const choice = h.fd._chooseFile();
  await settle();

  assert.strictEqual(scrimAtInvoke, true,
    'the window opens over a parent that is already blocked — never the other way round');
  assert.strictEqual(inertAtInvoke, true, 'and inert by then too');
  assert.deepStrictEqual(h.log.slice(0, 2), ['listen:chooser-resolved', 'invoke:open_file_chooser'],
    'the listener is attached before the invoke: a resolution cannot outrun it');

  h.resolve(null);
  await settles(choice, 'the chooser');
});

await checkAsync('the scrim comes down when the resolution arrives', async () => {
  const h = makeHarness({});
  const choice = h.fd._chooseFile();
  await settle();
  assert.strictEqual(h.scrimUp(), true, 'precondition');
  assert.strictEqual(h.appRoot.getAttribute('aria-hidden'), 'true', 'precondition');

  h.resolve(REMOTE_CHOICE);
  assert.deepStrictEqual(await settles(choice, 'the chooser'), REMOTE_CHOICE);

  assert.strictEqual(h.scrimUp(), false, 'a scrim that outlives its chooser is a locked app');
  assert.strictEqual(h.appRoot.hasAttribute('inert'), false);
  assert.strictEqual(h.appRoot.hasAttribute('aria-hidden'), false);
  assert.strictEqual(h.unlistenCount(), 1, 'and the event listener was released');
});

await checkAsync('the scrim comes down when open_file_chooser REJECTS', async () => {
  // The bridge this replaces threw inside a deferred `.then` and left
  // `activeChoice` claimed with a promise nobody would ever settle. Every
  // later ⌘O was then refused in silence.
  const h = makeHarness({
    invoke: (command) => (command === 'open_file_chooser'
      ? Promise.reject(new Error('chooser window failed to build'))
      : undefined),
  });
  const choice = h.fd._chooseFile();

  assert.strictEqual(await settles(choice, 'a chooser whose window would not open'), null,
    'it resolves null — a failure to open is not a rejection to the caller');
  await settle();
  assert.strictEqual(h.scrimUp(), false, 'the scrim came down in the finally');
  assert.strictEqual(h.appRoot.hasAttribute('inert'), false);
  assert.strictEqual(h.appRoot.hasAttribute('aria-hidden'), false);
  assert.strictEqual(h.toasts.length, 1, 'and the user was told');
  assert.match(h.toasts[0].title, /Cannot Open File/);

  // The claim released: a later ⌘O really opens another chooser.
  const again = h.fd._chooseFile();
  await settle();
  assert.strictEqual(h.commands().filter((c) => c === 'open_file_chooser').length, 2,
    'activeChoice was released — the next ⌘O is not swallowed');
  h.resolve(null, 1);
  await settles(again, 'the second chooser');
});

await checkAsync('an unlisten that THROWS still lets the answer land', async () => {
  // Teardown is best effort; the answer is not. If releasing the listener could
  // throw out of the latch it would skip `resolveChoice` and leave
  // `activeChoice` claimed on a promise nobody will ever settle — and every
  // later same-mode ⌘O takes the share branch and hangs on that dead promise.
  const h = makeHarness({ unlistenThrows: true });
  const first = h.fd._chooseFile();
  await settle();
  h.resolve({ scope: { kind: 'local' }, path: '/a.txt', entry: null }, 1);

  const value = await settles(first, 'a chooser whose unlisten threw');
  assert.ok(value, 'the pick still came back');
  assert.strictEqual(value.path, '/a.txt');
  await settle();
  assert.strictEqual(h.scrimUp(), false, 'and the scrim still came down');

  // The claim released: the next ⌘O opens a chooser instead of sharing a dead
  // promise.
  const second = h.fd._chooseFile();
  assert.notStrictEqual(second, first, 'a new session, not the dead one');
  await settle();
  assert.strictEqual(h.commands().filter((c) => c === 'open_file_chooser').length, 2);
  h.resolve(null, 2);
  assert.strictEqual(await settles(second, 'the next chooser'), null);
});

await checkAsync('a listen that rejects settles null with the scrim down too', async () => {
  const h = makeHarness({
    listen: () => Promise.reject(new Error('event bus unavailable')),
  });
  const choice = h.fd._chooseFile({ mode: 'save' });
  assert.strictEqual(await settles(choice, 'a chooser with no event transport'), null);
  await settle();
  assert.strictEqual(h.scrimUp(), false);
  assert.strictEqual(h.appRoot.hasAttribute('inert'), false);
  assert.deepStrictEqual(h.commands(), [], 'and no window was opened for nobody to hear');
  assert.match(h.toasts[0].title, /Cannot Save File/);
});

// ---------------------------------------------------------------------------
// 4-5. The reqId
// ---------------------------------------------------------------------------

console.log('file dialog proxy: reqId matching');

await checkAsync('a stale reqId is ignored; the matching one settles', async () => {
  const h = makeHarness({});
  const choice = h.fd._chooseFile();
  await settle();
  const reqId = 1;                                   // the id the stub handed out

  h.resolve({ scope: { kind: 'local' }, path: '/wrong', entry: null }, reqId + 41);
  await settle();
  assert.strictEqual(h.scrimUp(), true, 'a stale answer settles nothing');

  h.resolve({ scope: { kind: 'local' }, path: '/right', entry: null }, reqId);
  const value = await settles(choice, 'the chooser');
  assert.strictEqual(value.path, '/right');
});

await checkAsync('THE RACE: a resolution that arrives before the reqId does still lands', async () => {
  // Both the invoke's return and the event originate in Rust after the
  // registry insert, and a fast cancel (the user hitting Escape on the new
  // window, or a parent-death sweep) lets the event win. Fired synchronously
  // from inside the invoke stub, which is as early as it can possibly be.
  const h = makeHarness({
    invoke: (command, args, api) => {
      if (command !== 'open_file_chooser') return undefined;
      api.resolve({ scope: { kind: 'local' }, path: '/home/u/fast.txt', entry: null }, 1);
      return Promise.resolve(1);
    },
  });
  const choice = h.fd._chooseFile();
  const value = await settles(choice, 'a chooser answered before its reqId was known');
  assert.ok(value, 'the answer was buffered, not dropped');
  assert.strictEqual(value.path, '/home/u/fast.txt');
  await settle();
  assert.strictEqual(h.scrimUp(), false, 'and the scrim came down with it');
});

// ---------------------------------------------------------------------------
// 6. cancelForPane
// ---------------------------------------------------------------------------

console.log('file dialog proxy: cancelForPane');

await checkAsync('cancelForPane cancels the chooser opened FOR that pane', async () => {
  const h = makeHarness({});
  const pane = { kind: 'editor', tabId: 1 };
  const saving = h.fd._chooseFile({ mode: 'save', filename: 'Untitled', pane });
  await settle();

  assert.strictEqual(h.fd.cancelForPane(pane), true, 'it reports that there was one');
  assert.strictEqual(await settles(saving, 'the cancelled chooser'), null);
  await settle();
  assert.ok(h.commands().includes('cancel_file_chooser'),
    'Rust closes the window — cancel_file_chooser is passed reqId, scoping the resolve to this chooser');
  assert.strictEqual(h.scrimUp(), false);
  assert.strictEqual(h.appRoot.hasAttribute('inert'), false);
});

await checkAsync('cancelForPane for a DIFFERENT pane does nothing at all', async () => {
  const h = makeHarness({});
  const pane = { kind: 'editor', tabId: 1 };
  const other = { kind: 'editor', tabId: 2 };
  const saving = h.fd._chooseFile({ mode: 'save', filename: 'Untitled', pane });
  await settle();
  const before = h.commands().slice();

  assert.strictEqual(h.fd.cancelForPane(other), false);
  await settle();
  assert.deepStrictEqual(h.commands(), before, 'no cancel was sent');
  assert.strictEqual(h.scrimUp(), true, 'and the chooser is still up');

  h.resolve(null);
  await settles(saving, 'the untouched chooser');
});

// ---------------------------------------------------------------------------
// 6b. Cancelling DURING setup — the two windows the checks above step over
// ---------------------------------------------------------------------------
//
// Both checks above cancel a session that is fully set up. The proxy's two
// setup guards live in the windows before that: between the listen and its
// resolution, and between `open_file_chooser` and its return. The gated
// transport is what lets a check stand inside them.

console.log('file dialog proxy: cancelling mid-setup');

await checkAsync('a cancel while the listener is still attaching opens no window', async () => {
  const h = makeHarness({ gateListen: true });
  const pane = { kind: 'editor', tabId: 1 };
  const saving = h.fd._chooseFile({ mode: 'save', filename: 'Untitled', pane });
  await settle();
  assert.deepStrictEqual(h.commands(), [],
    'precondition: the listen is still pending, so nothing has been opened');

  assert.strictEqual(h.fd.cancelForPane(pane), true);
  assert.strictEqual(await settles(saving, 'the chooser cancelled mid-attach'), null);
  await settle();
  assert.deepStrictEqual(h.commands(), ['cancel_file_chooser']);

  h.releaseListen();          // the listener finally attaches, after the cancel
  await settle();

  assert.deepStrictEqual(h.commands(), ['cancel_file_chooser'],
    'no window is built for a session that is already over');
  assert.strictEqual(h.unlistenCount(), 1,
    'and the listener that did attach was released rather than left running');
  assert.strictEqual(h.listenerCount(), 0);
  assert.strictEqual(h.scrimUp(), false);
  assert.deepStrictEqual(h.toasts, [],
    'a deliberate cancel is not a failure — nothing is reported to the user');
});

await checkAsync('a cancel while the window is being built re-sends the cancel', async () => {
  // The first cancel raced ahead of the registry insert, so Rust force-resolved
  // nothing (chooser_window.rs:598-604 returns Ok on None). Without the second
  // one the user is left looking at a chooser window nobody is listening to.
  const h = makeHarness({ gateOpen: true });
  const pane = { kind: 'editor', tabId: 1 };
  const saving = h.fd._chooseFile({ mode: 'save', filename: 'Untitled', pane });
  await settle();
  assert.deepStrictEqual(h.commands(), ['open_file_chooser'],
    'precondition: the build is in flight');

  assert.strictEqual(h.fd.cancelForPane(pane), true);
  assert.strictEqual(await settles(saving, 'the chooser cancelled mid-build'), null);
  await settle();
  assert.deepStrictEqual(h.commands(), ['open_file_chooser', 'cancel_file_chooser']);

  h.releaseOpen();            // the window finally exists
  await settle();

  assert.deepStrictEqual(h.commands(),
    ['open_file_chooser', 'cancel_file_chooser', 'cancel_file_chooser'],
    'the cancel goes again now that there IS a registry entry to force-resolve');
  assert.strictEqual(h.scrimUp(), false);
});

await checkAsync('that late cancel never reaches the NEXT session\'s chooser', async () => {
  // The dead session's re-send carries ITS OWN reqId (myReqId, from its own
  // open_file_chooser). By the time it lands, the parent has already opened a
  // second chooser: Rust's open_file_chooser displaces a live entry by
  // cancel-and-recreate rather than reusing it, so the successor is minted a
  // FRESH req_id and a fresh, request-unique window label
  // (chooser_window.rs's `open_file_chooser`/`ChooserRegistry::open`). The
  // dead session's cancel names the old id, which the registry no longer has
  // live, so scoping by reqId resolves nothing — an unscoped force-resolve
  // would instead answer the successor's question with a silent null: a ⌘O
  // that does nothing at all.
  const h = makeHarness({ gateOpen: true });
  const pane = { kind: 'editor', tabId: 1 };
  const saving = h.fd._chooseFile({ mode: 'save', filename: 'Untitled', pane });
  await settle();
  h.fd.cancelForPane(pane);
  assert.strictEqual(await settles(saving, 'the chooser cancelled mid-build'), null);
  await settle();

  // ⌘O, immediately — before the dead session's build has returned.
  const opening = h.fd.openForOpen();
  await settle();
  const before = h.commands().slice();
  assert.ok(before.includes('open_file_chooser'), 'precondition: a new session is up');
  assert.strictEqual(h.scrimUp(), true, 'precondition: on its own scrim');

  h.releaseOpen();            // the DEAD session's build returns (FIFO)
  await settle();

  assert.deepStrictEqual(h.commands(), before,
    'the dead session sends nothing once a live one has claimed the chooser');
  assert.strictEqual(h.scrimUp(), true, 'the new session is untouched');

  h.releaseOpen();            // and the new session's own build
  await settle();
  h.resolve(null, 2);
  assert.strictEqual(await settles(opening, 'the new session'), null);
});

await checkAsync('a cancel names its OWN chooser, so a late one cannot kill the next', async () => {
  // `cancel_file_chooser` used to name no chooser at all — it force-resolved
  // whatever was live for the calling window. Two IPC calls from one window
  // have no ordering guarantee, so a cancel dispatched for chooser A could
  // arrive after A was already answered and the parent had opened chooser B,
  // and resolve B null instead: a ⌘O that silently does nothing.
  //
  // The transport below enforces the registry's rule (chooser_window.rs's
  // `ChooserRegistry::cancel`): a reqId that does not name the live entry
  // resolves nothing.
  const h = makeHarness({});
  const pane = { kind: 'editor', tabId: 1 };
  const savingA = h.fd._chooseFile({ mode: 'save', filename: 'Untitled', pane });
  await settle();
  assert.strictEqual(h.liveReqId(), 1, 'precondition: chooser 1 is live');

  assert.strictEqual(h.fd.cancelForPane(pane), true);
  assert.strictEqual(await settles(savingA, 'the cancelled chooser'), null);
  await settle();
  assert.deepEqual(h.argsFor('cancel_file_chooser'), { reqId: 1 },
    'the proxy names the chooser it opened, not "whatever is live"');

  // ⌘O: a chooser of its own.
  const opening = h.fd.openForOpen();
  await settle();
  assert.strictEqual(h.liveReqId(), 2, 'precondition: a DIFFERENT chooser is live now');
  assert.strictEqual(h.scrimUp(), true);

  // A's cancel finally lands at Rust, long after A was answered.
  await h.wire('cancel_file_chooser', { reqId: 1 });
  await settle();
  assert.strictEqual(h.liveReqId(), 2, 'the stale cancel resolved nothing');
  assert.strictEqual(h.scrimUp(), true, 'and the live chooser is untouched');

  // While the one that names it really does close it.
  await h.wire('cancel_file_chooser', { reqId: 2 });
  assert.strictEqual(await settles(opening, 'the live chooser'), null);
  await settle();
  assert.strictEqual(h.liveReqId(), null);
  assert.strictEqual(h.scrimUp(), false);
});

await checkAsync('an open against a live chooser REPLACES it instead of adopting it', async () => {
  // Rust used to hand a second open the live entry's req_id and focus the
  // existing window. That is an adoption: the caller ends up bound to a window
  // built for a DIFFERENT question. A save-mode request adopting an open-mode
  // window gets a chooser with no filename field and no overwrite confirm — and
  // `openForSave` then writes the pick straight over whatever is there.
  //
  // The request below is delivered at the transport, not through the proxy, on
  // purpose: `activeChoice` shares or refuses every legitimate duplicate, so an
  // open reaching Rust while an entry is live is always an abnormal flow (the
  // cancel/open IPC race, or a reloaded webview with a zombie chooser).
  const h = makeHarness({});
  const pane = { kind: 'editor', tabId: 1 };
  const savingA = h.fd._chooseFile({ mode: 'save', filename: 'Untitled', pane });
  await settle();
  assert.strictEqual(h.liveReqId(), 1, 'precondition: chooser 1 is live');

  const replacement = await h.wire('open_file_chooser', {
    mode: 'open', filename: null, selectFilename: false,
  });
  await settle();

  assert.notStrictEqual(replacement, 1,
    'the second request gets a chooser of its OWN — never the live id reused');
  assert.strictEqual(h.liveReqId(), replacement, 'and it is the one now registered');
  assert.strictEqual(await settles(savingA, 'the displaced chooser'), null,
    'the session it displaced was resolved as cancelled, not orphaned');
  await settle();
  assert.strictEqual(h.scrimUp(), false, 'so the displaced session let its parent go');

  // And a cancel still naming the displaced chooser is a no-op: that id no
  // longer refers to anything.
  await h.wire('cancel_file_chooser', { reqId: 1 });
  await settle();
  assert.strictEqual(h.liveReqId(), replacement, 'the replacement survives it');
});

await checkAsync('a stale early event cannot crowd out this session\'s own answer', async () => {
  // The buffer holds every event that arrives before the reqId is known, not
  // just the first. A previous session's late cancel echoing back can land in
  // that window; if it took the only slot, the real answer behind it would be
  // dropped and this session would wedge — scrim up, promise unsettled.
  const h = makeHarness({ gateOpen: true });
  const choice = h.fd._chooseFile();
  await settle();

  h.resolve({ scope: { kind: 'local' }, path: '/stale', entry: null }, 99);   // not ours
  h.resolve({ scope: { kind: 'local' }, path: '/ours', entry: null }, 1);     // ours
  await settle();
  assert.strictEqual(h.scrimUp(), true, 'precondition: neither has been matched yet');

  h.releaseOpen();
  const value = await settles(choice, 'a chooser whose answer queued behind a stale event');
  assert.strictEqual(value.path, '/ours');
  await settle();
  assert.strictEqual(h.scrimUp(), false);
});

// ---------------------------------------------------------------------------
// 7. Rehydration: the choice the event carries drives the real open/save paths
// ---------------------------------------------------------------------------

console.log('file dialog proxy: the choice crosses the boundary intact');

await checkAsync('openForOpen routes a remote choice to openRemoteFile with the CLEAN hostLabel', async () => {
  const h = makeHarness({});
  const done = h.fd.openForOpen();
  await settle();
  h.resolve(REMOTE_CHOICE);
  await settles(done, 'the open');
  await settle();

  // deepEqual, not deepStrictEqual: the descriptor is built inside the vm
  // context, so its prototype is the sandbox's Object — same pattern as
  // test_editor_save_as.mjs's saveAs assertions.
  assert.deepEqual(h.openCalls, [['openRemoteFile', {
    paneId: 4,
    remotePath: '/home/me/notes.txt',
    hostLabel: 'me@example.com',
    size: 120,
  }]], 'the descriptor test_editor_remote_transfer.mjs asserts downstream');
  assert.ok(!/pane/.test(h.openCalls[0][1].hostLabel),
    'the display suffix never reaches editor_temp_path');
});

await checkAsync('openForOpen routes a local choice to openLocalFile(path)', async () => {
  const h = makeHarness({});
  const done = h.fd.openForOpen();
  await settle();
  h.resolve({ scope: { id: 'local', kind: 'local', label: 'This Mac', start: '/home/u' }, path: '/home/u/a.txt', entry: null });
  await settles(done, 'the open');
  await settle();
  assert.deepStrictEqual(h.openCalls, [['openLocalFile', '/home/u/a.txt']]);
});

await checkAsync('openForSave routes a remote choice to saveAs with paneId + hostLabel', async () => {
  const h = makeHarness({});
  const pane = { kind: 'editor', tabId: 1, filePath: '/tmp/x/scratch.txt', remote: null };
  const done = h.fd.openForSave(pane);
  await settle();
  assert.deepEqual(h.argsFor('open_file_chooser'), {
    mode: 'save', filename: 'scratch.txt', selectFilename: false,
  }, 'the question is asked over the wire, in the shape Rust deserialises');

  h.resolve(REMOTE_CHOICE);
  await settles(done, 'the save');
  await settle();
  assert.deepEqual(h.openCalls, [['saveAs', {
    scope: 'remote',
    paneId: 4,
    hostLabel: 'me@example.com',
    remotePath: '/home/me/notes.txt',
  }]]);
});

await checkAsync('a cancelled openForSave saves nothing', async () => {
  const h = makeHarness({});
  const pane = { kind: 'editor', tabId: 1, filePath: null, remote: null, untitledSeq: 2 };
  h.sandbox.termlabEditorTabLabel = { editorTabLabel: () => ({ label: 'Untitled-2' }) };
  const done = h.fd.openForSave(pane);
  await settle();
  assert.deepEqual(h.argsFor('open_file_chooser'), {
    mode: 'save', filename: 'Untitled-2', selectFilename: true,
  }, 'an untitled buffer sends its tab label, selected to type over');

  h.resolve(null);
  assert.strictEqual(await settles(done, 'the cancelled save'), null);
  await settle();
  assert.deepStrictEqual(h.openCalls, []);
});

// ---------------------------------------------------------------------------
// 8. What the proxy must NOT do
// ---------------------------------------------------------------------------

check('the proxy never asks for its own window label', () => {
  // Rust derives the parent from the CALLING window. A blocking label fetch
  // before painting the scrim is both redundant and a window in which a
  // cancel can strand the session (it did, in the bridge this replaces).
  const src = fs.readFileSync(path.join(APP, 'features/editor/file-dialog.js'), 'utf8');
  assert.ok(!src.includes('getCurrentWindowLabel'),
    'the calling window IS the parent — never fetch a label to say so');
  assert.ok(!src.includes('tlDialog'),
    'the tl-dialog bridge is gone; the chooser has a window of its own');
  assert.ok(!/tl-filedlg__/.test(src),
    'and the chooser\'s DOM belongs to file-dialog-view.js alone');
});

check('the scrim sits BELOW tl-dialog\'s band, not above it', () => {
  // Same idiom as the source greps around it: no styles are computed in this
  // harness, so the rule is read from the stylesheet it ships in.
  //
  // The trap this guards: the parent can still raise a dialog of its own while
  // a chooser is open (⌘W's Unsaved Changes, the Settings accelerator), and
  // those overlays are `document.body` children — `inert` on #app does not
  // reach them. A scrim above the dialog band leaves such a prompt
  // interactive but invisible, which is the locked app the scrim's own
  // `finally` exists to prevent. Below the band it renders over the scrim and
  // stays answerable.
  const css = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/styles/design-system/components/file-dialog.css'),
    'utf8',
  );
  const rule = /\.tl-chooser-scrim\s*\{([^}]*)\}/.exec(css);
  assert.ok(rule, 'the scrim class file-dialog.js applies has a rule of its own');
  const zIndex = /z-index:\s*(\d+)/.exec(rule[1]);
  assert.ok(zIndex, 'and that rule sets a z-index');
  const value = Number(zIndex[1]);
  // 3000 is tl-dialog's base, +10 per stacked depth (test_tl_dialog.mjs:66-68).
  assert.ok(value < 3000,
    `the scrim must stay under tl-dialog's 3000 band, got ${value}`);
  assert.ok(value > 1000,
    `and over the app's own surfaces, got ${value}`);
  assert.match(rule[1], /background:\s*var\(--tl-dialog-scrim\)/,
    'tokens only — the same scrim colour the modal dialogs use');
});

check('the proxy registers nothing with the close guard', () => {
  // close_guard::on_close_requested already fires for chooser windows; the
  // Rust registry owns the chooser window's close semantics.
  const src = fs.readFileSync(path.join(APP, 'features/editor/file-dialog.js'), 'utf8');
  assert.ok(!/closeGuard|close_guard|onCloseRequested/.test(src));
});

// ---------------------------------------------------------------------------

if (failures) {
  console.error(`file dialog proxy: ${failures} of ${ran} check(s) FAILED`);
  process.exit(1);
}
console.log(`file dialog proxy: all ${ran} checks passed`);
