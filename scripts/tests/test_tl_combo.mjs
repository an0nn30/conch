// Run: node scripts/tests/test_tl_combo.mjs
//
// No jsdom available in this repo, so — like test_tl_dialog.mjs and
// test_tl_icon.mjs — this stubs just enough of `window`/`document` for the
// real app/ui/tl-menu.js and app/ui/tl-combo.js to load via eval(), then
// exercises the riskiest logic called out in the design-system-phase-5a
// task-2 brief: tl-menu's arrow-key navigation (wrap, skip disabled, jump),
// and tl-combo's idempotent attach(), fresh-options-on-every-open, checked
// state, change-event dispatch, label/disabled sync, and the aria-haspopup
// fix from task-2 review finding #1. It does not attempt to drive a real
// layout engine (positioning, focus rings, etc. — exercised by hand per the
// brief's manual smoke-run instructions).
import assert from 'node:assert';

// --- minimal element stub -------------------------------------------------
function makeElement(tag) {
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    className: '',
    children: [],
    style: {},
    disabled: false,
    tabIndex: 0,
    isConnected: false,
    options: [],
    selectedIndex: -1,
    value: '',
    appendChild(child) {
      this.children.push(child);
      child.parentNode = this;
      child.isConnected = this.isConnected;
      return child;
    },
    insertAdjacentElement(where, child) {
      // Only 'afterend' is used by tl-combo.js: splice into the parent's
      // children right after this node.
      if (this.parentNode) {
        const idx = this.parentNode.children.indexOf(this);
        this.parentNode.children.splice(idx + 1, 0, child);
        child.parentNode = this.parentNode;
        child.isConnected = this.parentNode.isConnected;
      }
      return child;
    },
    removeChild(child) {
      const idx = this.children.indexOf(child);
      if (idx >= 0) this.children.splice(idx, 1);
      child.parentNode = null;
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
    dispatchEvent(evt) {
      const arr = listeners.get(evt.type) || [];
      for (const fn of arr.slice()) fn(evt);
      return true;
    },
    querySelectorAll(sel) {
      // Only '.tl-menu__item' is used, by tl-menu.js's arrow-nav handler.
      const out = [];
      const walk = (node) => {
        for (const c of node.children) {
          if (sel === '.tl-menu__item' && c.className && c.className.split(' ').includes('tl-menu__item')) out.push(c);
          walk(c);
        }
      };
      walk(this);
      return out;
    },
    contains(node) {
      let n = node;
      while (n) { if (n === this) return true; n = n.parentNode; }
      return false;
    },
    querySelector(sel) {
      if (sel === '.tl-menu__item:not(.is-disabled)') {
        return this.querySelectorAll('.tl-menu__item').find((c) => !c.classList.contains('is-disabled')) || null;
      }
      return null;
    },
    focus() { document.activeElement = this; },
    getBoundingClientRect() { return { left: 10, top: 10, right: 100, bottom: 36, width: 90, height: 26 }; },
    classList: {
      _set: new Set(),
      add(c) { this._set.add(c); },
      remove(c) { this._set.delete(c); },
      contains(c) { return this._set.has(c); },
    },
  };
  // className setter keeps classList in sync (tl-menu.js's buildItemEl sets
  // el.className directly rather than going through classList.add).
  Object.defineProperty(el, 'className', {
    get() { return Array.from(el.classList._set).join(' '); },
    set(v) { el.classList._set = new Set(String(v).split(' ').filter(Boolean)); },
  });
  // Mimic native <select>.value: setting it updates selectedIndex to match,
  // same as a real <select> — tl-combo.js's refresh()/currentLabel() depend
  // on selectedIndex tracking value.
  if (tag === 'select') {
    let currentValue = '';
    Object.defineProperty(el, 'value', {
      get() { return currentValue; },
      set(v) {
        currentValue = v;
        const idx = el.options.findIndex((o) => o.value === v);
        if (idx !== -1) el.selectedIndex = idx;
      },
    });
  }
  return el;
}

const window = {};
globalThis.window = window;
const document = {
  activeElement: null,
  body: makeElement('body'),
  createElement: (tag) => makeElement(tag),
  addEventListener() {},
  removeEventListener() {},
};
document.body.isConnected = true;
globalThis.document = document;
window.document = document;
window.requestAnimationFrame = (fn) => fn();
globalThis.requestAnimationFrame = window.requestAnimationFrame;
window.MutationObserver = class { observe() {} disconnect() {} };
globalThis.MutationObserver = window.MutationObserver;

const { readFileSync } = await import('node:fs');
// Load the real production modules — this test exercises the actual
// window.tlMenu/window.tlCombo functions, not a reimplementation of them.
eval(readFileSync('crates/termlab_tauri/frontend/app/ui/tl-menu.js', 'utf8'));
eval(readFileSync('crates/termlab_tauri/frontend/app/ui/tl-combo.js', 'utf8'));

// --- tl-menu arrow-key nav (ArrowDown/ArrowUp/Home/End) ---------------------
{
  const menu = window.tlMenu.open({
    x: 0, y: 0,
    items: [
      { label: 'a', onSelect() {} },
      { label: 'b', disabled: true, onSelect() {} },
      { label: 'c', onSelect() {} },
    ],
  });
  const items = menu.querySelectorAll('.tl-menu__item');
  assert.equal(items.length, 3);
  // requestAnimationFrame (stubbed as synchronous above) already focused
  // the first enabled item.
  assert.equal(document.activeElement, items[0], 'open() focuses first enabled item');

  const fireKey = (key) => menu.dispatchEvent({ type: 'keydown', key, preventDefault() {} });
  fireKey('ArrowDown');
  assert.equal(document.activeElement, items[2], 'ArrowDown skips disabled item b, wraps to c');
  fireKey('ArrowDown');
  assert.equal(document.activeElement, items[0], 'ArrowDown wraps back to a');
  fireKey('ArrowUp');
  assert.equal(document.activeElement, items[2], 'ArrowUp wraps to last enabled item');
  fireKey('Home');
  assert.equal(document.activeElement, items[0], 'Home jumps to first enabled item');
  fireKey('End');
  assert.equal(document.activeElement, items[2], 'End jumps to last enabled item');
  window.tlMenu.close();
  console.log('tl-menu arrow-nav: ok');
}

// --- tl-combo: aria-haspopup agrees with what tl-menu renders --------------
{
  const select = makeElement('select');
  select.parentNode = document.body;
  document.body.appendChild(select);
  select.options = [];
  const api = window.tlCombo.attach(select);
  // tl-menu always renders role="menu" / role="menuitem(checkbox)" (see
  // tl-menu.js's open()/buildItemEl()) — review finding #1 required the
  // trigger's aria-haspopup to match that rendered widget instead of
  // claiming "listbox".
  assert.equal(api.button.getAttribute('aria-haspopup'), 'menu', 'aria-haspopup must agree with tl-menu\'s role="menu" popup');
  console.log('tl-combo aria-haspopup: ok');
}

// --- tl-combo: attach, idempotency, re-read-on-open, change dispatch -------
{
  const select = makeElement('select');
  select.parentNode = document.body;
  document.body.appendChild(select);
  const mkOpt = (value, text) => {
    const o = makeElement('option');
    o.value = value; o.textContent = text; o.disabled = false;
    return o;
  };
  select.options = [mkOpt('a', 'Alpha'), mkOpt('b', 'Bravo')];
  select.options.forEach((o, i) => { o.index = i; });
  select.selectedIndex = 0;
  select.value = 'a';

  const api1 = window.tlCombo.attach(select);
  const api2 = window.tlCombo.attach(select);
  assert.equal(api1, api2, 'attach() twice must be idempotent (same api/button)');
  // Count how many times api1's own button node appears in the DOM (not
  // "any .tl-combo button", since an earlier test block in this file also
  // attaches a combo to document.body) — a duplicate-insert bug would show
  // up as this exact node appearing twice.
  const buttonOccurrences = document.body.children.filter((c) => c === api1.button).length;
  assert.equal(buttonOccurrences, 1, 'idempotent attach must not insert a duplicate button node');

  const button = api1.button;
  const labelEl = button.children.find((c) => c.className === 'tl-combo__label');
  assert.equal(labelEl.textContent, 'Alpha', 'button label reflects current selection at attach time');
  assert.equal(button.disabled, false);

  // Re-populate options at runtime (mirrors populateAccountPicker replacing
  // select.innerHTML) then open the popup — items must reflect the NEW
  // list, not whatever was read at attach() time.
  select.options = [mkOpt('x', 'Xray'), mkOpt('y', 'Yankee'), mkOpt('z', 'Zulu')];
  select.options.forEach((o, i) => { o.index = i; });
  select.selectedIndex = 1;
  select.value = 'y';

  let openedItems = null;
  const originalOpen = window.tlMenu.open;
  window.tlMenu.open = (opts) => { openedItems = opts.items; return originalOpen(opts); };
  button.dispatchEvent({ type: 'click' });
  assert.equal(openedItems.length, 3, 'popup re-reads options each time it opens');
  assert.deepEqual(openedItems.map((i) => i.label), ['Xray', 'Yankee', 'Zulu']);
  assert.equal(openedItems[1].checked, true, 'current option renders checked');
  assert.equal(openedItems[0].checked, false);

  // Selecting an item sets select.value and dispatches a bubbling change event.
  let changeFired = false;
  select.addEventListener('change', () => { changeFired = true; });
  openedItems[2].onSelect();
  assert.equal(select.value, 'z', 'onSelect sets selectEl.value');
  assert.equal(changeFired, true, 'onSelect dispatches a change event existing handlers can see');
  assert.equal(labelEl.textContent, 'Zulu', 'button label updates after selection');
  window.tlMenu.open = originalOpen;

  // Disabled select must block the popup from opening (button.disabled is
  // kept in sync by refresh(), so a real button wouldn't even fire click —
  // this exercises openPopup()'s own guard directly).
  select.disabled = true;
  let openedAgain = false;
  window.tlMenu.open = () => { openedAgain = true; return originalOpen({ items: [] }); };
  button.dispatchEvent({ type: 'click' });
  assert.equal(openedAgain, false, 'openPopup() refuses to open when the select is disabled');
  window.tlMenu.open = originalOpen;

  console.log('tl-combo attach/idempotency/reread/change: ok');
}

console.log('ok');
