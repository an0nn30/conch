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

console.log('ok');
