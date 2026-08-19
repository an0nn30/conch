// Run: node scripts/tests/test_appearance.mjs
//
// appearance.js is the one place that sets `data-tl-appearance` — the
// attribute tokens-light.css gates on (styles/design-system/tokens-light.css:1)
// and tl-icon.js reads to choose an icon variant (app/ui/tl-icon.js:19-20).
// Before this module existed nothing in the app ever set it.
//
// Each `check` below loads a fresh copy of the module into its own vm
// context (module state — the registered matchMedia listener — is a closure,
// so re-running the IIFE is how a test gets a clean slate rather than
// inheriting whatever the previous check left registered).
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../..');
const APPEARANCE = path.join(ROOT, 'crates/termlab_tauri/frontend/app/core/appearance.js');
const TL_ICON = path.join(ROOT, 'crates/termlab_tauri/frontend/app/ui/tl-icon.js');
const THEME = path.join(ROOT, 'crates/termlab_tauri/frontend/app/features/editor/theme.js');

function loadFresh(files) {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return sandbox;
}

// A minimal `document` whose documentElement supports get/set/removeAttribute
// — the only DOM surface appearance.js's setResolved() touches.
function makeDocStub() {
  const attrs = {};
  const documentElement = {
    getAttribute: (name) => (Object.prototype.hasOwnProperty.call(attrs, name) ? attrs[name] : null),
    setAttribute: (name, value) => { attrs[name] = value; },
    removeAttribute: (name) => { delete attrs[name]; },
  };
  return { documentElement, attrs };
}

// A minimal MediaQueryList: a `matches` flag, add/removeEventListener with
// call counters (so "one listener max" and "removed on mode switch" are
// assertable), and `flip()` to simulate the OS changing preference.
function makeMatchMediaStub(initialMatches) {
  let matches = initialMatches;
  let addCalls = 0;
  let removeCalls = 0;
  const listeners = new Set();
  const mql = {
    get matches() { return matches; },
    addEventListener: (type, fn) => { addCalls += 1; listeners.add(fn); },
    removeEventListener: (type, fn) => { removeCalls += 1; listeners.delete(fn); },
  };
  return {
    matchMediaFn: () => mql,
    flip(nextMatches) { matches = nextMatches; for (const fn of listeners) fn(); },
    get addCalls() { return addCalls; },
    get removeCalls() { return removeCalls; },
    get listenerCount() { return listeners.size; },
  };
}

const results = [];
const check = (name, fn) => results.push({ name, fn });

check('dark removes the attribute', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();
  doc.attrs['data-tl-appearance'] = 'light'; // prove it actively removes, not just skips
  termlabAppearance.apply('dark', { doc });
  assert.strictEqual(doc.attrs['data-tl-appearance'], undefined);
  assert.strictEqual(termlabAppearance.current(), 'dark');
});

check('light sets the attribute to "light"', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();
  termlabAppearance.apply('light', { doc });
  assert.strictEqual(doc.attrs['data-tl-appearance'], 'light');
  assert.strictEqual(termlabAppearance.current(), 'light');
});

check('system resolves via matchMedia(dark) to no attribute', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();
  const mm = makeMatchMediaStub(true); // OS prefers dark
  termlabAppearance.apply('system', { doc, matchMedia: mm.matchMediaFn });
  assert.strictEqual(doc.attrs['data-tl-appearance'], undefined);
  assert.strictEqual(termlabAppearance.current(), 'dark');
});

check('system resolves via matchMedia(light) to the attribute set', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();
  const mm = makeMatchMediaStub(false); // OS prefers light
  termlabAppearance.apply('system', { doc, matchMedia: mm.matchMediaFn });
  assert.strictEqual(doc.attrs['data-tl-appearance'], 'light');
  assert.strictEqual(termlabAppearance.current(), 'light');
});

check('an OS flip re-resolves through the registered change listener', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();
  const mm = makeMatchMediaStub(true); // starts preferring dark
  termlabAppearance.apply('system', { doc, matchMedia: mm.matchMediaFn });
  assert.strictEqual(doc.attrs['data-tl-appearance'], undefined, 'starts dark');

  mm.flip(false); // OS now prefers light
  assert.strictEqual(doc.attrs['data-tl-appearance'], 'light', 'flip to light is picked up live');
  assert.strictEqual(termlabAppearance.current(), 'light');

  mm.flip(true); // and back
  assert.strictEqual(doc.attrs['data-tl-appearance'], undefined, 'flip back to dark is picked up live');
  assert.strictEqual(termlabAppearance.current(), 'dark');
});

check('switching system -> dark unregisters the change listener', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();
  const mm = makeMatchMediaStub(true);
  termlabAppearance.apply('system', { doc, matchMedia: mm.matchMediaFn });
  assert.strictEqual(mm.addCalls, 1, 'system registered exactly one listener');
  assert.strictEqual(mm.removeCalls, 0);

  termlabAppearance.apply('dark', { doc, matchMedia: mm.matchMediaFn });
  assert.strictEqual(mm.removeCalls, 1, 'leaving system tears the listener down');
  assert.strictEqual(mm.listenerCount, 0);

  // Proof the listener is really gone: flipping the (now-orphaned) stub must
  // not move the attribute back off 'dark'.
  mm.flip(false);
  assert.strictEqual(doc.attrs['data-tl-appearance'], undefined, 'still dark: the stale listener never fires');
});

check('re-applying system twice registers only one listener (idempotent)', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();
  const mm = makeMatchMediaStub(true);
  termlabAppearance.apply('system', { doc, matchMedia: mm.matchMediaFn });
  termlabAppearance.apply('system', { doc, matchMedia: mm.matchMediaFn });
  assert.strictEqual(mm.addCalls, 1, 'the second apply(\'system\') did not stack a second listener');
  assert.strictEqual(mm.removeCalls, 0);
  assert.strictEqual(mm.listenerCount, 1);
});

// tl-icon.js reads `document.documentElement.getAttribute('data-tl-appearance')`
// directly (app/ui/tl-icon.js:19-20: isDarkAppearance() is
// `!== 'light'`), and resolve() only swaps in an icon's `_dark` suffix
// variant when isDarkAppearance() is true (app/ui/tl-icon.js:6-17). The
// vendored files back this up: add.svg fills its glyph #6E6E6E (a dark grey,
// legible on a light surface) while add_dark.svg fills #AFB1B3 (a light
// grey, legible on a dark surface) — so "_dark" names the variant *for* dark
// appearance, not the reverse. Loading both modules into one sandbox with a
// live `document` (no injected deps — this exercises appearance.js's default
// `global.document` fallback) proves the two modules agree once this task's
// attribute is actually set.
check('tl-icon picks the light (no-suffix) variant once the attribute is set', () => {
  const sandbox = loadFresh([APPEARANCE, TL_ICON]);
  sandbox.document = {
    documentElement: (() => {
      const attrs = {};
      return {
        getAttribute: (name) => (Object.prototype.hasOwnProperty.call(attrs, name) ? attrs[name] : null),
        setAttribute: (name, value) => { attrs[name] = value; },
        removeAttribute: (name) => { delete attrs[name]; },
      };
    })(),
    createElement: () => ({}),
  };

  sandbox.termlabAppearance.apply('dark'); // default (no attribute) baseline
  const darkIcon = sandbox.tlIcon.create('add', {});
  assert.strictEqual(darkIcon.src, 'vendor/intellij-icons/add_dark.svg', 'dark appearance still picks the _dark variant');

  sandbox.termlabAppearance.apply('light');
  const lightIcon = sandbox.tlIcon.create('add', {});
  assert.strictEqual(lightIcon.src, 'vendor/intellij-icons/add.svg', 'light appearance picks the plain (light-legible) file');
});

// theme.js's isDarkTheme() (app/features/editor/theme.js:87-93) infers dark
// vs light from the *computed* --tl-bg, via `getComputedStyle(document
// .documentElement).getPropertyValue('--tl-bg')` (theme.js:12-13) — read as
// a browser-resolved rgb() string, which is why the stub below returns
// 'rgb(...)' rather than the source hex. isDarkTheme() itself isn't
// exported; it only runs inside buildTheme(), which short-circuits to []
// unless global.CM6 is set (theme.js:19-20), so the stub below fakes just
// enough of CM6's EditorView.theme/tags/HighlightStyle/syntaxHighlighting
// surface to reach it and capture the `dark` flag it computes.
check('isDarkTheme reads --tl-bg luminance: light bg is non-dark, dark bg is dark', () => {
  const sandbox = loadFresh([THEME]);
  sandbox.document = { documentElement: {} };

  let cssVars = {};
  sandbox.getComputedStyle = () => ({
    getPropertyValue: (name) => cssVars[name] || '',
  });

  const captured = {};
  function tagFn(name) {
    const fn = (arg) => ({ tag: name, arg });
    return fn;
  }
  const tagsCache = {};
  sandbox.CM6 = {
    EditorView: {
      theme: (styles, opts) => { captured.dark = opts && opts.dark; return { styles, opts }; },
    },
    tags: new Proxy({}, {
      get(_, prop) {
        if (!(prop in tagsCache)) tagsCache[prop] = tagFn(String(prop));
        return tagsCache[prop];
      },
    }),
    HighlightStyle: { define: (specs) => ({ specs }) },
    syntaxHighlighting: (hs) => ({ hs }),
  };

  // tokens-light.css:70 --tl-base-background: #E3E8EF -> rgb(227, 232, 239),
  // which is what a real getComputedStyle() would report for --tl-bg once
  // the light stylesheet is active.
  cssVars = { '--tl-bg': 'rgb(227, 232, 239)' };
  sandbox.termlabEditorTheme.buildTheme();
  assert.strictEqual(captured.dark, false, 'light --tl-bg (#E3E8EF) reports non-dark');

  // tokens-dark.css:230 --tl-base-background: #21252b -> rgb(33, 37, 43).
  cssVars = { '--tl-bg': 'rgb(33, 37, 43)' };
  sandbox.termlabEditorTheme.buildTheme();
  assert.strictEqual(captured.dark, true, 'dark --tl-bg (#21252b) reports dark');
});

let failed = 0;
for (const { name, fn } of results) {
  try {
    fn();
    console.log(`  ok   ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${error && error.stack ? error.stack : error}`);
  }
}
if (failed) {
  console.log(`appearance: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`appearance: all ${results.length} checks passed`);
}
