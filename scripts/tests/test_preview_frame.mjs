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
    children: [], attrs: {}, style: {}, listeners: {}, parentNode: null,
    setAttribute(k, v) { this.attrs[k] = String(v); },
    getAttribute(k) { return Object.prototype.hasOwnProperty.call(this.attrs, k) ? this.attrs[k] : null; },
    appendChild(c) { c.parentNode = this; this.children.push(c); return c; },
    removeChild(c) { this.children = this.children.filter((x) => x !== c); },
    addEventListener(t, fn) { (this.listeners[t] ||= []).push(fn); },
    // Only the one selector this module ever queries — a general CSS engine
    // would be a lot of machinery for a single attribute check.
    querySelectorAll(sel) {
      if (sel === '[data-src-line]') {
        return this.children.filter((c) => Object.prototype.hasOwnProperty.call(c.attrs, 'data-src-line'));
      }
      return [];
    },
    remove() {},
  };
}

function makeBlock(line) {
  const el = makeEl('div');
  el.setAttribute('data-src-line', String(line));
  el.scrollIntoView = (opts) => { el.scrolledWith = opts; };
  return el;
}

// This harness's addEventListener stub records handlers but never dispatches
// — the established idiom here is to fire the registered handler directly.
function triggerLoad(iframe) {
  (iframe.listeners.load || []).forEach((fn) => fn());
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

// --- Scenario 4: the CSS fetch rejects --------------------------------------
// Degrade, never blank: a network failure must not stop the markdown itself
// from rendering.
{
  const sandbox = loadModule({
    fetch: () => Promise.reject(new Error('network down')),
  });
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, { readToken: () => '#111111' });
  frame.setContent('<p>still renders</p>');
  await new Promise((r) => setTimeout(r, 10));

  const iframe = host.children.find((c) => c.tagName === 'IFRAME');
  const srcdoc = iframe.getAttribute('srcdoc');
  assert.ok(
    srcdoc.includes('<p>still renders</p>'),
    'content must render even when the CSS fetch rejects outright',
  );
}

// --- Scenario 5: the CSS fetch resolves but the response is not ok ---------
// A 404/500 response object is not a rejection — loadSharedCss must treat a
// non-ok response as a failure too, not try to call .text() on it regardless.
{
  const sandbox = loadModule({
    fetch: () => Promise.resolve({
      ok: false,
      status: 404,
      statusText: 'Not Found',
      text: () => Promise.resolve('should never be read'),
    }),
  });
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, { readToken: () => '#111111' });
  frame.setContent('<p>still renders on 404</p>');
  await new Promise((r) => setTimeout(r, 10));

  const iframe = host.children.find((c) => c.tagName === 'IFRAME');
  const srcdoc = iframe.getAttribute('srcdoc');
  assert.ok(
    srcdoc.includes('<p>still renders on 404</p>'),
    'content must render even when the CSS response is not ok',
  );
  assert.ok(
    !srcdoc.includes('should never be read'),
    'a non-ok response body must never be inlined as stylesheet text',
  );
}

// --- Scenario 6: destroy() cancels the pending shared-CSS subscription -----
// If a frame is torn down before the fetch settles, the late .then must not
// go on to setAttribute('srcdoc', ...) on a document nobody is looking at.
{
  let resolveFetch;
  const sandbox = loadModule({
    fetch: () => new Promise((resolve) => { resolveFetch = resolve; }),
  });
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, { readToken: () => '#111111' });
  frame.setContent('<p>before destroy</p>');

  const iframe = host.children.find((c) => c.tagName === 'IFRAME');
  const beforeDestroy = iframe.getAttribute('srcdoc');

  frame.destroy();
  resolveFetch({ ok: true, text: () => Promise.resolve('.after-destroy-marker {}') });
  await new Promise((r) => setTimeout(r, 10));

  const afterResolve = iframe.getAttribute('srcdoc');
  assert.strictEqual(
    afterResolve, beforeDestroy,
    'a destroyed frame must not re-render when its pending CSS load settles afterwards',
  );
  assert.ok(
    !afterResolve.includes('.after-destroy-marker'),
    'CSS that arrives after destroy() must never be applied',
  );
}

// --- Scenario 7/8/9: ANSI syntax tokens snapshotted only when not light ----
// features/editor/theme.js's `syntax` helper ignores the ANSI vars under
// Light and uses the plain app tokens instead; the frame's token list must
// branch the same way so it doesn't paint dark-tuned syntax colours onto a
// light preview. readToken here echoes the token name back so presence and
// value can both be asserted precisely.
{
  const sandbox = loadModule({ termlabAppearance: { current: () => 'dark' } });
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, {
    readToken: (name) => `${name}-value`,
    css: '',
  });
  frame.setContent('<p>x</p>');
  const iframe = host.children.find((c) => c.tagName === 'IFRAME');
  const srcdoc = iframe.getAttribute('srcdoc');
  assert.ok(srcdoc.includes('--green: --green-value;'), 'a dark appearance must snapshot --green');
  assert.ok(srcdoc.includes('--cyan: --cyan-value;'), 'a dark appearance must snapshot --cyan too');
}
{
  const sandbox = loadModule({ termlabAppearance: { current: () => 'light' } });
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, {
    readToken: (name) => `${name}-value`,
    css: '',
  });
  frame.setContent('<p>x</p>');
  const iframe = host.children.find((c) => c.tagName === 'IFRAME');
  const srcdoc = iframe.getAttribute('srcdoc');
  assert.ok(!srcdoc.includes('--green:'), 'a light appearance must omit --green so the CSS fallback applies');
  assert.ok(!srcdoc.includes('--cyan:'), 'a light appearance must omit --cyan too');
  assert.ok(
    srcdoc.includes('--tl-accent: --tl-accent-value;'),
    'the base app tokens must still be snapshotted under light',
  );
}
{
  // No termlabAppearance global at all: must default to not-light, matching
  // theme.js's own isLightAppearance() default.
  const sandbox = loadModule({});
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, {
    readToken: (name) => `${name}-value`,
    css: '',
  });
  frame.setContent('<p>x</p>');
  const iframe = host.children.find((c) => c.tagName === 'IFRAME');
  const srcdoc = iframe.getAttribute('srcdoc');
  assert.ok(
    srcdoc.includes('--green: --green-value;'),
    'a missing termlabAppearance global must default to not-light, same as theme.js',
  );
}

// --- Scenarios 10-12: link interception -------------------------------------
// This is the second-most security-relevant thing in the module: a click on
// a link must never be allowed to navigate the sandboxed frame itself.
{
  const sandbox = loadModule({});
  const host = makeEl('div');
  const clicks = [];
  const frame = sandbox.termlabPreviewFrame.createFrame(host, {
    readToken: () => '#111111',
    css: '',
    onLinkClick: (href) => clicks.push(href),
  });
  const iframe = host.children.find((c) => c.tagName === 'IFRAME');
  const fakeDoc = makeEl('body');
  iframe.contentDocument = fakeDoc;
  triggerLoad(iframe);
  const clickHandler = fakeDoc.listeners.click[0];

  // 10: a direct click on an anchor.
  const anchor = makeEl('a');
  anchor.setAttribute('href', '/docs/target.md');
  let prevented = false;
  clickHandler({ target: anchor, preventDefault() { prevented = true; } });
  assert.ok(prevented, 'clicking an anchor must call preventDefault');
  assert.deepStrictEqual(clicks, ['/docs/target.md'], 'onLinkClick must be called with the href');

  // 11: a click on an element nested inside the anchor must still resolve to it.
  const nestedAnchor = makeEl('a');
  nestedAnchor.setAttribute('href', '/nested.md');
  const span = makeEl('span');
  nestedAnchor.appendChild(span);
  let nestedPrevented = false;
  clickHandler({ target: span, preventDefault() { nestedPrevented = true; } });
  assert.ok(nestedPrevented, 'a click on a descendant of an anchor must still be intercepted');
  assert.deepStrictEqual(
    clicks, ['/docs/target.md', '/nested.md'],
    'the nested click must resolve to the containing anchor\'s href',
  );

  // 12: an anchor with no href must not invoke the callback or throw.
  const hreflessAnchor = makeEl('a');
  let hreflessPrevented = false;
  let threw = false;
  try {
    clickHandler({ target: hreflessAnchor, preventDefault() { hreflessPrevented = true; } });
  } catch (err) {
    threw = true;
  }
  assert.ok(!threw, 'a hrefless anchor must not throw');
  assert.ok(!hreflessPrevented, 'a hrefless anchor must not call preventDefault');
  assert.deepStrictEqual(clicks, ['/docs/target.md', '/nested.md'], 'a hrefless anchor must not invoke onLinkClick');
}

// --- Scenarios 13-16: scrollToLine -------------------------------------------
{
  const sandbox = loadModule({});
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, { readToken: () => '#111111', css: '' });
  const iframe = host.children.find((c) => c.tagName === 'IFRAME');

  // 13: picks the LAST block at or below the target line, not the first or the closest above.
  {
    const b1 = makeBlock(1);
    const b5 = makeBlock(5);
    const b10 = makeBlock(10);
    const fakeDoc = makeEl('body');
    fakeDoc.children = [b1, b5, b10];
    iframe.contentDocument = fakeDoc;

    frame.scrollToLine(7);
    assert.ok(b5.scrolledWith, 'must scroll the last block at or below the target line (5)');
    assert.strictEqual(b1.scrolledWith, undefined, 'must not scroll a block below the chosen one');
    assert.strictEqual(b10.scrolledWith, undefined, 'must not scroll a block past the target line');
  }

  // 14: line 0, with every block strictly above it — nothing should scroll.
  {
    const b1 = makeBlock(1);
    const b5 = makeBlock(5);
    const fakeDoc = makeEl('body');
    fakeDoc.children = [b1, b5];
    iframe.contentDocument = fakeDoc;

    frame.scrollToLine(0);
    assert.strictEqual(b1.scrolledWith, undefined, 'no block qualifies at line 0');
    assert.strictEqual(b5.scrolledWith, undefined, 'no block qualifies at line 0');
  }

  // 15: a line past every block — the last block must be picked.
  {
    const b1 = makeBlock(1);
    const b10 = makeBlock(10);
    const fakeDoc = makeEl('body');
    fakeDoc.children = [b1, b10];
    iframe.contentDocument = fakeDoc;

    frame.scrollToLine(9999);
    assert.ok(b10.scrolledWith, 'a line past every block must still pick the last one');
    assert.strictEqual(b1.scrolledWith, undefined, 'must not settle for an earlier block when a later one qualifies');
  }

  // 16: a document with no mapped blocks at all must not throw.
  {
    const fakeDoc = makeEl('body');
    fakeDoc.children = [];
    iframe.contentDocument = fakeDoc;
    let threw = false;
    try {
      frame.scrollToLine(5);
    } catch (err) {
      threw = true;
    }
    assert.ok(!threw, 'scrollToLine must not throw when there are no mapped blocks');
  }
}

// --- Scenario: consecutive renders are never byte-identical -----------------
//
// The parent hangs image resolution off the frame's `load` event, so it needs
// the frame to actually re-navigate. Whether a webview re-navigates when
// `srcdoc` is assigned the value it already holds is engine-dependent, and the
// concrete failure is a Save As between two directories holding same-named
// files: identical bytes, identical palette, identical stylesheet — and a
// stale set of already-resolved images if no load fires. A monotonic render
// token removes the question.
{
  const sandbox = loadModule({});
  const host = makeEl('div');
  const frame = sandbox.termlabPreviewFrame.createFrame(host, {
    readToken: () => '#111111',
    css: '.md-preview-body { color: red; }',
  });
  const iframe = frame.element;

  frame.setContent('<p data-src-line="0">same</p>');
  const first = iframe.getAttribute('srcdoc');
  frame.setContent('<p data-src-line="0">same</p>');
  const second = iframe.getAttribute('srcdoc');

  assert.notStrictEqual(
    first, second,
    'identical content must still produce a different srcdoc, or the frame may never re-navigate',
  );
  // Only the token differs: the rendered body is untouched.
  assert.match(first, /<!-- termlab-render 1 -->/, 'the token is present and monotonic');
  assert.match(second, /<!-- termlab-render 2 -->/);
  assert.strictEqual(
    first.replace(/<!-- termlab-render \d+ -->/, ''),
    second.replace(/<!-- termlab-render \d+ -->/, ''),
    'and nothing else about the document changed',
  );
  // Ahead of the doctype a comment would put the document into quirks mode.
  assert.ok(
    second.indexOf('<!doctype html>') < second.indexOf('<!-- termlab-render'),
    'the token must sit after the doctype, not before it',
  );
}

console.log('test_preview_frame: ok');
