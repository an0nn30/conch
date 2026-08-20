// Run: node scripts/tests/test_sftp_connect.mjs
//
// SFTP independent host connections, Task 3: the remote pane's host dropdown
// and the pinning path (frontend/app/panels/files-panel.js,
// frontend/app/features/files/pane-view.js).
//
// Two harnesses, both loading the REAL production modules (no
// reimplementation), matching the vm-harness idiom test_panel_host.mjs's
// scenario 33 established for this file family:
//
//   "Logic" harness (scenarios 1, 3-10) — loads the real files-panel.js,
//   features/files/data-service.js and features/files/pane-store.js, but
//   stubs termlabFilesPaneView.renderPane as a spy that just RECORDS
//   (pane, el, deps) instead of touching a DOM. Since deps carries the
//   actual closures files-panel.js wires up (onHostComboChange, onDisconnect,
//   onComboMount, ...), the tests can drive the combo/pin/disconnect/refresh
//   logic by calling those captured callbacks directly — no HTML parser
//   needed (this repo has none; see test_tl_combo.mjs's header).
//
//   "DOM" harness (scenario 2 only) — loads the real pane-view.js too, to
//   prove the re-attach trap (pane-view.js:28's innerHTML rebuild) is
//   actually handled: a minimal fake `document`/element good enough for
//   pane-view.js's host-combo block (built with the DOM API, not template
//   interpolation, for exactly this reason) to run without touching the
//   parts of renderPane this test doesn't exercise (the row table, the
//   static toolbar buttons — still template-interpolated and thus opaque to
//   a stub with no parser, but never queried by the combo code path).
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const FRONTEND = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app',
);
const FILES_PANEL_PATH = path.join(FRONTEND, 'panels/files-panel.js');
const FILES_DATA_SERVICE_PATH = path.join(FRONTEND, 'features/files/data-service.js');
const FILES_PANE_STORE_PATH = path.join(FRONTEND, 'features/files/pane-store.js');
const FILES_ACTIONS_PATH = path.join(FRONTEND, 'features/files/actions.js');
const PANE_VIEW_PATH = path.join(FRONTEND, 'features/files/pane-view.js');

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
// Several scenarios chain more than one await inside files-panel.js (e.g.
// connectToHost awaits connectHost() THEN pinRemotePane() THEN
// getRemoteRealPath() THEN loadEntries()) — one microtask tick is not
// always enough to drain that whole chain.
const settle = async (times = 4) => { for (let i = 0; i < times; i += 1) await tick(); };

// --- shared element stub (basic — panelEl/local-pane/remote-pane roots in
// the logic harness; querySelector is overridden per test to hand back
// fixed #fp-local/#fp-remote stand-ins, exactly as test_panel_host.mjs
// scenario 33 does) ---------------------------------------------------------
function makeElement(tag) {
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    dataset: {},
    className: '',
    title: '',
    textContent: '',
    parentNode: null,
    classList: {
      _set: new Set(),
      add(...c) { c.forEach((x) => this._set.add(x)); },
      remove(...c) { c.forEach((x) => this._set.delete(x)); },
      contains(c) { return this._set.has(c); },
    },
    appendChild(child) {
      this.children.push(child);
      if (child && typeof child === 'object') child.parentNode = this;
      return child;
    },
    removeChild(child) {
      const i = this.children.indexOf(child);
      if (i >= 0) this.children.splice(i, 1);
      if (child && typeof child === 'object') child.parentNode = null;
      return child;
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    removeEventListener() {},
    dispatchEvent(evt) {
      for (const fn of (listeners.get(evt.type) || []).slice()) fn(evt);
      return true;
    },
    setAttribute(n, v) { attrs.set(n, String(v)); },
    getAttribute(n) { return attrs.has(n) ? attrs.get(n) : null; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }; },
  };
  return el;
}

// --- fixtures ----------------------------------------------------------

// A folder entry ('e-build') and an ssh_config entry sharing THE SAME id —
// the shape vault-linking produces once it promotes an ssh-config host into
// a config-owned copy (T2 review finding F5). The config-owned copy (in
// `folders`) must win; 'e-build (ssh_config duplicate)' must never appear.
function makeServersFixture() {
  return {
    folders: [
      {
        id: 'f1',
        name: 'Work',
        expanded: true,
        entries: [
          { id: 'e-build', label: 'build-box', host: 'build.example.com', port: 22 },
        ],
      },
    ],
    ungrouped: [
      { id: 'e-personal', label: 'personal-vps', host: 'vps.example.com', port: 22 },
    ],
    ssh_config: [
      { id: 'e-build', label: 'build-box (ssh_config duplicate)', host: 'build.example.com', port: 22 },
      { id: 'e-raw', label: 'raw-ssh-host', host: 'raw.example.com', port: 22 },
    ],
  };
}

function makeSessionsFixture() {
  return [
    // Tab-owned: an ordinary spawned SSH pane also happens to be a live
    // "session" (remote_get_sessions makes no terminal/detached distinction
    // — see server_commands.rs's active_sessions comment).
    { key: 'main:7', host: '10.0.0.5', user: 'alice', port: 22 },
    // Detached (panel-only): pane-id tail >= DETACHED_PANE_ID_BASE
    // (1,000,000) — see detached_commands.rs.
    { key: 'main:1000007', host: 'detached.example.com', user: 'bob', port: 2222 },
  ];
}

// --- logic-harness factory --------------------------------------------

// Builds a fresh vm context, loads the real pane-store/data-service/actions/
// panel modules into it, stubs termlabFilesPaneView.renderPane as a
// recording spy, and runs filesPanel.init(). `invokeExtra(cmd, args)` may
// return a Promise (or undefined to fall through to the default fixture
// responses) so each scenario can override exactly the calls it cares
// about. `opts.getActiveTab`, if given, replaces the default `() => null`
// (used by the M1 cwd-poll scenario, which needs a focused ssh tab).
async function setupLogicHarness(invokeExtra, opts = {}) {
  const invokeCalls = [];
  const renderCalls = []; // { pane, el, deps }
  const listeners = {}; // eventName -> handler, from opts.listen registrations

  let serversFixture = makeServersFixture();
  let sessionsFixture = makeSessionsFixture();

  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    const extra = invokeExtra ? invokeExtra(cmd, args) : undefined;
    if (extra !== undefined) return extra;
    if (cmd === 'get_home_dir') return Promise.resolve('/home/demo');
    if (cmd === 'get_all_settings') return Promise.resolve({});
    if (cmd === 'local_list_dir') return Promise.resolve([]);
    if (cmd === 'remote_get_servers') return Promise.resolve(serversFixture);
    if (cmd === 'remote_get_sessions') return Promise.resolve(sessionsFixture);
    if (cmd === 'sftp_realpath') return Promise.resolve('/home/pinned');
    if (cmd === 'sftp_list_dir') return Promise.resolve([]);
    return Promise.resolve(undefined);
  };

  const sandbox = {
    console,
    setTimeout,
    clearTimeout,
    setInterval: () => 0, // cwd-polling timers are irrelevant here; see scenario 33's identical stub
    clearInterval: () => {},
    Promise,
    Math,
    Array,
    JSON,
    Object,
    String,
    Number,
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  sandbox.utils = { formatSize: () => '', formatDate: () => '', esc: (v) => String(v == null ? '' : v), attr: (v) => String(v == null ? '' : v) };
  sandbox.toast = { error() {}, info() {}, warn() {}, success() {} };
  sandbox.toolWindowManager = { isVisible: () => true, activate() {}, deactivate() {} };
  sandbox.tlCombo = { attach: () => ({ button: makeElement('button'), refresh() {} }) };
  sandbox.termlabFilesPaneView = {
    renderPane: (pane, el, deps) => { renderCalls.push({ pane, el, deps }); },
    showColumnMenu: () => {},
    showRowContextMenu: () => {},
  };
  sandbox.termlabFilesTransfers = {};

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(FILES_PANE_STORE_PATH, 'utf8'), sandbox, { filename: FILES_PANE_STORE_PATH });
  vm.runInContext(fs.readFileSync(FILES_DATA_SERVICE_PATH, 'utf8'), sandbox, { filename: FILES_DATA_SERVICE_PATH });
  // Real actions.js (navigate/goBack/...), not a stub: the M1 poll-vs-pin
  // scenario needs a genuine navigate() that mutates pane.currentPath and
  // drives loadEntries, so it can tell "the poll navigated the pinned pane"
  // apart from "navigate is a no-op in this harness".
  vm.runInContext(fs.readFileSync(FILES_ACTIONS_PATH, 'utf8'), sandbox, { filename: FILES_ACTIONS_PATH });
  vm.runInContext(fs.readFileSync(FILES_PANEL_PATH, 'utf8'), sandbox, { filename: FILES_PANEL_PATH });

  const panelEl = makeElement('div');
  const localRootEl = makeElement('div');
  const remoteRootEl = makeElement('div');
  panelEl.querySelector = (sel) => {
    if (sel === '#fp-local') return localRootEl;
    if (sel === '#fp-remote') return remoteRootEl;
    return null;
  };

  sandbox.filesPanel.init({
    invoke,
    panelEl,
    panelWrapEl: makeElement('div'),
    resizeHandleEl: makeElement('div'),
    layoutService: null,
    fitActiveTab: () => {},
    getActiveTab: opts.getActiveTab || (() => null),
    listen: (name, handler) => { listeners[name] = handler; },
  });
  await settle();
  invokeCalls.length = 0; // discard boot traffic (getHomeDir/settings/local_list_dir/servers/sessions)
  renderCalls.length = 0;

  return {
    sandbox,
    invoke,
    invokeCalls,
    renderCalls,
    listeners,
    setServersFixture: (v) => { serversFixture = v; },
    setSessionsFixture: (v) => { sessionsFixture = v; },
  };
}

// Latest remote-pane renderPane call, or throws if none happened yet.
function lastRemoteCall(renderCalls) {
  for (let i = renderCalls.length - 1; i >= 0; i -= 1) {
    if (renderCalls[i].pane.prefix === 'remote') return renderCalls[i];
  }
  throw new Error('no remote-pane renderPane call recorded yet');
}

// --- 1. Combo composition: follow + sessions + separator + deduped, ------
// folder-prefixed configured hosts. ------------------------------------
{
  const h = await setupLogicHarness();
  // init() already ran refreshHostCombo() once (part of the discarded boot
  // traffic); force a fresh render against the discarded invoke count so
  // there is something to inspect.
  await h.listeners['remote-sessions-changed']();
  await settle();

  const { deps } = lastRemoteCall(h.renderCalls);
  const values = deps.hostOptions.map((o) => o.value);
  const labels = deps.hostOptions.map((o) => o.label);

  assert.deepEqual(values, [
    '', 'main:7', 'main:1000007', '__separator__', 'e-build', 'e-personal', 'e-raw',
  ], 'combo order must be follow -> sessions -> separator -> configured hosts');
  assert.equal(labels[0], 'Follow active tab');
  assert.equal(labels[1], 'alice@10.0.0.5', 'session label goes through sessionHostLabel');
  assert.equal(labels[2], 'bob@detached.example.com:2222', 'non-22 port is included in the session label');
  assert.equal(deps.hostOptions[3].disabled, true, 'the separator option is disabled/unselectable');
  assert.equal(labels[4], 'Work / build-box', 'folder entries are prefixed "Folder / label"');
  assert.equal(labels[5], 'personal-vps', 'ungrouped entries are unprefixed');
  assert.equal(labels[6], 'raw-ssh-host', 'a genuine ssh_config-only entry still appears');
  assert.equal(
    deps.hostOptions.filter((o) => o.value === 'e-build').length, 1,
    'a promoted duplicate (same id in both folders and ssh_config) must appear exactly once, config copy preferred',
  );
  console.log('1. combo composition: ok');
}

// --- 2. Re-attach after a forced re-render: pin, force a second render, ---
// combo must still get attach()ed and the new <select>'s value preserved. -
{
  const attachCalls = [];

  const domSandbox = {
    console,
    setTimeout,
    clearTimeout,
    setInterval: () => 0,
    clearInterval: () => {},
    Promise,
    Math,
    Array,
    JSON,
    Object,
    String,
    Number,
  };
  domSandbox.window = domSandbox;
  domSandbox.global = domSandbox;
  domSandbox.utils = { formatSize: () => '', formatDate: () => '', esc: (v) => String(v == null ? '' : v), attr: (v) => String(v == null ? '' : v) };
  domSandbox.toast = { error() {}, info() {}, warn() {}, success() {} };
  domSandbox.toolWindowManager = { isVisible: () => true, activate() {}, deactivate() {} };
  domSandbox.termlabFilesActions = {};
  domSandbox.termlabFilesTransfers = {};
  // window.tlCombo — a spy standing in for the real app/ui/tl-combo.js
  // (that module is exercised for real by test_tl_combo.mjs; this test's
  // job is only to prove files-panel.js/pane-view.js call attach() again on
  // every rebuild, not to re-verify tl-combo's own internals).
  domSandbox.tlCombo = { attach: (selectEl) => { attachCalls.push(selectEl); return { button: makeElement('button'), refresh() {} }; } };

  // A minimal `document` — just enough for pane-view.js's DOM-built host
  // combo block to run. The row table and the static (template-interpolated)
  // toolbar buttons are never queried by that code path, so they need no
  // stand-in here; pane.entries stays empty for this test.
  function makeFakeElement(tag) {
    const t = String(tag || 'div').toUpperCase();
    const listeners = new Map();
    const el = {
      tagName: t,
      children: [],
      className: '',
      textContent: '',
      disabled: false,
      title: '',
      type: '',
      appendChild(child) { this.children.push(child); return child; },
      addEventListener(name, fn) {
        if (!listeners.has(name)) listeners.set(name, []);
        listeners.get(name).push(fn);
      },
      dispatchEvent(evt) {
        for (const fn of (listeners.get(evt.type) || []).slice()) fn(evt);
        return true;
      },
    };
    if (t === 'SELECT') {
      let currentValue = '';
      Object.defineProperty(el, 'options', {
        get() { return el.children.filter((c) => c.tagName === 'OPTION'); },
      });
      Object.defineProperty(el, 'value', {
        get() { return currentValue; },
        set(v) { currentValue = v; },
      });
    }
    return el;
  }
  const remoteHostComboSlot = makeFakeElement('span');
  domSandbox.document = { createElement: (tag) => makeFakeElement(tag) };

  vm.createContext(domSandbox);
  vm.runInContext(fs.readFileSync(FILES_PANE_STORE_PATH, 'utf8'), domSandbox, { filename: FILES_PANE_STORE_PATH });
  vm.runInContext(fs.readFileSync(FILES_DATA_SERVICE_PATH, 'utf8'), domSandbox, { filename: FILES_DATA_SERVICE_PATH });
  vm.runInContext(fs.readFileSync(PANE_VIEW_PATH, 'utf8'), domSandbox, { filename: PANE_VIEW_PATH });
  vm.runInContext(fs.readFileSync(FILES_PANEL_PATH, 'utf8'), domSandbox, { filename: FILES_PANEL_PATH });

  const invokeCalls = [];
  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    if (cmd === 'get_home_dir') return Promise.resolve('/home/demo');
    if (cmd === 'get_all_settings') return Promise.resolve({});
    if (cmd === 'local_list_dir') return Promise.resolve([]);
    if (cmd === 'remote_get_servers') return Promise.resolve(makeServersFixture());
    if (cmd === 'remote_get_sessions') return Promise.resolve(makeSessionsFixture());
    if (cmd === 'sftp_realpath') return Promise.resolve('/home/pinned');
    if (cmd === 'sftp_list_dir') return Promise.resolve([]);
    return Promise.resolve(undefined);
  };

  const panelEl = makeElement('div');
  const localRootEl = makeElement('div');
  // The remote pane root: innerHTML is a plain (unparsed) property — same
  // no-op semantics as the shared local-harness elements — and
  // querySelector always hands back the persistent combo slot regardless of
  // the template string just assigned, mirroring how the string always
  // contains .fp-host-combo-slot for the remote pane (pane-view.js:37).
  const remoteRootEl = makeElement('div');
  remoteRootEl.querySelector = (sel) => (sel === '.fp-host-combo-slot' ? remoteHostComboSlot : null);
  remoteRootEl.querySelectorAll = () => [];

  panelEl.querySelector = (sel) => {
    if (sel === '#fp-local') return localRootEl;
    if (sel === '#fp-remote') return remoteRootEl;
    return null;
  };

  domSandbox.filesPanel.init({
    invoke,
    panelEl,
    panelWrapEl: makeElement('div'),
    resizeHandleEl: makeElement('div'),
    layoutService: null,
    fitActiveTab: () => {},
    getActiveTab: () => null,
    listen: () => {},
  });
  await settle();
  attachCalls.length = 0;

  await domSandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  assert.ok(attachCalls.length >= 1, 'pinning must render the remote pane and attach the combo');
  const firstSelect = attachCalls[attachCalls.length - 1];
  assert.equal(firstSelect.value, 'main:1000007', 'the freshly attached select must show the pinned session as selected');

  // Force a second, independent re-render — calling pinRemotePane again is a
  // legitimate real trigger (Task 4's dialogs may re-pin), and exercises
  // exactly the innerHTML-rebuild trap: pane-view.js:28 wipes whatever
  // select the FIRST render attached, so a naive one-time attach() would
  // leave the SECOND render's combo never wired to tl-combo at all.
  await domSandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  assert.ok(attachCalls.length >= 2, 'attach() must run again after the forced re-render, not just once');
  const secondSelect = attachCalls[attachCalls.length - 1];
  assert.notEqual(secondSelect, firstSelect, 'each render mounts a genuinely new <select> (innerHTML wiped the old one)');
  assert.equal(secondSelect.value, 'main:1000007', 'the pinned value survives the rebuild');

  console.log('2. re-attach after forced re-render: ok');
}

// --- 3. Pick-session pins directly — no sftp_connect_host invoke. ---------
{
  const h = await setupLogicHarness();
  // Force a render so there is a deps object to read the callback off, since
  // the boot-traffic renders were discarded by setupLogicHarness.
  await h.listeners['remote-sessions-changed']();
  await settle();
  const picked = lastRemoteCall(h.renderCalls).deps;

  await picked.onHostComboChange('main:7'); // a live "session" option
  await settle();

  assert.ok(
    !h.invokeCalls.some((c) => c.cmd === 'sftp_connect_host'),
    'picking an already-live session must not invoke sftp_connect_host',
  );
  const after = lastRemoteCall(h.renderCalls).deps;
  assert.equal(after.hostComboValue, 'main:7', 'the picked session becomes the pinned/selected value');
  console.log('3. pick-session pins without connect: ok');
}

// --- 4. Pick-host invokes sftp_connect_host with the entry id. -----------
{
  const h = await setupLogicHarness((cmd) => {
    if (cmd === 'sftp_connect_host') {
      return Promise.resolve({ sessionKey: 'main:1000099', host: 'build.example.com', user: 'ci', port: 22, paneId: 1000099 });
    }
    return undefined;
  });
  await h.listeners['remote-sessions-changed']();
  await settle();
  const deps = lastRemoteCall(h.renderCalls).deps;

  await deps.onHostComboChange('e-build'); // a configured-host option, not a live session
  await settle();

  const connectCall = h.invokeCalls.find((c) => c.cmd === 'sftp_connect_host');
  assert.ok(connectCall, 'picking a configured host must invoke sftp_connect_host');
  assert.deepEqual(connectCall.args, { serverEntryId: 'e-build' });

  const after = lastRemoteCall(h.renderCalls).deps;
  assert.equal(after.hostComboValue, 'main:1000099', 'a successful connect pins to the returned session key');
  assert.equal(after.hostComboBusy, false);
  console.log('4. pick-host invokes connect with the entry id: ok');
}

// --- 5. Busy state renders during flight, clears on resolution. ----------
{
  let resolveConnect;
  const pending = new Promise((resolve) => { resolveConnect = resolve; });
  const h = await setupLogicHarness((cmd) => {
    if (cmd === 'sftp_connect_host') return pending;
    return undefined;
  });
  await h.listeners['remote-sessions-changed']();
  await settle();
  const deps = lastRemoteCall(h.renderCalls).deps;

  const picked = deps.onHostComboChange('e-personal'); // not awaited yet — inspect mid-flight
  await settle(1);

  const midFlight = lastRemoteCall(h.renderCalls).deps;
  assert.equal(midFlight.hostComboBusy, true, 'the combo must render busy while the connect is in flight');
  assert.equal(midFlight.hostComboValue, 'e-personal', 'the picked host stays shown (disabled) while busy');

  resolveConnect({ sessionKey: 'main:1000055', host: 'vps.example.com', user: 'me', port: 22, paneId: 1000055 });
  await picked;
  await settle();

  const after = lastRemoteCall(h.renderCalls).deps;
  assert.equal(after.hostComboBusy, false, 'busy clears once the connect resolves');
  assert.equal(after.hostComboValue, 'main:1000055');
  console.log('5. busy state during flight: ok');
}

// --- 6. connectInProgress is retry-after-current, never an error. --------
{
  const h = await setupLogicHarness((cmd) => {
    if (cmd === 'sftp_connect_host') return Promise.reject({ kind: 'connectInProgress' });
    return undefined;
  });
  await h.listeners['remote-sessions-changed']();
  await settle();
  const deps = lastRemoteCall(h.renderCalls).deps;

  await deps.onHostComboChange('e-personal');
  await settle();

  const after = lastRemoteCall(h.renderCalls);
  assert.equal(after.deps.hostComboBusy, true, 'connectInProgress must leave the busy state persisting, not clear it');
  assert.equal(after.pane.error, null, 'connectInProgress must never surface as an .fp-error');
  console.log('6. connectInProgress: busy persists, no error: ok');
}

// --- 7. Pinned suppresses onTabChanged rebinding. -------------------------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;

  // Drive the delegate directly with a pane payload — a live SSH tab just
  // got focused/spawned, exactly the shape onTabChanged normally rebinds to.
  await h.sandbox.filesPanel.onTabChanged({ type: 'ssh', spawned: true, paneId: 42 });
  await settle();

  assert.ok(
    !h.invokeCalls.some((c) => c.cmd === 'sftp_realpath' && c.args && c.args.paneId === 42),
    'onTabChanged must not rebind the remote pane to the newly-focused tab while pinned',
  );
  const deps = lastRemoteCall(h.renderCalls).deps;
  assert.equal(deps.hostComboValue, 'main:1000007', 'the pin must still hold after the suppressed tab event');
  console.log('7. pinned suppresses onTabChanged rebinding: ok');
}

// --- 8. Unpin restores follow behavior on the next tab event. -------------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();

  await h.sandbox.filesPanel.pinRemotePane(null);
  await settle();
  h.invokeCalls.length = 0;

  await h.sandbox.filesPanel.onTabChanged({ type: 'ssh', spawned: true, paneId: 42 });
  await settle();

  const realpathCall = h.invokeCalls.find((c) => c.cmd === 'sftp_realpath');
  assert.ok(realpathCall, 'after unpinning, the next tab event must rebind the remote pane again');
  assert.equal(realpathCall.args.paneId, 42);
  console.log('8. unpin restores follow behavior: ok');
}

// --- 9. Disconnect: only offered on a pinned DETACHED session; tears down -
// via sftp_disconnect and returns to follow mode. -------------------------
{
  const h = await setupLogicHarness();

  // Pinned to a TAB-owned session (pane-id tail 7 < DETACHED_PANE_ID_BASE):
  // no disconnect affordance — it dies with its tab, not this panel.
  await h.sandbox.filesPanel.pinRemotePane('main:7');
  await settle();
  assert.equal(lastRemoteCall(h.renderCalls).deps.showDisconnect, false, 'a tab-owned pinned session must not offer disconnect');

  // Pinned to a DETACHED session (pane-id tail 1,000,007 >= base): eject is offered.
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  const deps = lastRemoteCall(h.renderCalls).deps;
  assert.equal(deps.showDisconnect, true, 'a pinned detached session must offer disconnect');

  h.invokeCalls.length = 0;
  await deps.onDisconnect();
  await settle();

  const disconnectCall = h.invokeCalls.find((c) => c.cmd === 'sftp_disconnect');
  assert.ok(disconnectCall, 'the eject action must invoke sftp_disconnect');
  assert.deepEqual(disconnectCall.args, { sessionKey: 'main:1000007' });

  const after = lastRemoteCall(h.renderCalls).deps;
  assert.equal(after.hostComboValue, '', 'disconnecting must return the combo to follow mode');
  console.log('9. disconnect flow: ok');
}

// --- 10. remote-sessions-changed rebuilds options and drops a vanished ----
// pin back to follow mode with an error note. ------------------------------
{
  const h = await setupLogicHarness();
  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  assert.equal(lastRemoteCall(h.renderCalls).deps.hostComboValue, 'main:1000007');

  // The pinned session is gone from the next remote_get_sessions response —
  // e.g. the host disconnected out from under the pin, or another window
  // tore it down.
  h.setSessionsFixture(makeSessionsFixture().filter((s) => s.key !== 'main:1000007'));
  await h.listeners['remote-sessions-changed']();
  await settle();

  const after = lastRemoteCall(h.renderCalls);
  assert.equal(after.deps.hostComboValue, '', 'a vanished pin must drop back to follow mode');
  assert.ok(after.pane.error, 'a vanished pin must leave an error note in the remote pane');
  assert.ok(
    !after.deps.hostOptions.some((o) => o.value === 'main:1000007'),
    'the vanished session must no longer appear in the combo options',
  );
  console.log('10. remote-sessions-changed drops a vanished pin: ok');
}

// --- 11 (M1). The remote cwd-follow poll must not navigate a PINNED pane --
// off the ACTIVE TAB's cwd — that would yank a pane pinned to host A onto
// host B's cwd interpreted as a path on host A (wrong listing / error
// loop). Unpinning must restore the poll's normal navigate-on-cwd-change
// behavior on the very next tick. ------------------------------------------
{
  const h = await setupLogicHarness(
    (cmd) => {
      if (cmd === 'ssh_get_pane_cwd') return Promise.resolve('/tab/on/host-b');
      return undefined;
    },
    { getActiveTab: () => ({ type: 'ssh', spawned: true, paneId: 42 }) },
  );

  await h.sandbox.filesPanel.pinRemotePane('main:1000007');
  await settle();
  h.invokeCalls.length = 0;

  // Simulate a poll tick while pinned: the focused TAB (paneId 42) is a
  // different session than the pin (main:1000007).
  await h.sandbox.filesPanel.pollActiveRemotePaneCwd(42);
  await settle();

  assert.ok(
    !h.invokeCalls.some((c) => c.cmd === 'ssh_get_pane_cwd'),
    'the poll must not even query the focused tab cwd while the remote pane is pinned',
  );
  assert.ok(
    !h.invokeCalls.some((c) => c.cmd === 'sftp_list_dir'),
    'the poll must not navigate/list the pinned pane off the focused tab cwd',
  );
  assert.equal(
    lastRemoteCall(h.renderCalls).deps.hostComboValue, 'main:1000007',
    'the pin must still hold after the suppressed poll tick',
  );

  // Unpin: the very next poll tick must resume normal navigate-on-cwd-change
  // behavior (mirrors scenario 8's onTabChanged-after-unpin check).
  await h.sandbox.filesPanel.pinRemotePane(null);
  await settle();
  h.invokeCalls.length = 0;

  await h.sandbox.filesPanel.pollActiveRemotePaneCwd(42);
  await settle();

  assert.ok(
    h.invokeCalls.some((c) => c.cmd === 'ssh_get_pane_cwd'),
    'after unpinning, the poll must query the focused tab cwd again',
  );
  assert.ok(
    h.invokeCalls.some((c) => c.cmd === 'sftp_list_dir'),
    'after unpinning, the poll must navigate on a cwd change again',
  );
  console.log('11. M1: cwd-follow poll gated on the pin, resumes after unpin: ok');
}

// --- 12 (L1). refreshHostCombo must not clear a still-in-flight busy state
// on an UNRELATED remote-sessions-changed event (another window's own
// connect/disconnect); it clears busy only once the busy entry's own
// session actually shows up in the refreshed sessions list. ----------------
{
  let resolveConnect;
  const pending = new Promise((resolve) => { resolveConnect = resolve; });
  const h = await setupLogicHarness((cmd) => {
    if (cmd === 'sftp_connect_host') return pending;
    return undefined;
  });
  await h.listeners['remote-sessions-changed']();
  await settle();
  const deps = lastRemoteCall(h.renderCalls).deps;

  const picked = deps.onHostComboChange('e-personal'); // host vps.example.com:22
  await settle(1);
  assert.equal(lastRemoteCall(h.renderCalls).deps.hostComboBusy, true, 'connect must render busy while in flight');

  // An unrelated sessions-changed event fires mid-flight. The refreshed
  // list still has no session for vps.example.com:22 — the racing connect
  // hasn't landed.
  await h.listeners['remote-sessions-changed']();
  await settle();
  assert.equal(
    lastRemoteCall(h.renderCalls).deps.hostComboBusy, true,
    'an unrelated sessions-changed event mid-flight must not clear busy',
  );

  // Now the busy entry's own session appears (as if the in-flight connect
  // just landed and this window is only now hearing about it).
  h.setSessionsFixture(makeSessionsFixture().concat([
    { key: 'main:1000088', host: 'vps.example.com', user: 'me', port: 22 },
  ]));
  await h.listeners['remote-sessions-changed']();
  await settle();
  assert.equal(
    lastRemoteCall(h.renderCalls).deps.hostComboBusy, false,
    "busy clears once the busy entry's session appears in the refreshed list",
  );

  // Let the original connect settle too so nothing is left dangling.
  resolveConnect({ sessionKey: 'main:1000088', host: 'vps.example.com', user: 'me', port: 22, paneId: 1000088 });
  await picked;
  await settle();
  console.log("12. L1: busy-clear scoped to the busy entry's session appearing: ok");
}

console.log('sftp connect part 1 (host dropdown + pinning): all assertions passed');

// =============================================================================
// Part 2 — Task 4: the auth dialog chain
// (frontend/app/features/files/connect-auth.js), driven off the typed
// SftpConnectError variants a connect attempt already got back.
//
// Loads the REAL tl-dialog.js, connect-auth.js, files-panel.js and
// features/files/data-service.js — no reimplementation, same idiom as Part
// 1's header describes. Unlike Part 1's harness (which stubs
// termlabFilesPaneView.renderPane to avoid needing a DOM at all), this part
// needs a real-enough `document` for tl-dialog.js's overlay/panel/focus-trap
// machinery and connect-auth.js's own `document.createElement`-built dialog
// bodies to run — the same minimal element factory
// test_editor_close_guards.mjs established for exactly that combination
// (no jsdom in this repo).
// =============================================================================

const FILES_DATA_SERVICE_PATH_2 = FILES_DATA_SERVICE_PATH; // same constant, just named for clarity below
const TL_DIALOG_PATH = path.join(FRONTEND, 'ui/tl-dialog.js');
const CONNECT_AUTH_PATH = path.join(FRONTEND, 'features/files/connect-auth.js');

// --- minimal DOM (mirrors test_editor_close_guards.mjs's makeElement) -----
function makeDomElement(tag, doc) {
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    disabled: false,
    tabIndex: 0,
    isConnected: false,
    innerHTML: '',
    type: '',
    checked: false,
    value: '',
    appendChild(child) {
      this.children.push(child);
      child.parentNode = this;
      child.isConnected = this.isConnected;
      return child;
    },
    removeChild(child) {
      const idx = this.children.indexOf(child);
      if (idx >= 0) this.children.splice(idx, 1);
      child.parentNode = null;
      child.isConnected = false;
      return child;
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    setAttribute(name, value) { attrs.set(name, String(value)); },
    getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
    removeAttribute(name) { attrs.delete(name); },
    hasAttribute(name) { return attrs.has(name); },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    removeEventListener(name, fn) {
      const arr = listeners.get(name) || [];
      const idx = arr.indexOf(fn);
      if (idx >= 0) arr.splice(idx, 1);
    },
    click() {
      for (const fn of (listeners.get('click') || []).slice()) fn({ target: this });
    },
    dispatch(name, event) {
      for (const fn of (listeners.get(name) || []).slice()) fn(event);
    },
    querySelector() { return null; },
    querySelectorAll() { return []; },
    contains(node) {
      let n = node;
      while (n) { if (n === this) return true; n = n.parentNode; }
      return false;
    },
    focus() { doc.activeElement = this; },
    getBoundingClientRect() { return { left: 0, top: 0, right: 100, bottom: 30, width: 100, height: 30 }; },
    classList: {
      _set: new Set(),
      add(...c) { c.forEach((x) => this._set.add(x)); },
      remove(...c) { c.forEach((x) => this._set.delete(x)); },
      contains(c) { return this._set.has(c); },
    },
  };
  Object.defineProperty(el, 'className', {
    get() { return Array.from(el.classList._set).join(' '); },
    set(v) { el.classList._set = new Set(String(v).split(' ').filter(Boolean)); },
  });
  let text = '';
  Object.defineProperty(el, 'textContent', {
    get() { return text; },
    set(v) {
      text = String(v == null ? '' : v);
      el.innerHTML = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    },
  });
  return el;
}

// Recursive tree-walk finders — connect-auth.js builds every dialog body via
// document.createElement + direct references (never innerHTML), so nothing
// here needs real CSS-selector parsing.
function findAll(root, pred, acc = []) {
  if (!root) return acc;
  if (pred(root)) acc.push(root);
  for (const child of root.children || []) findAll(child, pred, acc);
  return acc;
}
function findButton(root, label) {
  return findAll(root, (n) => n.tagName === 'BUTTON' && n.textContent === label)[0] || null;
}
function findByClass(root, cls) {
  return findAll(root, (n) => n.classList && n.classList.contains(cls))[0] || null;
}

// Builds a fresh sandbox with the real tl-dialog.js + connect-auth.js (+
// optionally files-panel.js for the integration scenarios), a spy around
// tlDialog.open recording every {opts, handle} pair so tests can drive
// whichever dialog is currently on top, and a controllable invoke.
function setupAuthHarness(invokeExtra, opts = {}) {
  const sandbox = {
    console: { ...console, warn: () => {} }, // suppress tl-dialog's "no keyboard router" noise
    setTimeout,
    clearTimeout,
    setInterval: () => 0,
    clearInterval: () => {},
    Promise,
    Math,
    Array,
    JSON,
    Object,
    String,
    Number,
  };
  sandbox.window = sandbox;
  sandbox.global = sandbox;
  sandbox.requestAnimationFrame = (fn) => fn();
  const document = {
    activeElement: null,
    createElement: (tag) => makeDomElement(tag, document),
    addEventListener() {},
    removeEventListener() {},
  };
  document.body = makeDomElement('body', document);
  document.body.isConnected = true;
  sandbox.document = document;
  sandbox.utils = { formatSize: () => '', formatDate: () => '', esc: (v) => String(v == null ? '' : v), attr: (v) => String(v == null ? '' : v) };
  sandbox.toast = { error() {}, info() {}, warn() {}, success() {} };
  sandbox.toolWindowManager = { isVisible: () => true, activate() {}, deactivate() {} };
  sandbox.tlCombo = { attach: () => ({ button: makeDomElement('button', document), refresh() {} }) };
  const renderCalls = [];
  sandbox.termlabFilesPaneView = {
    // Same recording-spy shape as Part 1's setupLogicHarness: deps carries
    // files-panel.js's actual closures (onHostComboChange etc.), so
    // integration scenarios can drive the combo without a full pane-view DOM.
    renderPane: (pane, el, deps) => { renderCalls.push({ pane, el, deps }); },
    showColumnMenu: () => {},
    showRowContextMenu: () => {},
  };
  sandbox.termlabFilesActions = {};
  sandbox.termlabFilesTransfers = {};

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(TL_DIALOG_PATH, 'utf8'), sandbox, { filename: TL_DIALOG_PATH });
  vm.runInContext(fs.readFileSync(CONNECT_AUTH_PATH, 'utf8'), sandbox, { filename: CONNECT_AUTH_PATH });
  vm.runInContext(fs.readFileSync(FILES_DATA_SERVICE_PATH_2, 'utf8'), sandbox, { filename: FILES_DATA_SERVICE_PATH_2 });
  if (opts.withFilesPanel) {
    vm.runInContext(fs.readFileSync(FILES_PANE_STORE_PATH, 'utf8'), sandbox, { filename: FILES_PANE_STORE_PATH });
    vm.runInContext(fs.readFileSync(FILES_PANEL_PATH, 'utf8'), sandbox, { filename: FILES_PANEL_PATH });
  }

  const dialogOpens = [];
  const realOpen = sandbox.tlDialog.open;
  sandbox.tlDialog.open = (dialogOpts) => {
    const handle = realOpen(dialogOpts);
    dialogOpens.push({ opts: dialogOpts, handle });
    return handle;
  };

  let serversFixture = { folders: [], ungrouped: [], ssh_config: [] };
  const invokeCalls = [];
  const invoke = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    const extra = invokeExtra ? invokeExtra(cmd, args, invokeCalls) : undefined;
    if (extra !== undefined) return extra;
    if (cmd === 'remote_get_servers') return Promise.resolve(serversFixture);
    return Promise.reject(new Error(`setupAuthHarness: unstubbed invoke ${cmd}`));
  };

  return {
    sandbox,
    document,
    invoke,
    invokeCalls,
    dialogOpens,
    renderCalls,
    setServersFixture: (v) => { serversFixture = v; },
    data: sandbox.termlabFilesFeatureDataService,
  };
}

// Latest remote-pane renderPane call, or throws — same helper as Part 1's
// lastRemoteCall, duplicated here because it closes over THIS section's
// local `renderCalls` arrays rather than Part 1's.
function lastRemoteAuthCall(renderCalls) {
  for (let i = renderCalls.length - 1; i >= 0; i -= 1) {
    if (renderCalls[i].pane.prefix === 'remote') return renderCalls[i];
  }
  throw new Error('no remote-pane renderPane call recorded yet');
}

function authFixtureWithVaultAccount(user) {
  return {
    folders: [],
    ungrouped: [
      { id: 'e-build', label: 'build-box', host: 'build.example.com', port: 22, user },
    ],
    ssh_config: [],
  };
}

const settle2 = async (times = 6) => { for (let i = 0; i < times; i += 1) await tick(); };

// --- 11. vaultLocked: wrong password re-prompts with an error line, then --
// the correct password unlocks and retries sftp_connect_host EXACTLY ONCE. -
{
  const session = { sessionKey: 'main:1000200', host: 'build.example.com', user: 'alice', port: 22, paneId: 1000200 };
  const h = setupAuthHarness((cmd, args) => {
    if (cmd === 'vault_unlock') {
      return args.request.password === 'correct-horse'
        ? Promise.resolve(null)
        : Promise.reject('Incorrect master password');
    }
    if (cmd === 'sftp_connect_host') return Promise.resolve(session);
    return undefined;
  });

  const resultPromise = h.sandbox.termlabConnectAuth.run(
    'e-build',
    { kind: 'vaultLocked' },
    { invoke: h.invoke, data: h.data, onError: () => {} },
  );
  await settle2();
  assert.equal(h.dialogOpens.length, 1, 'the master-password dialog must open immediately');

  const firstInput = findByClass(h.dialogOpens[0].handle.el, 'ca-master-password');
  assert.ok(firstInput, 'the master password field must be a type=password input');
  assert.equal(firstInput.type, 'password');
  firstInput.value = 'wrong-guess';
  findButton(h.dialogOpens[0].handle.el, 'Unlock').click();
  await settle2();

  assert.equal(firstInput.value, '', 'the DOM password field must be cleared on close');
  assert.equal(h.dialogOpens.length, 2, 'a wrong master password must re-prompt with a fresh dialog');
  const errLine = findByClass(h.dialogOpens[1].handle.el, 'ca-error-line');
  assert.ok(errLine && errLine.textContent.includes('Incorrect master password'), 'the re-prompt must show the rejection message');

  const secondInput = findByClass(h.dialogOpens[1].handle.el, 'ca-master-password');
  secondInput.value = 'correct-horse';
  findButton(h.dialogOpens[1].handle.el, 'Unlock').click();

  const result = await resultPromise;
  assert.deepEqual(result, session, 'the chain must resolve with the session the retried connect won');
  assert.equal(secondInput.value, '', 'the DOM password field must be cleared on close (success path too)');

  const connectHostCalls = h.invokeCalls.filter((c) => c.cmd === 'sftp_connect_host');
  assert.equal(connectHostCalls.length, 1, 'sftp_connect_host must be retried exactly once after unlock');
  assert.deepEqual(connectHostCalls[0].args, { serverEntryId: 'e-build' });
  const unlockCalls = h.invokeCalls.filter((c) => c.cmd === 'vault_unlock');
  assert.equal(unlockCalls.length, 2, 'one rejected attempt, one accepted attempt');
  console.log('11. vaultLocked: wrong-password re-prompt then unlock+retry-once: ok');
}

// --- 12. Cancel at the master-password rung: null, no further invokes, ----
// DOM field cleared. --------------------------------------------------------
{
  const h = setupAuthHarness(() => Promise.reject(new Error('must not be invoked after cancel')));
  const resultPromise = h.sandbox.termlabConnectAuth.run(
    'e-build',
    { kind: 'vaultLocked' },
    { invoke: h.invoke, data: h.data, onError: () => { throw new Error('onError must not fire on cancel'); } },
  );
  await settle2();
  const input = findByClass(h.dialogOpens[0].handle.el, 'ca-master-password');
  input.value = 'typed-then-abandoned';
  findButton(h.dialogOpens[0].handle.el, 'Cancel').click();

  const result = await resultPromise;
  assert.equal(result, null, 'cancelling the master-password dialog resolves null');
  assert.equal(input.value, '', 'the DOM field is cleared even when cancelling');
  assert.equal(h.invokeCalls.length, 0, 'no invoke call must happen after Cancel');
  console.log('12. vaultLocked cancel: null, no further invokes: ok');
}

// --- 13. needsPassword: title is "user@host", checkbox defaults CHECKED ---
// when hasVaultAccount is true, and saveToVault passes through as typed. ---
{
  const session = { sessionKey: 'main:1000201', host: 'build.example.com', user: 'alice', port: 22, paneId: 1000201 };
  const h = setupAuthHarness((cmd, args) => {
    if (cmd === 'sftp_connect_host_with_password') return Promise.resolve(session);
    return undefined;
  });
  h.setServersFixture(authFixtureWithVaultAccount('alice'));

  const resultPromise = h.sandbox.termlabConnectAuth.run(
    'e-build',
    { kind: 'needsPassword', hasVaultAccount: true },
    { invoke: h.invoke, data: h.data, onError: () => {} },
  );
  await settle2();
  assert.equal(h.dialogOpens.length, 1);
  assert.equal(h.dialogOpens[0].opts.title, 'alice@build.example.com', 'the dialog title must be "user@host" from the server entry');

  const checkbox = findByClass(h.dialogOpens[0].handle.el, 'ca-save-checkbox');
  assert.equal(checkbox.checked, true, 'Save to vault defaults CHECKED when hasVaultAccount is true');

  const input = findByClass(h.dialogOpens[0].handle.el, 'ca-host-password');
  assert.equal(input.type, 'password');
  input.value = 'hunter2';
  findButton(h.dialogOpens[0].handle.el, 'Connect').click();

  const result = await resultPromise;
  assert.deepEqual(result, session);
  assert.equal(input.value, '', 'the DOM password field must be cleared on close');
  const call = h.invokeCalls.find((c) => c.cmd === 'sftp_connect_host_with_password');
  assert.deepEqual(call.args, { serverEntryId: 'e-build', password: 'hunter2', saveToVault: true }, 'saveToVault passes through the checkbox state as typed');
  console.log('13. needsPassword: title, checkbox default true, saveToVault passthrough: ok');
}

// --- 14. needsPassword with hasVaultAccount=false: checkbox defaults ------
// UNCHECKED, and toggling it on still passes saveToVault=true through -------
// (proves the value isn't hardcoded to the default). -------------------------
{
  const session = { sessionKey: 'main:1000202', host: 'vps.example.com', user: 'bob', port: 22, paneId: 1000202 };
  const h = setupAuthHarness((cmd) => {
    if (cmd === 'sftp_connect_host_with_password') return Promise.resolve(session);
    return undefined;
  });
  h.setServersFixture(authFixtureWithVaultAccount('bob'));

  const resultPromise = h.sandbox.termlabConnectAuth.run(
    'e-build',
    { kind: 'needsPassword', hasVaultAccount: false },
    { invoke: h.invoke, data: h.data, onError: () => {} },
  );
  await settle2();
  const checkbox = findByClass(h.dialogOpens[0].handle.el, 'ca-save-checkbox');
  assert.equal(checkbox.checked, false, 'Save to vault defaults UNCHECKED when hasVaultAccount is false');
  checkbox.checked = true; // user opts in anyway

  findByClass(h.dialogOpens[0].handle.el, 'ca-host-password').value = 'letmein';
  findButton(h.dialogOpens[0].handle.el, 'Connect').click();
  await resultPromise;

  const call = h.invokeCalls.find((c) => c.cmd === 'sftp_connect_host_with_password');
  assert.equal(call.args.saveToVault, true, 'the checkbox toggle, not the default, decides saveToVault');
  console.log('14. needsPassword: checkbox default false, user toggle honored: ok');
}

// --- 15. Wrong host password re-prompts with an error line; the attempt ---
// counter only appears once two attempts have already failed (i.e. on the --
// dialog for the 3rd try). --------------------------------------------------
{
  const session = { sessionKey: 'main:1000203', host: 'build.example.com', user: 'alice', port: 22, paneId: 1000203 };
  let attempt = 0;
  const h = setupAuthHarness((cmd) => {
    if (cmd === 'sftp_connect_host_with_password') {
      attempt += 1;
      if (attempt < 3) return Promise.reject({ kind: 'authFailed', message: 'denied' });
      return Promise.resolve(session);
    }
    return undefined;
  });
  h.setServersFixture(authFixtureWithVaultAccount('alice'));

  const resultPromise = h.sandbox.termlabConnectAuth.run(
    'e-build',
    { kind: 'needsPassword', hasVaultAccount: true },
    { invoke: h.invoke, data: h.data, onError: () => {} },
  );
  await settle2();
  assert.equal(h.dialogOpens.length, 1);
  assert.equal(findByClass(h.dialogOpens[0].handle.el, 'ca-attempt-line'), null, 'no counter before any failure');
  findByClass(h.dialogOpens[0].handle.el, 'ca-host-password').value = 'bad-1';
  findButton(h.dialogOpens[0].handle.el, 'Connect').click();
  await settle2();

  assert.equal(h.dialogOpens.length, 2, 'attempt 1 failing must re-prompt');
  assert.ok(findByClass(h.dialogOpens[1].handle.el, 'ca-error-line'), 'the re-prompt shows the failure');
  assert.equal(findByClass(h.dialogOpens[1].handle.el, 'ca-attempt-line'), null, 'still no counter after only 1 failure (this is attempt 2)');
  findByClass(h.dialogOpens[1].handle.el, 'ca-host-password').value = 'bad-2';
  findButton(h.dialogOpens[1].handle.el, 'Connect').click();
  await settle2();

  assert.equal(h.dialogOpens.length, 3, 'attempt 2 failing must re-prompt again');
  const attemptLine = findByClass(h.dialogOpens[2].handle.el, 'ca-attempt-line');
  assert.ok(attemptLine, 'the counter must be visible on the dialog for the 3rd attempt');
  assert.ok(attemptLine.textContent.includes('3'), 'the counter must read attempt 3');
  findByClass(h.dialogOpens[2].handle.el, 'ca-host-password').value = 'right-at-last';
  findButton(h.dialogOpens[2].handle.el, 'Connect').click();

  const result = await resultPromise;
  assert.deepEqual(result, session);
  console.log('15. needsPassword: wrong-password re-prompt + counter at attempt 3: ok');
}

// --- 16. Cancel at the host-password rung: null, no further invokes, ------
// DOM field cleared. --------------------------------------------------------
{
  const h = setupAuthHarness(() => Promise.reject(new Error('must not be invoked after cancel')));
  h.setServersFixture(authFixtureWithVaultAccount('alice'));
  const resultPromise = h.sandbox.termlabConnectAuth.run(
    'e-build',
    { kind: 'needsPassword', hasVaultAccount: true },
    { invoke: h.invoke, data: h.data, onError: () => { throw new Error('onError must not fire on cancel'); } },
  );
  await settle2();
  const input = findByClass(h.dialogOpens[0].handle.el, 'ca-host-password');
  input.value = 'abandoned';
  findButton(h.dialogOpens[0].handle.el, 'Cancel').click();

  const result = await resultPromise;
  assert.equal(result, null);
  assert.equal(input.value, '', 'the DOM field is cleared even when cancelling');
  // getServers is expected (for the title); nothing else must fire.
  assert.ok(!h.invokeCalls.some((c) => c.cmd === 'sftp_connect_host_with_password'), 'no connect attempt after Cancel');
  console.log('16. needsPassword cancel: null, no further invokes: ok');
}

// --- 17. unreachable/other at the password rung: no re-prompt, the -------
// message is routed to onError, and the chain resolves null. ---------------
{
  const h = setupAuthHarness((cmd) => {
    if (cmd === 'sftp_connect_host_with_password') return Promise.reject({ kind: 'unreachable', message: 'DNS failure' });
    return undefined;
  });
  h.setServersFixture(authFixtureWithVaultAccount('alice'));
  let reported = null;
  const resultPromise = h.sandbox.termlabConnectAuth.run(
    'e-build',
    { kind: 'needsPassword', hasVaultAccount: true },
    { invoke: h.invoke, data: h.data, onError: (message) => { reported = message; } },
  );
  await settle2();
  findByClass(h.dialogOpens[0].handle.el, 'ca-host-password').value = 'x';
  findButton(h.dialogOpens[0].handle.el, 'Connect').click();

  const result = await resultPromise;
  assert.equal(result, null);
  assert.equal(h.dialogOpens.length, 1, 'unreachable must not re-prompt');
  assert.equal(reported, 'DNS failure', 'the message must be routed to the caller error surface');
  console.log('17. needsPassword unreachable: routed to onError, resolves null, no re-prompt: ok');
}

// --- 18. connectInProgress reaching run() directly (defensive — the ------
// production caller filters this out before calling run(), see files-panel)
// is a quiet no-op: no dialog, no onError, resolves null. -------------------
{
  const h = setupAuthHarness(() => Promise.reject(new Error('must not be invoked')));
  let errorCalled = false;
  const result = await h.sandbox.termlabConnectAuth.run(
    'e-build',
    { kind: 'connectInProgress' },
    { invoke: h.invoke, data: h.data, onError: () => { errorCalled = true; } },
  );
  assert.equal(result, null);
  assert.equal(h.dialogOpens.length, 0, 'connectInProgress must never raise a dialog');
  assert.equal(errorCalled, false, 'connectInProgress must never surface as an error');
  console.log('18. connectInProgress reaching run() directly: quiet no-op: ok');
}

// --- 19. authFailed as the starting error (defensive — not expected from -
// a first sftp_connect_host today, but the type allows it): routed to -----
// onError, resolves null, no dialog. -----------------------------------------
{
  const h = setupAuthHarness(() => Promise.reject(new Error('must not be invoked')));
  let reported = null;
  const result = await h.sandbox.termlabConnectAuth.run(
    'e-build',
    { kind: 'authFailed', message: 'bad key' },
    { invoke: h.invoke, data: h.data, onError: (m) => { reported = m; } },
  );
  assert.equal(result, null);
  assert.equal(h.dialogOpens.length, 0);
  assert.equal(reported, 'Authentication failed: bad key');
  console.log('19. authFailed as starting error: routed to onError, no dialog: ok');
}

// --- 20. The bridge is gone: files-panel.js no longer describes typed -----
// errors as a static string itself; it routes them through connectAuth.run.
{
  const filesPanelSrc = fs.readFileSync(FILES_PANEL_PATH, 'utf8');
  assert.ok(!filesPanelSrc.includes('describeSftpConnectError'), 'describeSftpConnectError must be fully removed, not just unused');
  assert.ok(filesPanelSrc.includes('connectAuth.run'), 'connectToHost must route non-Ok kinds through connectAuth.run');
  console.log('20. bridge removed, chain wired in: ok');
}

// --- 21. Integration: files-panel.js's connectToHost end to end through ---
// the real connect-auth.js chain — vaultLocked -> unlock dialog -> retried
// sftp_connect_host -> pin. Proves the wiring (not just connect-auth.js in
// isolation): hostConnectBusyEntryId clears and the remote pane pins once
// the chain eventually succeeds. sftp_connect_host is stubbed to reject
// vaultLocked on its FIRST call and resolve the session on the retry (its
// second call), which is exactly the "retry sftp_connect_host ONCE" shape.
{
  const session = { sessionKey: 'main:1000204', host: 'build.example.com', user: 'alice', port: 22, paneId: 1000204 };
  let connectHostCalls = 0;
  const h = setupAuthHarness((cmd, args) => {
    if (cmd === 'sftp_connect_host') {
      connectHostCalls += 1;
      return connectHostCalls === 1 ? Promise.reject({ kind: 'vaultLocked' }) : Promise.resolve(session);
    }
    if (cmd === 'vault_unlock') return Promise.resolve(null);
    if (cmd === 'get_home_dir') return Promise.resolve('/home/demo');
    if (cmd === 'get_all_settings') return Promise.resolve({});
    if (cmd === 'local_list_dir') return Promise.resolve([]);
    if (cmd === 'remote_get_sessions') return Promise.resolve([]);
    if (cmd === 'sftp_realpath') return Promise.resolve('/home/pinned');
    if (cmd === 'sftp_list_dir') return Promise.resolve([]);
    return undefined;
  }, { withFilesPanel: true });
  h.setServersFixture({
    folders: [], ungrouped: [{ id: 'e-build', label: 'build-box', host: 'build.example.com', port: 22, user: 'alice' }], ssh_config: [],
  });

  const panelEl = makeDomElement('div', h.document);
  const localRootEl = makeDomElement('div', h.document);
  const remoteRootEl = makeDomElement('div', h.document);
  panelEl.querySelector = (sel) => {
    if (sel === '#fp-local') return localRootEl;
    if (sel === '#fp-remote') return remoteRootEl;
    return null;
  };

  const listeners = {};
  h.sandbox.filesPanel.init({
    invoke: h.invoke,
    panelEl,
    panelWrapEl: makeDomElement('div', h.document),
    resizeHandleEl: makeDomElement('div', h.document),
    layoutService: null,
    fitActiveTab: () => {},
    getActiveTab: () => null,
    listen: (name, handler) => { listeners[name] = handler; },
  });
  await settle2();
  h.invokeCalls.length = 0;
  h.renderCalls.length = 0;

  // Force a render so there is a `deps` to read onHostComboChange off (Part
  // 1's setupLogicHarness discards boot-traffic renders the same way).
  await listeners['remote-sessions-changed']();
  await settle2();
  const deps = lastRemoteAuthCall(h.renderCalls).deps;

  const connectPromise = deps.onHostComboChange('e-build'); // configured host, not a live session
  await settle2();
  assert.equal(h.dialogOpens.length, 1, 'the vaultLocked rung must raise the master-password dialog');
  assert.equal(lastRemoteAuthCall(h.renderCalls).deps.hostComboBusy, true, 'busy must render while the dialog is up');

  findByClass(h.dialogOpens[0].handle.el, 'ca-master-password').value = 'correct-horse';
  findButton(h.dialogOpens[0].handle.el, 'Unlock').click();
  await connectPromise;
  await settle2();

  assert.equal(connectHostCalls, 2, 'sftp_connect_host must be called once up front and once as the post-unlock retry');
  const after = lastRemoteAuthCall(h.renderCalls).deps;
  assert.equal(after.hostComboBusy, false, 'busy clears once the chain resolves');
  assert.equal(after.hostComboValue, 'main:1000204', 'a chain that eventually succeeds pins to the won session');
  console.log('21. integration: files-panel connectToHost drives the real chain to a pin: ok');
}

console.log('sftp connect part 2 (auth dialog chain): all assertions passed');
