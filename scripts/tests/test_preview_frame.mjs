// Run: node scripts/tests/test_preview_frame.mjs
//
// The sandbox attribute is the whole security design, so it is asserted as a
// literal rather than checked loosely: `allow-same-origin` lets the PARENT
// drive the frame's DOM (scroll sync, link interception) while the ABSENCE of
// allow-scripts means nothing inside the frame can execute. Adding
// allow-scripts would silently undo the feature's entire threat model, so this
// test exists to make that change fail loudly.
//
// No jsdom here: the frame's own contract is attribute-level, and the minimal
// element factory is the established idiom for this kind of test.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const SRC = path.join(ROOT, 'app/features/editor/preview/preview-frame.js');

function makeEl(tag) {
  return {
    tagName: String(tag).toUpperCase(),
    children: [], attrs: {}, style: {}, listeners: {},
    setAttribute(k, v) { this.attrs[k] = String(v); },
    getAttribute(k) { return Object.prototype.hasOwnProperty.call(this.attrs, k) ? this.attrs[k] : null; },
    appendChild(c) { this.children.push(c); return c; },
    removeChild(c) { this.children = this.children.filter((x) => x !== c); },
    addEventListener(t, fn) { (this.listeners[t] ||= []).push(fn); },
    remove() {},
  };
}

const sandbox = {
  window: {},
  document: { createElement: makeEl, documentElement: {} },
  getComputedStyle: () => ({ getPropertyValue: () => '#111111' }),
};
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(SRC, 'utf8'), sandbox);

const host = makeEl('div');
const frame = sandbox.termlabPreviewFrame.createFrame(host, { readToken: () => '#111111' });
assert.ok(frame, 'createFrame must return a handle');

const iframe = host.children.find((c) => c.tagName === 'IFRAME');
assert.ok(iframe, 'the frame must mount an iframe');

const sandboxAttr = iframe.getAttribute('sandbox');
assert.strictEqual(
  sandboxAttr, 'allow-same-origin',
  'sandbox must be exactly "allow-same-origin" — adding allow-scripts defeats the design',
);
assert.ok(
  !/allow-scripts/.test(sandboxAttr),
  'allow-scripts must never be present on the preview frame',
);

console.log('test_preview_frame: ok');
