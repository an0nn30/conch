// Run: node scripts/tests/test_appearance.mjs
//
// appearance.js is the one place that sets `data-tl-appearance` — the
// attribute tokens-light.css gates on (styles/design-system/tokens-light.css:1)
// and tl-icon.js reads to choose an icon variant (app/ui/tl-icon.js:19-20).
// Before this module existed nothing in the app ever set it.
//
// It is also the one place that ANNOUNCES a resolved appearance change, as a
// `tl-appearance-changed` CustomEvent on `document`. Everything that bakes a
// colour or a file path at build time — icon `src` variants, the editor's
// CodeMirror theme — re-resolves off that event, so the second half of this
// suite covers the announcement and its two consumers.
//
// Each `check` below loads a fresh copy of the module into its own vm
// context (module state — the registered matchMedia listener, the delegated
// icon listener — is a closure, so re-running the IIFE is how a test gets a
// clean slate rather than inheriting whatever the previous check left
// registered).
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { EDITOR_THEME_DARK_AT_BASE } from './fixtures/editor-theme-dark-base.mjs';

const ROOT = path.resolve(import.meta.dirname, '../..');
const FRONTEND = path.join(ROOT, 'crates/termlab_tauri/frontend');
const APPEARANCE = path.join(FRONTEND, 'app/core/appearance.js');
const APPEARANCE_SYNC = path.join(FRONTEND, 'app/core/appearance-sync.js');
const CONFIG_SERVICE = path.join(FRONTEND, 'app/core/config-service.js');
const CONFIG_RUNTIME = path.join(FRONTEND, 'app/config-runtime.js');
const TL_ICON = path.join(FRONTEND, 'app/ui/tl-icon.js');
const THEME = path.join(FRONTEND, 'app/features/editor/theme.js');

const CHANGED_EVENT = 'tl-appearance-changed';

// `seed` runs against the fresh context BEFORE any module file does, which is
// how a test gives tl-icon.js a `document` to register its delegated listener
// on — in a real page a classic script always has one.
function loadFresh(files, seed) {
  const sandbox = { console };
  sandbox.window = sandbox;
  // appearance.js dispatches through `global.CustomEvent`; the page has one.
  sandbox.CustomEvent = class CustomEventStub {
    constructor(type, init) {
      this.type = type;
      this.detail = (init && init.detail) || null;
    }
  };
  vm.createContext(sandbox);
  if (typeof seed === 'function') seed(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  return sandbox;
}

// A minimal `document`: a documentElement supporting get/set/removeAttribute
// (the only DOM surface appearance.js's setResolved() touches) plus the
// EventTarget trio the notification rides on.
function makeDocStub() {
  const attrs = {};
  const listeners = new Map();
  const documentElement = {
    getAttribute: (name) => (Object.prototype.hasOwnProperty.call(attrs, name) ? attrs[name] : null),
    setAttribute: (name, value) => { attrs[name] = value; },
    removeAttribute: (name) => { delete attrs[name]; },
  };
  const dispatched = [];
  return {
    documentElement,
    attrs,
    dispatched,
    addEventListener: (type, fn) => {
      if (!listeners.has(type)) listeners.set(type, new Set());
      listeners.get(type).add(fn);
    },
    removeEventListener: (type, fn) => {
      const set = listeners.get(type);
      if (set) set.delete(fn);
    },
    dispatchEvent: (event) => {
      dispatched.push(event);
      const set = listeners.get(event && event.type);
      if (set) for (const fn of Array.from(set)) fn(event);
      return true;
    },
    listenerCount: (type) => (listeners.get(type) ? listeners.get(type).size : 0),
  };
}

// An <img>-shaped stub. `src` is an accessor backed by the attribute map,
// which is how a real element behaves and is what lets the test tell
// `img.src = x` (create) and `img.setAttribute('src', x)` (refreshAll) apart
// from each other's effects: there are none, they are one value.
function makeElementStub(tag) {
  const attrs = {};
  const el = {
    tagName: String(tag).toUpperCase(),
    getAttribute: (name) => (Object.prototype.hasOwnProperty.call(attrs, name) ? attrs[name] : null),
    setAttribute: (name, value) => { attrs[name] = String(value); },
    removeAttribute: (name) => { delete attrs[name]; },
  };
  Object.defineProperty(el, 'src', {
    get() { return el.getAttribute('src'); },
    set(value) { el.setAttribute('src', value); },
  });
  return el;
}

// makeDocStub plus the createElement/querySelectorAll pair tl-icon.js needs.
// Everything createElement hands out stays in `created`, so querySelectorAll
// can answer over the whole document the way a real one would.
function makeIconDocStub() {
  const doc = makeDocStub();
  const created = [];
  doc.created = created;
  doc.createElement = (tag) => {
    const el = makeElementStub(tag);
    created.push(el);
    return el;
  };
  doc.querySelectorAll = (selector) => {
    assert.strictEqual(selector, 'img[data-tl-icon]', 'refreshAll selects by the stamped name attribute');
    return created.filter((el) => el.tagName === 'IMG' && el.getAttribute('data-tl-icon') !== null);
  };
  return doc;
}

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

// ---------------------------------------------------------------- attribute

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

// ------------------------------------------------------------ mode fallback
//
// The documented contract (appearance.js:8-12) is that the mode string is
// case-insensitive and that anything unrecognised — including a missing value
// — resolves to 'dark', matching the Rust-side AppearanceMode default. Both
// halves were correct by inspection but unasserted.

check('the mode string is matched case-insensitively', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();

  termlabAppearance.apply('LIGHT', { doc });
  assert.strictEqual(termlabAppearance.current(), 'light', 'uppercase LIGHT is light');
  assert.strictEqual(doc.attrs['data-tl-appearance'], 'light');

  termlabAppearance.apply('Dark', { doc });
  assert.strictEqual(termlabAppearance.current(), 'dark', 'mixed-case Dark is dark');

  const mm = makeMatchMediaStub(false); // OS prefers light
  termlabAppearance.apply('SyStEm', { doc, matchMedia: mm.matchMediaFn });
  assert.strictEqual(termlabAppearance.current(), 'light', 'mixed-case System still resolves via matchMedia');
  assert.strictEqual(mm.addCalls, 1, 'and still takes the system path, listener and all');
});

check('a falsy or unknown mode falls back to dark', () => {
  for (const mode of [undefined, null, '', 0, false, 'sepia']) {
    const { termlabAppearance } = loadFresh([APPEARANCE]);
    const doc = makeDocStub();
    doc.attrs['data-tl-appearance'] = 'light'; // start light so the fallback has to act
    termlabAppearance.apply(mode, { doc });
    assert.strictEqual(termlabAppearance.current(), 'dark', `${String(mode)} resolves dark`);
    assert.strictEqual(doc.attrs['data-tl-appearance'], undefined, `${String(mode)} clears the attribute`);
  }
});

check('system with no matchMedia available resolves dark, not light', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();
  doc.attrs['data-tl-appearance'] = 'light';
  // No matchMedia in deps and none on the sandbox global: an unresolvable OS
  // preference must follow the same unresolvable->dark convention as every
  // other fallback in the app, not silently pick light.
  termlabAppearance.apply('system', { doc });
  assert.strictEqual(termlabAppearance.current(), 'dark');
  assert.strictEqual(doc.attrs['data-tl-appearance'], undefined);
});

// ------------------------------------------------------------ notification

check('a resolved change dispatches tl-appearance-changed; a no-op change does not', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();

  termlabAppearance.apply('dark', { doc });
  assert.strictEqual(doc.dispatched.length, 0, 'dark over the dark default resolves to no change, so no event');

  termlabAppearance.apply('light', { doc });
  assert.strictEqual(doc.dispatched.length, 1, 'dark -> light announces once');
  assert.strictEqual(doc.dispatched[0].type, CHANGED_EVENT);
  // Compared field-wise, not deepStrictEqual: the detail object is built
  // inside the vm context, so it does not share this realm's Object.prototype.
  assert.strictEqual(doc.dispatched[0].detail.resolved, 'light');

  termlabAppearance.apply('light', { doc });
  assert.strictEqual(doc.dispatched.length, 1, 're-applying the same mode announces nothing');

  termlabAppearance.apply('dark', { doc });
  assert.strictEqual(doc.dispatched.length, 2, 'light -> dark announces');
  assert.strictEqual(doc.dispatched[1].detail.resolved, 'dark');
});

check('an OS flip in system mode announces too', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  const doc = makeDocStub();
  const mm = makeMatchMediaStub(true); // OS prefers dark
  termlabAppearance.apply('system', { doc, matchMedia: mm.matchMediaFn });
  assert.strictEqual(doc.dispatched.length, 0, 'resolving to the current value announces nothing');

  mm.flip(false);
  assert.strictEqual(doc.dispatched.length, 1, 'the OS flip is the only signal on this path — it must announce');
  assert.strictEqual(doc.dispatched[0].detail.resolved, 'light');

  mm.flip(true);
  assert.strictEqual(doc.dispatched.length, 2);
  assert.strictEqual(doc.dispatched[1].detail.resolved, 'dark');
});

check('the event name is exported so consumers need not hardcode it', () => {
  const { termlabAppearance } = loadFresh([APPEARANCE]);
  assert.strictEqual(termlabAppearance.CHANGED_EVENT, CHANGED_EVENT);
});

// -------------------------------------------------------------------- icons
//
// tl-icon.js reads `document.documentElement.getAttribute('data-tl-appearance')`
// directly (app/ui/tl-icon.js: isDarkAppearance() is `!== 'light'`), and
// resolve() only swaps in an icon's `_dark` suffix variant when
// isDarkAppearance() is true. The vendored files back this up: add.svg fills
// its glyph #6E6E6E (a dark grey, legible on a light surface) while
// add_dark.svg fills #AFB1B3 (a light grey, legible on a dark surface) — so
// "_dark" names the variant *for* dark appearance, not the reverse. Loading
// both modules into one sandbox with a live `document` (no injected deps —
// this exercises appearance.js's default `global.document` fallback) proves
// the two modules agree once the attribute is actually set.

check('tl-icon picks the light (no-suffix) variant once the attribute is set', () => {
  const doc = makeIconDocStub();
  const sandbox = loadFresh([APPEARANCE, TL_ICON], (s) => { s.document = doc; });

  sandbox.termlabAppearance.apply('dark'); // default (no attribute) baseline
  const darkIcon = sandbox.tlIcon.create('add', {});
  assert.strictEqual(darkIcon.src, 'vendor/intellij-icons/add_dark.svg', 'dark appearance still picks the _dark variant');

  sandbox.termlabAppearance.apply('light');
  const lightIcon = sandbox.tlIcon.create('add', {});
  assert.strictEqual(lightIcon.src, 'vendor/intellij-icons/add.svg', 'light appearance picks the plain (light-legible) file');
});

// The F1 regression. Icons built BEFORE a flip used to keep their baked
// `src` for the life of the element — most visibly the tool-window rail,
// which is built once at startup and never rebuilt, leaving #AFB1B3 glyphs
// on a light surface.
check('an appearance flip re-resolves every already-created icon, and back', () => {
  const doc = makeIconDocStub();
  const sandbox = loadFresh([APPEARANCE, TL_ICON], (s) => { s.document = doc; });

  sandbox.termlabAppearance.apply('dark', { doc });

  // Built under dark, as the rail is at startup. `terminal` has no _dark
  // variant, so it is the control: it must not move in either direction.
  const add = sandbox.tlIcon.create('add', { size: 16 });
  const gear = sandbox.tlIcon.create('gear', { size: 16 });
  const hide = sandbox.tlIcon.create('hideToolWindow', { size: 16 });
  const term = sandbox.tlIcon.create('terminal', { size: 16 });

  assert.strictEqual(add.getAttribute('data-tl-icon'), 'add', 'create stamps the logical name');
  assert.strictEqual(add.src, 'vendor/intellij-icons/add_dark.svg');
  assert.strictEqual(gear.src, 'vendor/intellij-icons/gear_dark.svg');
  assert.strictEqual(hide.src, 'vendor/intellij-icons/hideToolWindow_dark.svg');
  assert.strictEqual(term.src, 'vendor/intellij-icons/terminal.svg');

  // The flip a settings save (or an OS change) produces.
  sandbox.termlabAppearance.apply('light', { doc });
  assert.strictEqual(add.src, 'vendor/intellij-icons/add.svg', 'add re-resolved to the light variant');
  assert.strictEqual(gear.src, 'vendor/intellij-icons/gear.svg', 'gear re-resolved to the light variant');
  assert.strictEqual(hide.src, 'vendor/intellij-icons/hideToolWindow.svg', 'hideToolWindow re-resolved to the light variant');
  assert.strictEqual(term.src, 'vendor/intellij-icons/terminal.svg', 'a variant-less icon is left alone');

  sandbox.termlabAppearance.apply('dark', { doc });
  assert.strictEqual(add.src, 'vendor/intellij-icons/add_dark.svg', 'and back');
  assert.strictEqual(gear.src, 'vendor/intellij-icons/gear_dark.svg');
  assert.strictEqual(hide.src, 'vendor/intellij-icons/hideToolWindow_dark.svg');
  assert.strictEqual(term.src, 'vendor/intellij-icons/terminal.svg');

  // Asserted last so a broken re-resolve reds on the stale glyph itself
  // rather than on this bookkeeping detail.
  assert.strictEqual(
    doc.listenerCount(CHANGED_EVENT), 1,
    'four icons, still one delegated listener — no per-icon subscription leak',
  );
});

check('refreshAll re-resolves on demand and reports how many icons it walked', () => {
  const doc = makeIconDocStub();
  const sandbox = loadFresh([APPEARANCE, TL_ICON], (s) => { s.document = doc; });
  sandbox.termlabAppearance.apply('dark', { doc });
  const icons = ['add', 'edit', 'remove'].map((n) => sandbox.tlIcon.create(n, {}));

  // An unstamped <img> (nothing in the app makes one, but the walk must not
  // claim it) and a stamped non-img are both outside the selector.
  doc.createElement('img');
  assert.strictEqual(sandbox.tlIcon.refreshAll(doc), 3, 'only the stamped icons are walked');

  doc.documentElement.setAttribute('data-tl-appearance', 'light');
  sandbox.tlIcon.refreshAll(doc);
  for (const img of icons) {
    assert.ok(!img.src.endsWith('_dark.svg'), `${img.getAttribute('data-tl-icon')} follows the attribute`);
  }
});

// ------------------------------------------------------------ editor theme
//
// theme.js's colours are read from the *computed* custom properties, via
// `getComputedStyle(document.documentElement).getPropertyValue(...)` — a
// browser-resolved rgb() string, which is why the stubs below use 'rgb(...)'
// rather than the source hex. Nothing in the module is exported except
// buildTheme(), which short-circuits to [] unless global.CM6 is set, so the
// stub fakes just enough of CM6's EditorView.theme / tags / HighlightStyle /
// syntaxHighlighting surface to capture what it builds.

// A dark install as it actually runs on this branch: the terminal palette
// (--tl-terminal-bg and the --red/--green/... ANSI vars, written as inline
// root styles by config-service.js) is dark, and so are the app tokens.
const DARK_VARS = {
  '--tl-terminal-bg': 'rgb(7, 10, 14)',
  '--tl-bg': 'rgb(33, 37, 43)',
  '--tl-fg': 'rgb(191, 198, 206)',
  '--tl-fg-muted': 'rgb(128, 134, 142)',
  '--tl-accent': 'rgb(53, 116, 240)',
  '--tl-border': 'rgb(45, 49, 55)',
  '--tl-selection-bg': 'rgb(33, 66, 131)',
  '--tl-row-hover': 'rgb(43, 45, 48)',
  '--tl-danger': 'rgb(199, 84, 80)',
  '--green': 'rgb(152, 195, 121)',
  '--yellow': 'rgb(229, 192, 123)',
  '--blue': 'rgb(97, 175, 239)',
  '--cyan': 'rgb(86, 182, 194)',
};

// A LIGHT install on this branch: app tokens flip (tokens-light.css), the
// terminal palette does NOT — the branch keeps the terminal dark on purpose,
// so --tl-terminal-bg and the ANSI vars stay exactly as above. That
// disagreement is the whole of F2.
const LIGHT_VARS = {
  ...DARK_VARS,
  '--tl-bg': 'rgb(227, 232, 239)',
  '--tl-fg': 'rgb(31, 41, 51)',
  '--tl-fg-muted': 'rgb(122, 130, 140)',
  '--tl-border': 'rgb(197, 205, 214)',
  '--tl-selection-bg': 'rgb(190, 214, 245)',
  '--tl-row-hover': 'rgb(214, 221, 230)',
};

function tagFn(name) {
  const fn = (arg) => ({ tag: name, arg });
  fn.tagName = name;
  return fn;
}

// Build the theme under a given appearance and return both the live capture
// and its deterministic serialization (tag functions replaced by their names,
// which is the only thing in the structure JSON cannot represent).
function captureEditorTheme(cssVars, appearanceMode) {
  const doc = makeDocStub();
  const sandbox = loadFresh([APPEARANCE, THEME], (s) => { s.document = doc; });
  sandbox.getComputedStyle = () => ({ getPropertyValue: (name) => cssVars[name] || '' });
  sandbox.termlabAppearance.apply(appearanceMode, { doc });

  const out = {};
  const tagsCache = {};
  sandbox.CM6 = {
    EditorView: { theme: (styles, opts) => { out.styles = styles; out.opts = opts; return {}; } },
    tags: new Proxy({}, {
      get(_, prop) {
        if (!(prop in tagsCache)) tagsCache[prop] = tagFn(String(prop));
        return tagsCache[prop];
      },
    }),
    HighlightStyle: { define: (specs) => { out.specs = specs; return {}; } },
    syntaxHighlighting: (hs) => hs,
  };
  sandbox.termlabEditorTheme.buildTheme();
  out.json = JSON.stringify(out, (k, v) => (typeof v === 'function' ? (v.tagName || '[fn]') : v), 2);
  return out;
}

// Find the colour a highlight spec assigns to a named tag. A tag is either a
// bare stub function (`t.keyword`) or the object a modifier produced
// (`t.definition(t.variableName)` -> `{ tag: 'definition', arg }`).
function specColor(capture, tagName) {
  for (const spec of capture.specs) {
    const tags = Array.isArray(spec.tag) ? spec.tag : [spec.tag];
    for (const tag of tags) {
      if (typeof tag === 'function' && tag.tagName === tagName) return spec.color;
      if (tag && typeof tag === 'object' && tag.tag === tagName) return spec.color;
    }
  }
  throw new Error(`no highlight spec for tag ${tagName}`);
}

check('isDarkTheme reads --tl-bg luminance: light bg is non-dark, dark bg is dark', () => {
  const sandbox = loadFresh([THEME], (s) => { s.document = { documentElement: {} }; });

  let cssVars = {};
  sandbox.getComputedStyle = () => ({ getPropertyValue: (name) => cssVars[name] || '' });

  const captured = {};
  const tagsCache = {};
  sandbox.CM6 = {
    EditorView: { theme: (styles, opts) => { captured.dark = opts && opts.dark; return { styles, opts }; } },
    tags: new Proxy({}, {
      get(_, prop) {
        if (!(prop in tagsCache)) tagsCache[prop] = tagFn(String(prop));
        return tagsCache[prop];
      },
    }),
    HighlightStyle: { define: (specs) => ({ specs }) },
    syntaxHighlighting: (hs) => ({ hs }),
  };

  // No termlabAppearance in this sandbox at all: isDarkTheme() is what
  // answers, which is the pre-appearance behaviour and the fallback when the
  // module has not loaded.
  cssVars = { '--tl-bg': 'rgb(227, 232, 239)' };
  sandbox.termlabEditorTheme.buildTheme();
  assert.strictEqual(captured.dark, false, 'light --tl-bg (#E3E8EF) reports non-dark');

  cssVars = { '--tl-bg': 'rgb(33, 37, 43)' };
  sandbox.termlabEditorTheme.buildTheme();
  assert.strictEqual(captured.dark, true, 'dark --tl-bg (#21252b) reports dark');
});

// The F2 regression. Under Light the editor used to take --tl-terminal-bg
// (still the dark terminal background, by design) and paint #1F2933 text on
// it — about 1.3:1, unreadable — while also flagging itself `dark: false`.
check('under Light the editor takes its background from --tl-bg, not the terminal palette', () => {
  const light = captureEditorTheme(LIGHT_VARS, 'light');

  assert.strictEqual(light.styles['&'].backgroundColor, LIGHT_VARS['--tl-bg'],
    'editor background is the app background');
  assert.notStrictEqual(light.styles['&'].backgroundColor, LIGHT_VARS['--tl-terminal-bg'],
    'and specifically NOT the terminal background');
  assert.strictEqual(light.styles['&'].color, LIGHT_VARS['--tl-fg']);
  assert.strictEqual(light.styles['.cm-gutters'].backgroundColor, LIGHT_VARS['--tl-bg'],
    'the gutter follows the same surface');
  assert.strictEqual(light.styles['.cm-panels'].backgroundColor, LIGHT_VARS['--tl-bg'],
    'so do the vim/search panels');
  assert.strictEqual(light.opts.dark, false, 'CodeMirror is told it is a light editor');
});

check('under Light the syntax colours come from app tokens, not the terminal ANSI vars', () => {
  const light = captureEditorTheme(LIGHT_VARS, 'light');
  const accent = LIGHT_VARS['--tl-accent'];

  assert.strictEqual(specColor(light, 'string'), accent, 'strings take --tl-accent, not --green');
  assert.notStrictEqual(specColor(light, 'string'), LIGHT_VARS['--green']);
  assert.strictEqual(specColor(light, 'number'), accent, 'numbers take --tl-accent, not --yellow');
  assert.notStrictEqual(specColor(light, 'number'), LIGHT_VARS['--yellow']);
  assert.strictEqual(specColor(light, 'typeName'), accent, 'types take --tl-accent, not --cyan');
  assert.notStrictEqual(specColor(light, 'typeName'), LIGHT_VARS['--cyan']);
  assert.strictEqual(specColor(light, 'definition'), LIGHT_VARS['--tl-fg'], 'definitions take --tl-fg, not --blue');
  assert.notStrictEqual(specColor(light, 'definition'), LIGHT_VARS['--blue']);

  // The colours that were already token-derived are untouched by the branch.
  assert.strictEqual(specColor(light, 'keyword'), accent);
  assert.strictEqual(specColor(light, 'comment'), LIGHT_VARS['--tl-fg-muted']);
  assert.strictEqual(specColor(light, 'invalid'), LIGHT_VARS['--tl-danger']);
});

// The dark byte-identity pin. `feat/termlab-light` is merged on the promise
// that a dark install is unchanged, and theme.js is the one file on the
// branch where a light code path lives inside a function the dark path also
// executes. The fixture is the serialized theme this file produced at
// b043fbf — the commit before that branch existed.
check('under Dark the built theme is byte-identical to the pre-branch snapshot', () => {
  const dark = captureEditorTheme(DARK_VARS, 'dark');
  assert.strictEqual(dark.json, EDITOR_THEME_DARK_AT_BASE,
    'the dark editor theme changed; see scripts/tests/fixtures/editor-theme-dark-base.mjs');
  // Restated positively so a failure above reads clearly: dark still prefers
  // the terminal background and still uses the ANSI accents.
  assert.strictEqual(dark.styles['&'].backgroundColor, DARK_VARS['--tl-terminal-bg']);
  assert.strictEqual(specColor(dark, 'string'), DARK_VARS['--green']);
  assert.strictEqual(dark.opts.dark, true);
});

check('under System-resolved-dark the theme is the dark one, byte for byte', () => {
  // 'system' with the OS on dark must land on exactly the same theme as an
  // explicit 'dark' — the branch keys off the RESOLVED value, not the mode.
  const doc = makeDocStub();
  const sandbox = loadFresh([APPEARANCE, THEME], (s) => { s.document = doc; });
  sandbox.getComputedStyle = () => ({ getPropertyValue: (name) => DARK_VARS[name] || '' });
  const mm = makeMatchMediaStub(true); // OS prefers dark
  sandbox.termlabAppearance.apply('system', { doc, matchMedia: mm.matchMediaFn });
  assert.strictEqual(sandbox.termlabAppearance.current(), 'dark');

  const out = {};
  const tagsCache = {};
  sandbox.CM6 = {
    EditorView: { theme: (styles, opts) => { out.styles = styles; out.opts = opts; return {}; } },
    tags: new Proxy({}, { get(_, p) { if (!(p in tagsCache)) tagsCache[p] = tagFn(String(p)); return tagsCache[p]; } }),
    HighlightStyle: { define: (specs) => { out.specs = specs; return {}; } },
    syntaxHighlighting: (hs) => hs,
  };
  sandbox.termlabEditorTheme.buildTheme();
  const json = JSON.stringify(out, (k, v) => (typeof v === 'function' ? (v.tagName || '[fn]') : v), 2);
  assert.strictEqual(json, EDITOR_THEME_DARK_AT_BASE);
});

// ------------------------------------------------------- editor flip wiring
//
// An OS light/dark flip emits no `config-changed` at all, so nothing used to
// rebuild an open editor's theme: its colours are baked at buildTheme() time.
// config-runtime subscribes the same pane walk applyConfigChanged uses to the
// appearance notification.

function makeConfigRuntimeSandbox(seedExtra) {
  const doc = makeDocStub();
  doc.documentElement.classList = { toggle: () => {} };
  const sandbox = loadFresh([APPEARANCE, CONFIG_RUNTIME], (s) => {
    s.document = doc;
    s.console = { log: () => {}, warn: () => {}, error: () => {} };
    s.requestAnimationFrame = (fn) => { fn(); };
    if (typeof seedExtra === 'function') seedExtra(s);
  });
  return { sandbox, doc };
}

check('an appearance change refreshes every open editor pane (and nothing else)', async () => {
  const refreshed = [];
  const { sandbox, doc } = makeConfigRuntimeSandbox((s) => {
    s.termlabEditorPane = {
      refreshTheme: (view) => { refreshed.push(view); },
      setVimMode: () => {},
      setFontSize: () => {},
    };
  });

  const editorView = { id: 'editor-1' };
  const panes = new Map([
    ['p1', { kind: 'editor', view: editorView }],
    ['p2', { kind: 'terminal', term: { options: {} } }],
    ['p3', { kind: 'editor', view: null }], // not yet mounted
  ]);

  const runtime = sandbox.termlabConfigRuntime.create({
    invoke: async () => ({}),
    listenOnCurrentWindow: () => {},
    refreshKeyboardShortcutFallbacks: async () => {},
    getPanes: () => panes,
    setTheme: () => {},
    getFontFallbacks: () => '',
    setTermFontFamily: () => {},
    setTermFontSize: () => {},
    setEditorVimMode: () => {},
  });
  runtime.init();

  // The flip now re-fetches the palette before walking the panes, so the walk
  // lands a microtask later than it used to; appearanceSettled() is the
  // handle on that promise (test-only — nothing in the app reads it).
  sandbox.termlabAppearance.apply('light', { doc });
  await runtime.appearanceSettled();
  assert.deepStrictEqual(refreshed, [editorView], 'the one mounted editor rebuilt its theme');

  sandbox.termlabAppearance.apply('dark', { doc });
  await runtime.appearanceSettled();
  assert.deepStrictEqual(refreshed, [editorView, editorView], 'and again on the way back');
});

// ------------------------------------------------- appearance -> re-theme
//
// The default terminal theme is now the reserved name `auto`, which resolves
// to TermLab Dark or TermLab Light by the app's RESOLVED appearance. Rust
// cannot resolve that itself — `system` lives in matchMedia inside the
// webview — so every get_theme_colors invoke carries
// `termlabAppearance.current()`, and an appearance flip has to re-fetch.

check('the flip re-fetches get_theme_colors with the NEW resolved appearance', async () => {
  const calls = [];
  const { sandbox, doc } = makeConfigRuntimeSandbox((s) => {
    s.termlabEditorPane = { refreshTheme: () => {}, setVimMode: () => {}, setFontSize: () => {} };
  });

  // A stand-in backend: `auto` answers with a different palette per
  // appearance, exactly as termlab_core::effective_theme does.
  const PALETTES = {
    dark: { background: '#070A0E', foreground: '#BFC6CE' },
    light: { background: '#FFFFFF', foreground: '#1F2933' },
  };

  const terminalPane = { kind: 'terminal', term: { options: {} } };
  const applied = [];
  const runtime = sandbox.termlabConfigRuntime.create({
    invoke: async (command, args) => {
      calls.push({ command, args });
      if (command === 'get_theme_colors') return PALETTES[args.resolvedAppearance];
      throw new Error(`unexpected command ${command}`);
    },
    listenOnCurrentWindow: () => {},
    refreshKeyboardShortcutFallbacks: async () => {},
    getPanes: () => new Map([['p1', terminalPane]]),
    setTheme: (t) => { applied.push(t); },
    getFontFallbacks: () => '',
    setTermFontFamily: () => {},
    setTermFontSize: () => {},
    setEditorVimMode: () => {},
  });
  runtime.init();

  sandbox.termlabAppearance.apply('light', { doc });
  await runtime.appearanceSettled();

  assert.strictEqual(calls.length, 1, 'the flip issued exactly one theme fetch');
  assert.strictEqual(calls[0].command, 'get_theme_colors');
  // Compared field-wise rather than with deepStrictEqual: the args object is
  // built inside the vm context and does not share this realm's prototype.
  assert.strictEqual(calls[0].args.resolvedAppearance, 'light',
    'and it carried the NEW resolved value, not the outgoing one');
  assert.deepStrictEqual(Object.keys(calls[0].args), ['resolvedAppearance'],
    'and nothing else');
  assert.strictEqual(applied.length, 1);
  assert.strictEqual(applied[0].background, '#FFFFFF', 'the light palette reached setTheme');
  assert.strictEqual(terminalPane.term.options.theme.background, '#FFFFFF',
    'and the open terminal pane, via the same walk applyConfigChanged uses');

  sandbox.termlabAppearance.apply('dark', { doc });
  await runtime.appearanceSettled();
  assert.strictEqual(calls[1].args.resolvedAppearance, 'dark');
  assert.strictEqual(terminalPane.term.options.theme.background, '#070A0E', 'and back');
});

// The decoupling guarantee at the frontend boundary. A user on a concrete
// theme still gets the re-fetch (harmless), but the RESULT must be identical
// — their terminal must not shift palette when the app appearance flips.
// Rust pins the resolution invariance itself
// (theme.rs: a_concrete_theme_name_yields_an_identical_payload_under_both_appearances);
// this pins that the frontend does not introduce a difference of its own.
check('a concrete theme name produces an identical theme object across a flip', async () => {
  const { sandbox, doc } = makeConfigRuntimeSandbox();
  const GRUVBOX = {
    background: '#282828', foreground: '#ebdbb2',
    cursor_color: '#ebdbb2', cursor_text: '#282828',
    selection_bg: '#504945', selection_text: '#ebdbb2',
    black: '#282828', red: '#cc241d', green: '#98971a', yellow: '#d79921',
    blue: '#458588', magenta: '#b16286', cyan: '#689d6a', white: '#a89984',
    bright_black: '#928374', bright_red: '#fb4934', bright_green: '#b8bb26',
    bright_yellow: '#fabd2f', bright_blue: '#83a598', bright_magenta: '#d3869b',
    bright_cyan: '#8ec07c', bright_white: '#ebdbb2',
    extended_ansi: [],
  };

  const applied = [];
  const runtime = sandbox.termlabConfigRuntime.create({
    // colors.theme = "gruvbox_dark": the backend ignores resolvedAppearance
    // entirely for a concrete name, which is the whole point.
    invoke: async () => GRUVBOX,
    listenOnCurrentWindow: () => {},
    refreshKeyboardShortcutFallbacks: async () => {},
    getPanes: () => new Map(),
    setTheme: (t) => { applied.push(t); },
    getFontFallbacks: () => '',
    setTermFontFamily: () => {},
    setTermFontSize: () => {},
    setEditorVimMode: () => {},
  });
  runtime.init();

  sandbox.termlabAppearance.apply('light', { doc });
  await runtime.appearanceSettled();
  sandbox.termlabAppearance.apply('dark', { doc });
  await runtime.appearanceSettled();

  assert.strictEqual(applied.length, 2, 'both flips re-themed');
  assert.deepStrictEqual(applied[0], applied[1],
    'a Gruvbox user sees the same palette under both appearances');
});

check('a failing re-theme fetch still refreshes open editors', async () => {
  const refreshed = [];
  const { sandbox, doc } = makeConfigRuntimeSandbox((s) => {
    s.termlabEditorPane = {
      refreshTheme: (view) => { refreshed.push(view); },
      setVimMode: () => {},
      setFontSize: () => {},
    };
  });
  const editorView = { id: 'editor-1' };
  const runtime = sandbox.termlabConfigRuntime.create({
    invoke: async () => { throw new Error('backend is unhappy'); },
    listenOnCurrentWindow: () => {},
    refreshKeyboardShortcutFallbacks: async () => {},
    getPanes: () => new Map([['p1', { kind: 'editor', view: editorView }]]),
    setTheme: () => {},
    getFontFallbacks: () => '',
    setTermFontFamily: () => {},
    setTermFontSize: () => {},
    setEditorVimMode: () => {},
  });
  runtime.init();

  sandbox.termlabAppearance.apply('light', { doc });
  await runtime.appearanceSettled();
  assert.deepStrictEqual(refreshed, [editorView],
    'the editor-only fallback ran, so a rejected fetch cannot strand open editors');
});

check('applyConfigChanged fetches the theme with the resolved appearance too', async () => {
  const calls = [];
  const { sandbox } = makeConfigRuntimeSandbox();
  const runtime = sandbox.termlabConfigRuntime.create({
    invoke: async (command, args) => {
      calls.push({ command, args });
      if (command === 'get_app_config') return { appearance_mode: 'light' };
      if (command === 'get_theme_colors') return { background: '#FFFFFF' };
      if (command === 'get_terminal_config') return { font_family: '', font_size: 14 };
      throw new Error(`unexpected command ${command}`);
    },
    listenOnCurrentWindow: () => {},
    refreshKeyboardShortcutFallbacks: async () => {},
    getPanes: () => new Map(),
    setTheme: () => {},
    getFontFallbacks: () => '',
    setTermFontFamily: () => {},
    setTermFontSize: () => {},
    setEditorVimMode: () => {},
  });

  await runtime.applyConfigChanged();
  const themeCall = calls.find((c) => c.command === 'get_theme_colors');
  assert.ok(themeCall, 'the theme was fetched');
  assert.strictEqual(themeCall.args.resolvedAppearance, 'light',
    'and it carried the appearance the same handler had just applied');
  // Ordering matters: the appearance apply has to land BEFORE the fetch, or
  // the fetch carries the outgoing value.
  assert.ok(
    calls.findIndex((c) => c.command === 'get_app_config')
      < calls.findIndex((c) => c.command === 'get_theme_colors'),
    'get_app_config (which resolves the appearance) precedes get_theme_colors',
  );
});

// -------------------------------------------------------- extendedAnsi
//
// Alacritty's colors.indexed_colors[] reaches xterm through
// ITheme.extendedAnsi. Rust sends it sparse (null per un-overridden slot);
// config-service maps the nulls to undefined so xterm's
//   function p(e, t) { if (void 0 !== e) try { return css.toColor(e) } catch {} return t }
// takes the clean short-circuit branch instead of relying on the throw.

check('extended_ansi becomes extendedAnsi, with nulls turned into undefined holes', () => {
  const sandbox = loadFresh([CONFIG_SERVICE], (s) => {
    s.document = { documentElement: { style: { setProperty() {}, removeProperty() {} } } };
  });
  const theme = sandbox.termlabConfigService.toTerminalTheme(
    { background: '#000000', extended_ansi: ['#d18616', null, '#f97583'] },
    {},
  );
  assert.strictEqual(theme.extendedAnsi.length, 3);
  assert.strictEqual(theme.extendedAnsi[0], '#d18616', 'element 0 is ANSI slot 16');
  assert.strictEqual(theme.extendedAnsi[1], undefined, 'a hole is undefined, not null');
  assert.strictEqual(theme.extendedAnsi[2], '#f97583');
});

check('a theme with no indexed colors omits extendedAnsi entirely', () => {
  const sandbox = loadFresh([CONFIG_SERVICE], (s) => {
    s.document = { documentElement: { style: { setProperty() {}, removeProperty() {} } } };
  });
  for (const extended_ansi of [[], undefined, null]) {
    const theme = sandbox.termlabConfigService.toTerminalTheme(
      { background: '#000000', extended_ansi }, {},
    );
    assert.ok(!('extendedAnsi' in theme),
      `extended_ansi=${JSON.stringify(extended_ansi)} must not add the key`);
  }
});

// The wiring in the two HTML entrypoints and the boot/sync modules cannot be
// driven from node, so it is asserted statically — without this, `auto` would
// silently resolve dark in the settings window, the chooser and at startup.
check('every get_theme_colors call site passes resolvedAppearance', () => {
  const sites = [
    'app/config-runtime.js',
    'app/startup-runtime.js',
    'app/core/appearance-sync.js',
    'settings.html',
    'chooser.html',
  ];
  for (const site of sites) {
    const source = fs.readFileSync(path.join(FRONTEND, site), 'utf8');
    const invokes = source.match(/invoke\(\s*'(?:get_theme_colors|GET_THEME_COLORS)'[^)]*/g) || [];
    assert.ok(invokes.length > 0, `${site} still invokes get_theme_colors`);
    for (const call of invokes) {
      assert.ok(/resolvedAppearance/.test(call),
        `${site} passes resolvedAppearance: ${call}`);
    }
  }
});

// terminal-themes Task 4 retired this call site: the settings "Theme" row's
// old single-selection preview (which re-fetched preview_theme_colors, and
// so needed resolvedAppearance to render `auto` correctly) was replaced by
// the "Terminal Theme" picker (app/features/settings/theme-picker.js),
// which renders every entry's palette strip straight from
// list_terminal_themes()'s already-included palette_preview — no per-entry
// round trip, so no appearance argument to get right or wrong here. (The
// picker's Auto entry has no fixed palette to show at all — see
// test_settings_terminal_theme_picker.mjs's "auto renders no palette
// strip" — sidestepping the stale-preview failure mode this check used to
// guard, rather than reproducing it.) This assertion now pins the
// retirement itself, so the round trip cannot silently come back without
// the resolvedAppearance plumbing this suite's other checks require.
check('the settings preview round trip was retired, not left half-removed', () => {
  const source = fs.readFileSync(
    path.join(FRONTEND, 'app/features/settings/sections-appearance.js'), 'utf8',
  );
  assert.ok(!/invoke\(\s*['"]preview_theme_colors['"]/.test(source),
    'sections-appearance.js must not call preview_theme_colors any more');
  assert.ok(/themePicker\.normalizeThemeEntries/.test(source),
    'sections-appearance.js must build its entries via theme-picker.js');
  assert.ok(/themePicker\.buildTerminalThemePicker/.test(source),
    'sections-appearance.js must render via theme-picker.js');
});

// The startup ordering bug this would otherwise have: loadTheme used to run
// alongside applyAppConfig, so under `auto` a light install painted the dark
// built-in until the next flip.
check('startup chains the theme load behind the app-config (appearance) fetch', () => {
  const source = fs.readFileSync(path.join(FRONTEND, 'app/main-runtime.js'), 'utf8');
  assert.ok(
    source.indexOf('const startupAppConfigPromise') < source.indexOf('const startupThemePromise'),
    'the app-config promise is created first',
  );
  assert.ok(
    /startupThemePromise\s*=[\s\S]{0,400}startupAppConfigPromise[\s\S]{0,200}loadTheme/.test(source),
    'and the theme load is chained onto it, not run in parallel',
  );
});

// Both secondary windows fetch the theme AFTER applying the appearance, or
// the fetch carries the outgoing value.
check('settings.html and chooser.html resolve appearance before fetching the theme', () => {
  for (const page of ['settings.html', 'chooser.html']) {
    const html = fs.readFileSync(path.join(FRONTEND, page), 'utf8');
    assert.ok(
      html.indexOf('termlabAppearance.apply(') < html.indexOf("invoke('GET_THEME_COLORS'"),
      `${page} applies the appearance before it fetches the theme`,
    );
  }
});

// F4: with the get_app_config fetch moved ahead of get_theme_colors, an
// invoke failure used to abort the handler before the theme reached the
// window at all.
check('a failing get_app_config no longer takes the theme CSS down with it', async () => {
  const applied = [];
  const { sandbox } = makeConfigRuntimeSandbox();
  sandbox.termlabConfigService = {
    applyThemeCss: (tc) => { applied.push(tc); },
    applyUiConfig: () => { throw new Error('applyUiConfig must not run without a config'); },
    toTerminalTheme: (tc, fallback) => fallback,
  };

  let vimCalls = 0;
  const runtime = sandbox.termlabConfigRuntime.create({
    invoke: async (command) => {
      if (command === 'get_app_config') throw new Error('backend is unhappy');
      if (command === 'get_theme_colors') return { background: '#070A0E', foreground: '#BFC6CE' };
      if (command === 'get_terminal_config') return { font_family: '', font_size: 14 };
      throw new Error(`unexpected command ${command}`);
    },
    listenOnCurrentWindow: () => {},
    refreshKeyboardShortcutFallbacks: async () => {},
    getPanes: () => new Map(),
    setTheme: () => {},
    getFontFallbacks: () => '',
    setTermFontFamily: () => {},
    setTermFontSize: () => {},
    setEditorVimMode: () => { vimCalls += 1; },
  });

  await runtime.applyConfigChanged();
  assert.strictEqual(applied.length, 1, 'applyThemeCss ran despite the app-config failure');
  assert.strictEqual(applied[0].background, '#070A0E');
  assert.strictEqual(vimCalls, 0, 'and the config-dependent work was skipped, not fed a null config');
});

// ------------------------------------------------------- secondary windows
//
// save_settings broadcasts `config-changed` to every window, but a broadcast
// only helps windows that listen — and until appearance-sync.js the only
// listener lived in config-runtime.js, which index.html alone loads. So the
// standalone settings window flipped every other window from its own Apply
// button and stayed on the old appearance itself.

function makeSyncSandbox() {
  const doc = makeDocStub();
  const sandbox = loadFresh([APPEARANCE, APPEARANCE_SYNC], (s) => {
    s.document = doc;
    s.console = { log: () => {}, warn: () => {}, error: () => {} };
  });
  const calls = { themeCss: [], uiConfig: [] };
  sandbox.termlabConfigService = {
    applyThemeCss: (tc) => { calls.themeCss.push(tc); },
    applyUiConfig: (cfg) => { calls.uiConfig.push(cfg); },
  };
  return { sandbox, doc, calls };
}

check('a settings-shaped window re-applies appearance, native chrome and theme css on config-changed', async () => {
  const { sandbox, doc, calls } = makeSyncSandbox();
  let mode = 'dark';
  let handler = null;

  sandbox.termlabAppearance.apply(mode); // boot
  assert.strictEqual(doc.attrs['data-tl-appearance'], undefined, 'the window boots dark');

  sandbox.termlabAppearanceSync.create({
    invoke: async (command) => {
      if (command === 'GET_APP_CONFIG') return { appearance_mode: mode, platform: 'macos', decorations: 'full' };
      if (command === 'GET_THEME_COLORS') return { background: '#E3E8EF' };
      throw new Error(`unexpected command ${command}`);
    },
    listen: (name, fn) => {
      assert.strictEqual(name, 'config-changed');
      handler = fn;
      return Promise.resolve(() => {});
    },
    applyUiConfig: true,
    label: 'settings',
  }).init();

  assert.ok(typeof handler === 'function', 'a config-changed listener was registered');

  mode = 'light'; // the user clicked Apply in THIS window (keepOpen path)
  await handler();

  assert.strictEqual(doc.attrs['data-tl-appearance'], 'light', 'the settings window flipped itself');
  assert.strictEqual(sandbox.termlabAppearance.current(), 'light');
  assert.strictEqual(calls.uiConfig.length, 1, 'applyUiConfig re-ran (this is what re-tints the native frame)');
  assert.strictEqual(calls.uiConfig[0].appearance_mode, 'light');
  assert.strictEqual(calls.themeCss.length, 1, 'and the terminal accent vars were re-applied');
  assert.strictEqual(calls.themeCss[0].background, '#E3E8EF');
});

check('a chooser-shaped window re-applies appearance and theme css, and does not add applyUiConfig', async () => {
  const { sandbox, doc, calls } = makeSyncSandbox();
  let handler = null;
  sandbox.termlabAppearance.apply('dark');

  sandbox.termlabAppearanceSync.create({
    invoke: async (command) => {
      if (command === 'GET_APP_CONFIG') return { appearance_mode: 'light' };
      if (command === 'GET_THEME_COLORS') return { background: '#E3E8EF' };
      throw new Error(`unexpected command ${command}`);
    },
    listen: (name, fn) => { handler = fn; return Promise.resolve(() => {}); },
    label: 'chooser',
  }).init();

  await handler();
  assert.strictEqual(doc.attrs['data-tl-appearance'], 'light');
  assert.strictEqual(calls.themeCss.length, 1);
  assert.strictEqual(calls.uiConfig.length, 0,
    'the chooser boot does not call applyUiConfig, so its sync must not either');
});

check('a failing GET_APP_CONFIG in a secondary window still lets the theme css through', async () => {
  const { sandbox, calls } = makeSyncSandbox();
  let handler = null;

  sandbox.termlabAppearanceSync.create({
    invoke: async (command) => {
      if (command === 'GET_APP_CONFIG') throw new Error('backend is unhappy');
      if (command === 'GET_THEME_COLORS') return { background: '#E3E8EF' };
      throw new Error(`unexpected command ${command}`);
    },
    listen: (name, fn) => { handler = fn; return Promise.resolve(() => {}); },
    applyUiConfig: true,
    label: 'settings',
  }).init();

  await handler();
  assert.strictEqual(calls.themeCss.length, 1, 'the two fetches are isolated from each other');
  assert.strictEqual(calls.uiConfig.length, 0);
});

// The wiring itself cannot be exercised from node — it lives in the inline
// boot scripts of two HTML entrypoints — so it is asserted statically. Without
// this, every check above could pass against a module nothing loads.
check('settings.html and chooser.html both load and initialise the sync', () => {
  for (const page of ['settings.html', 'chooser.html']) {
    const html = fs.readFileSync(path.join(FRONTEND, page), 'utf8');
    assert.ok(
      html.includes('src="app/core/appearance-sync.js"'),
      `${page} loads app/core/appearance-sync.js`,
    );
    assert.ok(
      /termlabAppearanceSync\.create\(/.test(html) && /\}\)\.init\(\);/.test(html),
      `${page} constructs the sync and calls init()`,
    );
    // appearance-sync.js must load AFTER appearance.js: it reads
    // window.termlabAppearance, and these are classic scripts in document
    // order.
    assert.ok(
      html.indexOf('src="app/core/appearance.js"') < html.indexOf('src="app/core/appearance-sync.js"'),
      `${page} loads appearance.js before appearance-sync.js`,
    );
  }
});

// ---------------------------------------------------------- native chrome
//
// config-service.js decides the macOS frame tint. It has to answer the
// unresolvable case the same way appearance.js does, or a config-shape
// regression tints the frame by OS preference while the webview forces dark.

check('the native window theme follows the same fallback convention as the webview', () => {
  // applyUiConfig is the only exported route to the native-theme decision, so
  // the stub carries the rest of the DOM surface that function touches.
  const doc = makeDocStub();
  const noopStyle = () => ({ setProperty: () => {}, removeProperty: () => {} });
  doc.documentElement.style = noopStyle();
  doc.documentElement.classList = { toggle: () => {} };
  doc.body = { style: noopStyle() };
  doc.getElementById = () => null;
  const sandbox = loadFresh([CONFIG_SERVICE], (s) => { s.document = doc; });

  const setThemeCalls = [];
  sandbox.__TAURI__ = {
    window: {
      getCurrentWindow: () => ({
        setTheme: (value) => { setThemeCalls.push(value); return Promise.resolve(); },
      }),
    },
  };
  const macFull = (mode) => ({ platform: 'macos', decorations: 'full', appearance_mode: mode });

  sandbox.termlabConfigService.applyUiConfig(macFull('light'));
  sandbox.termlabConfigService.applyUiConfig(macFull('dark'));
  sandbox.termlabConfigService.applyUiConfig(macFull('system'));
  sandbox.termlabConfigService.applyUiConfig(macFull(undefined));

  assert.deepStrictEqual(setThemeCalls, ['light', 'dark', null, 'dark'],
    'light/dark are explicit, system hands off to the OS, and a missing mode is dark');
});

// -------------------------------------------------------------------- run

let failed = 0;
for (const { name, fn } of results) {
  try {
    await fn();
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
