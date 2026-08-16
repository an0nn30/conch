// Run: node scripts/tests/test_command_palette_grouping.mjs
//
// Regression test for phase 5b task-3 review round 1: renderPaletteResults()
// used to iterate a *grouped* re-ordering of the result list while every
// execution path (digit quick-pick, click, Enter-after-arrow) indexed into
// the *flat, score-sorted* `commandPalette.filtered` array — two different
// orderings sharing one index space. Whenever a query's top matches spanned
// more than one group, the two orderings diverged and a user selecting the
// row they could SEE at position N executed whatever command happened to
// sit at position N in the other array instead.
//
// No jsdom in this repo (see test_tl_dialog.mjs for the precedent) — this
// stubs just enough of window/document for command-palette-runtime.js to
// load and run for real, plus a stub window.tlDialog/window.termlabKeyboardRouter
// (command-palette-runtime.js no longer builds its own overlay; it hands its
// body to tlDialog.open() and registers its own keydown handler with the
// router — both stubbed here rather than re-implemented, since tl-dialog.js
// and keyboard-router.js are exercised by their own test files / by real
// use, not by this one).
//
// Fixture design: five synthetic commands spanning four different groups
// (Actions x2, SSH Hosts, Plugins, Tunnels), all built through the REAL
// buildPaletteCommands() pipeline (via mocked `invoke`) so the real
// templates/fuzzyScore/group tagging are exercised, not reimplemented here.
// The query "z" is chosen so every fixture's haystack scores exactly 1 (all
// haystacks exceed fuzzyScore's length-bonus window for a 1-char query, and
// no built-in core:* command's title/subtitle/keywords contains "z" — see
// buildPaletteCommands() — so score ties break on title.localeCompare(),
// making the flat order pure alphabetical and fully controlled by the
// fixture titles below). That flat order is DIFFERENT from the
// group-stable-partitioned render order at positions 1 and 2 (this file's
// EXPECTED_FLAT_ORDER/EXPECTED_RENDER_ORDER below) — exactly the scenario
// the reviewer reproduced by hand.
import assert from 'node:assert';

// --- minimal element stub ---------------------------------------------------
function makeElement(tag) {
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    children: [],
    style: {},
    disabled: false,
    isConnected: false,
    value: '',
    type: '',
    placeholder: '',
    spellcheck: false,
    _text: '',
    appendChild(child) { this.children.push(child); child.parentNode = this; return child; },
    removeChild(child) {
      const i = this.children.indexOf(child);
      if (i >= 0) this.children.splice(i, 1);
      child.parentNode = null;
      return child;
    },
    remove() { if (this.parentNode) this.parentNode.removeChild(this); },
    setAttribute(n, v) { attrs.set(n, String(v)); },
    getAttribute(n) { return attrs.has(n) ? attrs.get(n) : null; },
    removeAttribute(n) { attrs.delete(n); },
    hasAttribute(n) { return attrs.has(n); },
    addEventListener(name, fn) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(fn);
    },
    removeEventListener(name, fn) {
      const arr = listeners.get(name) || [];
      const i = arr.indexOf(fn);
      if (i >= 0) arr.splice(i, 1);
    },
    dispatch(name, evt) {
      for (const fn of (listeners.get(name) || []).slice()) fn(evt);
    },
    click() { this.dispatch('click', { target: this }); },
    focus() {},
    querySelectorAll() { return []; },
    classList: {
      _set: new Set(),
      add(c) { this._set.add(c); },
      remove(c) { this._set.delete(c); },
      contains(c) { return this._set.has(c); },
    },
  };
  Object.defineProperty(el, 'className', {
    get() { return Array.from(el.classList._set).join(' '); },
    set(v) { el.classList._set = new Set(String(v).split(' ').filter(Boolean)); },
  });
  Object.defineProperty(el, 'textContent', {
    get() { return el._text; },
    set(v) { el._text = v; },
  });
  Object.defineProperty(el, 'innerHTML', {
    get() { return ''; },
    set(_v) { el.children = []; },
  });
  return el;
}

function findChildByClass(el, cls) {
  return el.children.find((c) => c.className && c.className.split(' ').includes(cls));
}

function findPaletteRows(resultsEl) {
  const rows = [];
  for (const groupEl of resultsEl.children) {
    for (const child of groupEl.children) {
      if (child.className && child.className.split(' ').includes('tl-palette__item')) rows.push(child);
    }
  }
  return rows;
}

function rowTitle(row) {
  const mainEl = findChildByClass(row, 'tl-palette__main');
  const titleEl = findChildByClass(mainEl, 'tl-palette__title');
  return titleEl.textContent;
}

function fakeKeyEvent(key) {
  return { key, preventDefault() {}, stopPropagation() {} };
}

// --- window/document stubs --------------------------------------------------
const window = {};
globalThis.window = window;
const document = {
  activeElement: null,
  createElement: (tag) => makeElement(tag),
  querySelector: () => null,
  addEventListener() {},
  removeEventListener() {},
};
globalThis.document = document;
window.document = document;

let capturedDialogOpts = null;
let dialogOpenCount = 0;
window.tlDialog = {
  open(opts) {
    capturedDialogOpts = opts;
    dialogOpenCount = 1;
    const fakePanel = makeElement('div');
    fakePanel.isConnected = true;
    if (typeof opts.onOpen === 'function') opts.onOpen(fakePanel);
    return {
      el: fakePanel,
      close(result) {
        dialogOpenCount = 0;
        if (typeof opts.onClose === 'function') opts.onClose(result);
      },
    };
  },
  count: () => dialogOpenCount,
};

let registeredRouterHandler = null;
window.termlabKeyboardRouter = {
  register(options) {
    registeredRouterHandler = options;
    return () => { if (registeredRouterHandler === options) registeredRouterHandler = null; };
  },
};

// --- deps: invoke/createSshTab are spies; everything else is a no-op -------
const invokeCalls = [];
const sshConnectCalls = [];

function invoke(cmd, args) {
  invokeCalls.push({ cmd, args });
  if (cmd === 'scan_plugins') {
    return Promise.resolve([{ name: 'Czzz Gamma', loaded: false, source: 's', path: '/p' }]);
  }
  if (cmd === 'get_plugin_menu_items') {
    return Promise.resolve([
      { plugin: 'p1', label: 'Azzz Alpha', action: 'a1' },
      { plugin: 'p2', label: 'Dzzz Delta', action: 'a2' },
    ]);
  }
  if (cmd === 'remote_get_servers') {
    return Promise.resolve({
      ungrouped: [{ id: 's1', label: 'Bzzz Beta', user: '', host: 'g', port: 22 }],
      folders: [],
      ssh_config: [],
    });
  }
  if (cmd === 'tunnel_get_all') {
    return Promise.resolve([
      { id: 't1', label: 'Ezzz Epsilon', status: 'inactive', local_port: 1, remote_host: 'h', remote_port: 2 },
    ]);
  }
  return Promise.resolve(undefined);
}

function listen() { return Promise.resolve(); }
const esc = (s) => s;

const deps = {
  invoke,
  listen,
  esc,
  handleMenuAction: () => {},
  createSshTab: (args) => { sshConnectCalls.push(args); },
  getCurrentPane: () => null,
  showStatus: () => {},
  refreshTitlebar: () => {},
  refreshSshPanel: () => {},
};

const { readFileSync } = await import('node:fs');
eval(readFileSync('crates/termlab_tauri/frontend/app/command-palette-runtime.js', 'utf8'));

const runtime = window.termlabCommandPaletteRuntime.create(deps);

// The five fixtures' expected FLAT (score-tied, alphabetical) order vs. the
// expected GROUPED (stable-partition-by-group) render order — computed and
// verified independently in a throwaway probe script during development,
// reproduced here as the two orderings the bug conflated.
const EXPECTED_FLAT_ORDER = [
  'Azzz Alpha',                  // Actions
  'Connect: Bzzz Beta',          // SSH Hosts
  'Dzzz Delta',                  // Actions
  'Enable Plugin: Czzz Gamma',   // Plugins
  'Start Tunnel: Ezzz Epsilon',  // Tunnels
];
const EXPECTED_RENDER_ORDER = [
  'Azzz Alpha',                  // Actions
  'Dzzz Delta',                  // Actions (pulled forward: same group as row 0)
  'Connect: Bzzz Beta',          // SSH Hosts
  'Enable Plugin: Czzz Gamma',   // Plugins
  'Start Tunnel: Ezzz Epsilon',  // Tunnels
];
assert.notDeepEqual(EXPECTED_FLAT_ORDER, EXPECTED_RENDER_ORDER, 'fixture must actually interleave, or this test proves nothing');

async function openAndQuery() {
  await runtime.open();
  const input = capturedDialogOpts.body.children[0];
  input.value = 'z';
  input.dispatch('input', {});
  const resultsEl = capturedDialogOpts.body.children[1];
  const rows = findPaletteRows(resultsEl);
  return { input, rows };
}

// --- sanity: rendered rows show the grouped order, not the flat order ------
{
  const { rows } = await openAndQuery();
  assert.equal(rows.length, 5, 'all 5 synthetic commands should match query "z"');
  const titles = rows.map(rowTitle);
  assert.deepEqual(titles, EXPECTED_RENDER_ORDER, 'rendered rows must be in group-stable-partition order');
  console.log('render order matches expected grouped order: ok');
}

// Row 1 (0-indexed), badge "2", displays "Dzzz Delta" (an Actions/plugin-menu
// command, plugin p2/action a2) in the render order — but sits at flat-order
// index 1, "Connect: Bzzz Beta" (an SSH Hosts command). Selecting rendered
// row 1 by any of the three mechanisms below must run the plugin-menu
// action, never the SSH connect.

// --- digit quick-pick --------------------------------------------------------
{
  const { rows } = await openAndQuery();
  assert.equal(rowTitle(rows[1]), 'Dzzz Delta', 'row 1 must display Dzzz Delta');
  invokeCalls.length = 0;
  sshConnectCalls.length = 0;
  const consumed = registeredRouterHandler.onKeyDown(fakeKeyEvent('2'));
  assert.equal(consumed, true, 'digit 2 should be consumed');
  const pluginCall = invokeCalls.find((c) => c.cmd === 'trigger_plugin_menu_action');
  assert.ok(pluginCall, `digit "2" must run the command displayed at row 1 (Dzzz Delta); invoke calls were: ${JSON.stringify(invokeCalls)}, sshConnectCalls were: ${JSON.stringify(sshConnectCalls)}`);
  assert.deepEqual(pluginCall.args, { pluginName: 'p2', action: 'a2' });
  assert.equal(sshConnectCalls.length, 0, 'digit "2" must NOT connect to the SSH host shown at a different row');
  console.log('digit quick-pick executes the row it displays: ok');
}

// --- click -------------------------------------------------------------------
{
  const { rows } = await openAndQuery();
  assert.equal(rowTitle(rows[1]), 'Dzzz Delta', 'row 1 must display Dzzz Delta');
  invokeCalls.length = 0;
  sshConnectCalls.length = 0;
  rows[1].click();
  const pluginCall = invokeCalls.find((c) => c.cmd === 'trigger_plugin_menu_action');
  assert.ok(pluginCall, `clicking row 1 must run the command displayed there (Dzzz Delta); invoke calls were: ${JSON.stringify(invokeCalls)}`);
  assert.deepEqual(pluginCall.args, { pluginName: 'p2', action: 'a2' });
  assert.equal(sshConnectCalls.length, 0, 'clicking row 1 must NOT connect to the SSH host shown at a different row');
  console.log('click executes the row it displays: ok');
}

// --- Enter after arrow navigation --------------------------------------------
{
  const { rows } = await openAndQuery();
  assert.equal(rowTitle(rows[1]), 'Dzzz Delta', 'row 1 must display Dzzz Delta');
  invokeCalls.length = 0;
  sshConnectCalls.length = 0;
  registeredRouterHandler.onKeyDown(fakeKeyEvent('ArrowDown')); // selectedIndex 0 -> 1
  const consumed = registeredRouterHandler.onKeyDown(fakeKeyEvent('Enter'));
  assert.equal(consumed, true, 'Enter should be consumed');
  const pluginCall = invokeCalls.find((c) => c.cmd === 'trigger_plugin_menu_action');
  assert.ok(pluginCall, `Enter after ArrowDown to row 1 must run the command displayed there (Dzzz Delta); invoke calls were: ${JSON.stringify(invokeCalls)}`);
  assert.deepEqual(pluginCall.args, { pluginName: 'p2', action: 'a2' });
  assert.equal(sshConnectCalls.length, 0, 'Enter after arrow-navigating to row 1 must NOT connect to the SSH host shown at a different row');
  console.log('Enter after arrow-navigation executes the row it displays: ok');
}

console.log('ok');
