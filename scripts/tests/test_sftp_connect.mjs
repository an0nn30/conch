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

// Builds a fresh vm context, loads the real pane-store/data-service/panel
// modules into it, stubs termlabFilesPaneView.renderPane as a recording spy,
// and runs filesPanel.init(). `invokeExtra(cmd, args)` may return a Promise
// (or undefined to fall through to the default fixture responses) so each
// scenario can override exactly the calls it cares about.
async function setupLogicHarness(invokeExtra) {
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
  sandbox.termlabFilesActions = {};
  sandbox.termlabFilesTransfers = {};

  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(FILES_PANE_STORE_PATH, 'utf8'), sandbox, { filename: FILES_PANE_STORE_PATH });
  vm.runInContext(fs.readFileSync(FILES_DATA_SERVICE_PATH, 'utf8'), sandbox, { filename: FILES_DATA_SERVICE_PATH });
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
    getActiveTab: () => null,
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

console.log('sftp connect: all assertions passed');
