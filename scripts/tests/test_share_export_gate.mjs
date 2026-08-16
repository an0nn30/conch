// Run: node scripts/tests/test_share_export_gate.mjs
//
// No jsdom in this repo (see test_tl_dialog.mjs's precedent) — stub just
// enough of `window`/`document` for app/panels/ssh-panel.js to load (its
// only module-load-time external dependency is `window.utils.esc/attr`;
// everything else the file touches at top level either defaults to `{}` via
// `exports.termlabX || {}` or is only referenced inside functions, called
// lazily), then exercise window.termlabShareUi.canExport — the pure gate
// for the export dialog's Export button (task-4 brief, Step 3/4): disabled
// until at least one item is selected and the two passwords are non-empty
// and equal.
import assert from 'node:assert';

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
    querySelector() { return null; },
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

// ssh-panel.js's only module-load-time dependency outside `window` itself.
window.utils = {
  esc: (s) => String(s),
  attr: (s) => String(s),
};

const { readFileSync } = await import('node:fs');
eval(readFileSync('crates/termlab_tauri/frontend/app/panels/ssh-panel.js', 'utf8'));

assert.ok(window.termlabShareUi, 'ssh-panel.js should export window.termlabShareUi');
const { canExport } = window.termlabShareUi;
assert.equal(typeof canExport, 'function', 'canExport should be a function');

// Nothing selected.
assert.equal(
  canExport({ selectedCount: 0, password: 'hunter2', confirm: 'hunter2' }),
  false,
  'no selection must disable Export even with matching passwords'
);

// Selection, but empty passwords.
assert.equal(
  canExport({ selectedCount: 1, password: '', confirm: '' }),
  false,
  'empty passwords must disable Export'
);
assert.equal(
  canExport({ selectedCount: 1, password: 'hunter2', confirm: '' }),
  false,
  'an empty confirm field must disable Export'
);

// Selection, but mismatched passwords.
assert.equal(
  canExport({ selectedCount: 1, password: 'hunter2', confirm: 'hunter3' }),
  false,
  'mismatched passwords must disable Export'
);

// Selection with two equal, non-empty passwords.
assert.equal(
  canExport({ selectedCount: 1, password: 'hunter2', confirm: 'hunter2' }),
  true,
  'a selection with matching non-empty passwords must enable Export'
);
assert.equal(
  canExport({ selectedCount: 5, password: 'correct horse battery staple', confirm: 'correct horse battery staple' }),
  true,
  'multi-item selection with matching non-empty passwords must enable Export'
);

console.log('ok');
