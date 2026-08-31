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
const SOURCE = fs.readFileSync(SRC, 'utf8');

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

// Each scenario gets its own vm context: the module keeps a module-wide
// cache of the fetched stylesheet, and that cache must not leak between
// scenarios that deliberately set up different fetch behaviour (missing,
// pending, failing).
function loadModule(extraGlobals) {
  const sandbox = {
    window: {},
    document: { createElement: makeEl, documentElement: {} },
    getComputedStyle: () => ({ getPropertyValue: () => '#111111' }),
    ...extraGlobals,
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(SOURCE, sandbox);
  return sandbox;
}

// --- Scenario 1: the sandbox attribute itself -------------------------------
// No fetch global at all here, matching the original minimal stub — the
// module must not blow up at init just because there's nothing to fetch with.
{
  const sandbox = loadModule({});
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
}

// --- Scenario 2: deps.css injection seam ------------------------------------
// A caller (here, the test) can hand the frame stylesheet text directly,
// bypassing the network entirely — this is what keeps the suite hermetic.
{
  const sandbox = loadModule({});
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, {
    readToken: () => '#111111',
    css: '.injected-marker { color: red; }',
  });
  frame.setContent('<p>hello</p>');

  const iframe = host.children.find((c) => c.tagName === 'IFRAME');
  const srcdoc = iframe.getAttribute('srcdoc');
  assert.ok(
    srcdoc.includes('.injected-marker { color: red; }'),
    'setContent must inline the CSS text supplied via deps.css',
  );
  assert.ok(srcdoc.includes('<p>hello</p>'), 'setContent must inline the given HTML');
}

// --- Scenario 3: setContent before the CSS load settles ---------------------
// No deps.css here, and fetch resolves on demand rather than immediately —
// standing in for the real network gap between createFrame and the
// stylesheet actually arriving. Content must not wait on styling.
{
  let resolveFetch;
  const sandbox = loadModule({
    fetch: () => new Promise((resolve) => { resolveFetch = resolve; }),
  });
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, { readToken: () => '#111111' });
  frame.setContent('<p>early content</p>');

  const iframe = host.children.find((c) => c.tagName === 'IFRAME');
  const firstSrcdoc = iframe.getAttribute('srcdoc');
  assert.ok(
    firstSrcdoc.includes('<p>early content</p>'),
    'content must be injected even before the stylesheet has loaded',
  );
  assert.ok(
    !firstSrcdoc.includes('.late-marker'),
    'the not-yet-loaded stylesheet must not appear before it resolves',
  );

  // Now let the fetch resolve and give its .then chain a turn to run.
  resolveFetch({ ok: true, text: () => Promise.resolve('.late-marker { color: blue; }') });
  await new Promise((r) => setTimeout(r, 10));

  const secondSrcdoc = iframe.getAttribute('srcdoc');
  assert.ok(
    secondSrcdoc.includes('.late-marker { color: blue; }'),
    'once the stylesheet resolves, the most recent content must be re-applied with it',
  );
  assert.ok(
    secondSrcdoc.includes('<p>early content</p>'),
    'the re-applied render must keep the same content',
  );
}

console.log('test_preview_frame: ok');
