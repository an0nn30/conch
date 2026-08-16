// Run: node scripts/tests/test_tl_dialog.mjs
//
// No jsdom available in this repo, so — like test_tl_icon.mjs — this stubs
// just enough of `window`/`document` for tl-dialog.js to load, then
// exercises the pure logic called out in the task-1 brief: z-index by
// depth, the focusable-candidate filter, and close() popping only the top
// stack entry. It does not attempt to drive a real layout engine (focus
// trapping, aria-hidden sweep of document.body, requestAnimationFrame
// focus, etc. all need a real DOM and are exercised by hand in the app,
// per the brief's Step 5 manual-verify instructions).
import assert from 'node:assert';

// --- minimal element stub -------------------------------------------------
function makeElement(tag) {
  const attrs = new Map();
  const el = {
    tagName: String(tag || 'div').toUpperCase(),
    className: '',
    children: [],
    style: {},
    disabled: false,
    tabIndex: 0,
    isConnected: false,
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
      return child;
    },
    setAttribute(name, value) { attrs.set(name, String(value)); },
    getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
    removeAttribute(name) { attrs.delete(name); },
    hasAttribute(name) { return attrs.has(name); },
    addEventListener() {},
    removeEventListener() {},
    querySelectorAll() { return []; },
    focus() {},
    classList: { add() {}, remove() {}, contains() { return false; } },
  };
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

const { readFileSync } = await import('node:fs');
eval(readFileSync('crates/termlab_tauri/frontend/app/ui/tl-dialog.js', 'utf8'));

// --- z-index by depth ------------------------------------------------------
assert.equal(window.tlDialog._zIndexForDepth(0), 3000);
assert.equal(window.tlDialog._zIndexForDepth(1), 3010);
assert.equal(window.tlDialog._zIndexForDepth(2), 3020);

// --- focusable-candidate filter ---------------------------------------------
const isFocusable = window.tlDialog._isFocusableCandidate;

const link = makeElement('a');
link.setAttribute('href', '#');
assert.equal(isFocusable(link), true, 'a[href] should be focusable');

const button = makeElement('button');
assert.equal(isFocusable(button), true, 'button should be focusable');

const input = makeElement('input');
assert.equal(isFocusable(input), true, 'input should be focusable');

const select = makeElement('select');
assert.equal(isFocusable(select), true, 'select should be focusable');

const textarea = makeElement('textarea');
assert.equal(isFocusable(textarea), true, 'textarea should be focusable');

const tabbableDiv = makeElement('div');
tabbableDiv.setAttribute('tabindex', '0');
assert.equal(isFocusable(tabbableDiv), true, 'div[tabindex=0] should be focusable');

const disabledButton = makeElement('button');
disabledButton.disabled = true;
assert.equal(isFocusable(disabledButton), false, 'disabled button should be excluded');

const untabbableDiv = makeElement('div');
untabbableDiv.setAttribute('tabindex', '-1');
assert.equal(isFocusable(untabbableDiv), false, 'tabindex=-1 should be excluded');

const untabbableButton = makeElement('button');
untabbableButton.setAttribute('tabindex', '-1');
assert.equal(isFocusable(untabbableButton), false, 'button with tabindex=-1 should be excluded');

const plainDiv = makeElement('div');
assert.equal(isFocusable(plainDiv), false, 'plain div outside the standard set should be excluded');

const linkWithoutHref = makeElement('a');
assert.equal(isFocusable(linkWithoutHref), false, 'a without href should be excluded');

// --- close() pops only the top stack entry ---------------------------------
// Stub out the router so registerEscape() takes the no-router branch (its
// console.warn is expected noise here — irrelevant to the assertions below).
const originalWarn = console.warn;
console.warn = () => {};

const first = window.tlDialog.open({ title: 'First' });
const second = window.tlDialog.open({ title: 'Second' });
const third = window.tlDialog.open({ title: 'Third' });

console.warn = originalWarn;

assert.equal(window.tlDialog.count(), 3);
third.close();
assert.equal(window.tlDialog.count(), 2, 'close() should pop exactly one entry');
assert.equal(document.body.children.includes(third.el.parentNode), false, 'the closed overlay should be detached');
// The remaining two dialogs' overlays are still attached to document.body.
assert.equal(document.body.children.length, 2, 'lower dialogs must be left in the stack/DOM');

second.close();
assert.equal(window.tlDialog.count(), 1);
assert.equal(document.body.children.length, 1);

first.close();
assert.equal(window.tlDialog.count(), 0);
assert.equal(document.body.children.length, 0);

// --- Escape priority: a tl-menu popup opened while a tl-dialog is topmost
// must win Escape over the dialog (design-system-phase-5a final review,
// task 1 — before the fix, tl-dialog's fixed priority 225 beat tl-menu's
// old fixed priority 220, so Escape closed the dialog out from under an
// open popup and stranded it: appended to document.body as a SIBLING of
// the dialog overlay, it survives the overlay's removal). The tests above
// all run with `registerEscape()`'s no-router branch (see the console.warn
// stub above) precisely because no window.termlabKeyboardRouter was ever
// set — this block provides a minimal one so the priority values in
// app/ui/tl-dialog.js and app/ui/tl-menu.js actually get exercised, not
// just asserted about in isolation.
{
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
      // Mirrors core/keyboard-router.js's dispatch(): highest priority
      // first, ties broken by registration order, stop at the first
      // handler whose onKeyDown returns true. This ordering is exactly the
      // mechanism the production bug exploited, so the stub has to
      // reproduce it rather than just calling both handlers directly.
      dispatchEscape() {
        const sorted = Array.from(handlers.values()).sort((a, b) => {
          if (a.priority !== b.priority) return b.priority - a.priority;
          return a.order - b.order;
        });
        for (const entry of sorted) {
          if (entry.isActive && !entry.isActive()) continue;
          if (!entry.onKeyDown) continue;
          if (entry.onKeyDown({ key: 'Escape' }) === true) return true;
        }
        return false;
      },
    };
  }

  // tl-menu.js needs a little more DOM surface than tl-dialog.js does
  // (remove(), contains(), querySelector(), getBoundingClientRect(), a
  // classList.contains() that actually tracks state) plus a synchronous
  // requestAnimationFrame — swap in a richer element factory just for this
  // block rather than growing the shared makeElement() above and risking
  // the earlier no-router assertions.
  function makeRichElement(tag) {
    const attrs = new Map();
    const listeners = new Map();
    const el = {
      tagName: String(tag || 'div').toUpperCase(),
      children: [],
      style: {},
      disabled: false,
      tabIndex: 0,
      isConnected: false,
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
      querySelectorAll(sel) {
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
      querySelector(sel) {
        if (sel === '.tl-menu__item:not(.is-disabled)') {
          return this.querySelectorAll('.tl-menu__item').find((c) => !c.classList.contains('is-disabled')) || null;
        }
        return null;
      },
      contains(node) {
        let n = node;
        while (n) { if (n === this) return true; n = n.parentNode; }
        return false;
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
    Object.defineProperty(el, 'className', {
      get() { return Array.from(el.classList._set).join(' '); },
      set(v) { el.classList._set = new Set(String(v).split(' ').filter(Boolean)); },
    });
    return el;
  }

  document.createElement = makeRichElement;
  // document.body itself was created back at the top of this file via the
  // plainer makeElement(), whose removeChild() doesn't reset the removed
  // child's isConnected (never needed to, since nothing above checks it
  // post-removal) — swap it for a rich one so menu.isConnected actually
  // flips to false once tl-menu.js's close() calls activeMenu.remove().
  const originalBody = document.body;
  document.body = makeRichElement('body');
  document.body.isConnected = true;
  window.innerWidth = 1024;
  window.innerHeight = 768;
  window.requestAnimationFrame = (fn) => fn();
  globalThis.requestAnimationFrame = window.requestAnimationFrame;

  window.termlabKeyboardRouter = makeRouterStub();

  const { readFileSync: readFileSync2 } = await import('node:fs');
  eval(readFileSync2('crates/termlab_tauri/frontend/app/ui/tl-menu.js', 'utf8'));

  const dialogHandle = window.tlDialog.open({ title: 'Escape priority test' });
  assert.equal(window.tlDialog.count(), 1, 'dialog should be open');

  // Simulates what tl-combo.js's openPopup() does: open a tl-menu popup
  // without an explicit routerPriority, while this dialog is topmost.
  const menu = window.tlMenu.open({
    x: 0, y: 0,
    items: [{ label: 'one', onSelect() {} }],
  });
  assert.equal(menu.isConnected, true, 'popup should be open');

  const consumed1 = window.termlabKeyboardRouter.dispatchEscape();
  assert.equal(consumed1, true, 'Escape should be consumed by someone');
  assert.equal(menu.isConnected, false, 'first Escape must close the popup, not the dialog');
  assert.equal(window.tlDialog.count(), 1, 'first Escape must leave the dialog open');

  const consumed2 = window.termlabKeyboardRouter.dispatchEscape();
  assert.equal(consumed2, true, 'second Escape should be consumed by the dialog');
  assert.equal(window.tlDialog.count(), 0, 'second Escape must close the dialog');

  document.body = originalBody;
  console.log('tl-dialog/tl-menu Escape-priority: ok');
}

console.log('ok');
