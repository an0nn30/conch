// Run: node scripts/tests/test_command_palette_scroll_into_view.mjs
//
// Regression test for phase 5b final-review finding 3: MAX_RESULTS went from
// 5 to 20 (app/command-palette-runtime.js) but .tl-palette__results became a
// fixed-height scrolling box (styles/design-system/components/palette.css —
// height: min(480px, 66vh)) and neither renderPaletteResults() nor the
// ArrowUp/ArrowDown branches ever scrolled the selected row into view. At
// ~43px/row only about half the results are visible, so arrow-navigating
// past the fold moved commandPalette.selectedIndex correctly (as
// test_command_palette_grouping.mjs already covers) but left the highlighted
// row scrolled out of sight — further ArrowDown presses looked like no-ops
// and Enter ran a command the user could not see.
//
// Same "no jsdom, load the real module" approach as
// test_command_palette_grouping.mjs (see its header comment for the
// rationale) — this file reuses that stubbing shape but adds a
// scrollIntoView() spy to the element stub so the fix (renderPaletteResults()
// tracking the selected row and calling row.scrollIntoView({block:
// 'nearest'}) once per render) can be asserted on directly: every render
// must call scrollIntoView exactly on the row currently carrying
// selectedIndex, not on some other row and not zero times.
import assert from 'node:assert';

// --- minimal element stub (adds a scrollIntoView spy over the grouping
// test's version) -----------------------------------------------------------
const scrollCalls = [];
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
    // The fix under test: renderPaletteResults() calls this on the row
    // matching commandPalette.selectedIndex, once per render, guarded by
    // `typeof row.scrollIntoView === 'function'`. Recording (el, opts) here
    // — rather than just a counter — is what lets the assertions below
    // confirm it was called on the CORRECT row, not merely called at all.
    scrollIntoView(opts) { scrollCalls.push({ el, opts }); },
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

// --- deps: eight same-group SSH hosts, all matching query "z", named so
// alphabetical (score-tied) order equals creation order --------------------
function invoke(cmd) {
  if (cmd === 'remote_get_servers') {
    const ungrouped = [];
    for (let i = 0; i < 8; i++) {
      ungrouped.push({ id: `s${i}`, label: `Zzz Row${i}`, user: '', host: 'g', port: 22 });
    }
    return Promise.resolve({ ungrouped, folders: [], ssh_config: [] });
  }
  return Promise.resolve(cmd === 'scan_plugins' || cmd === 'get_plugin_menu_items' ? [] : undefined);
}

function listen() { return Promise.resolve(); }
const esc = (s) => s;

const deps = {
  invoke,
  listen,
  esc,
  handleMenuAction: () => {},
  createSshTab: () => {},
  getCurrentPane: () => null,
  showStatus: () => {},
  refreshTitlebar: () => {},
  refreshSshPanel: () => {},
};

const { readFileSync } = await import('node:fs');
eval(readFileSync('crates/termlab_tauri/frontend/app/command-palette-runtime.js', 'utf8'));

const runtime = window.termlabCommandPaletteRuntime.create(deps);

async function openAndQuery() {
  await runtime.open();
  const input = capturedDialogOpts.body.children[0];
  input.value = 'z';
  input.dispatch('input', {});
  const resultsEl = capturedDialogOpts.body.children[1];
  return findPaletteRows(resultsEl);
}

function lastScrollTarget() {
  assert.ok(scrollCalls.length > 0, 'scrollIntoView should have been called at least once by now');
  return scrollCalls[scrollCalls.length - 1];
}

// --- initial render scrolls row 0 into view ---------------------------------
{
  scrollCalls.length = 0;
  const rows = await openAndQuery();
  assert.equal(rows.length, 8, 'all 8 synthetic SSH hosts should match query "z"');
  assert.deepEqual(rows.map(rowTitle), ['Connect: Zzz Row0', 'Connect: Zzz Row1', 'Connect: Zzz Row2',
    'Connect: Zzz Row3', 'Connect: Zzz Row4', 'Connect: Zzz Row5', 'Connect: Zzz Row6', 'Connect: Zzz Row7'],
    'fixture rows must be in creation order (alphabetical, score-tied)');
  const last = lastScrollTarget();
  assert.equal(last.el, rows[0], 'the initial render (selectedIndex 0) must scroll row 0 into view');
  assert.deepEqual(last.opts, { block: 'nearest' });
  console.log('initial render scrolls the selected row into view: ok');
}

// --- ArrowDown past the fold keeps scrolling the newly-selected row --------
{
  await openAndQuery(); // re-open fresh (selectedIndex resets to 0)
  for (let step = 1; step <= 6; step++) {
    scrollCalls.length = 0;
    registeredRouterHandler.onKeyDown(fakeKeyEvent('ArrowDown'));
    const resultsEl = capturedDialogOpts.body.children[1];
    const rows = findPaletteRows(resultsEl);
    assert.equal(rowTitle(rows[step]), `Connect: Zzz Row${step}`, `row ${step} should now display Zzz Row${step}`);
    assert.ok(rows[step].classList.contains('is-active'), `row ${step} should carry .is-active after ${step} ArrowDown presses`);
    const last = lastScrollTarget();
    assert.equal(last.el, rows[step], `ArrowDown press ${step} must scroll the newly-selected row (index ${step}) into view, not some other row`);
  }
  console.log('ArrowDown scrolls the newly-selected row into view at every step, including past the fold: ok');
}

// --- ArrowUp back toward the top keeps scrolling the newly-selected row ----
{
  scrollCalls.length = 0;
  registeredRouterHandler.onKeyDown(fakeKeyEvent('ArrowUp')); // 6 -> 5
  const resultsEl = capturedDialogOpts.body.children[1];
  const rows = findPaletteRows(resultsEl);
  assert.equal(rowTitle(rows[5]), 'Connect: Zzz Row5');
  const last = lastScrollTarget();
  assert.equal(last.el, rows[5], 'ArrowUp must scroll the newly-selected row (index 5) into view');
  console.log('ArrowUp scrolls the newly-selected row into view: ok');
}

console.log('ok');
