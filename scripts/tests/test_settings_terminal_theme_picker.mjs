// Run: node scripts/tests/test_settings_terminal_theme_picker.mjs
//
// The settings "Terminal Theme" picker (app/features/settings/
// theme-picker.js) and its wiring into the Appearance section
// (app/features/settings/sections-appearance.js), which is the sole consumer
// of list_terminal_themes()'s payload — no backend round trip per selection
// (see test_appearance.mjs's "the settings preview round trip was retired,
// not left half-removed" for that retirement's own pin).
//
// The picker is ONE control — a <select>/tl-combo like every sibling row —
// plus a fake-terminal preview box below the row. It previously rendered a
// second, parallel palette-strip list; the semantics that list carried
// (which entries are pickable, and why an unpickable one isn't) now live in
// the option labels, and the palette it showed is shown far better by the
// preview box.
//
// No jsdom in this repo (see test_tl_combo.mjs/test_tl_dialog.mjs for the
// precedent) — this stubs just enough of `window`/`document` for the real
// theme-picker.js and sections-appearance.js to load via eval(), then
// exercises:
//   - normalizeThemeEntries: pure, one fixture carrying every entry kind
//     list_terminal_themes() can produce (builtin, plain user, a user theme
//     shadowing a builtin, a broken file) plus the two ways a reserved
//     `auto.toml` can show up (parses fine / also broken) — order asserted.
//   - buildTerminalThemePicker: the <select>'s options mirror that order;
//     broken/reserved/missing entries are disabled and labeled with their
//     reason; a committed selection round-trips into the value onChange
//     receives; the preview box paints the SELECTED entry's exact stubbed
//     palette; the synthetic Auto entry never collides with a same-valued
//     reserved entry, and resolves its preview by app appearance.
//   - buildThemePreview/updateThemePreview: the fake-terminal DOM, and that
//     palette colors reach it as inline --tp-* custom properties (so the
//     design-system stylesheet stays token-only).
//   - renderAppearance: end-to-end through the section renderer with a
//     stubbed list_terminal_themes payload, confirming the row order and
//     that the row is wired into the same pendingSettings.colors.theme +
//     addRow/setRowTarget path every sibling settings row uses.
import assert from 'node:assert';

// --- minimal element stub --------------------------------------------------
// Mirrors test_tl_combo.mjs's stub (appendChild/insertAdjacentElement/
// classList/select-value-sync) plus the bits that file didn't need:
// `dataset` (theme-picker.js tags rows with it) and classList.toggle(cls,
// force) (theme-picker.js's refresh() uses it), and appendChild maintaining
// a <select>'s `.options` for real (theme-picker.js builds <option>s via
// select.appendChild(), not by assigning `.options` directly).
function makeElement(tag) {
  const tagName = String(tag || 'div').toUpperCase();
  const attrs = new Map();
  const listeners = new Map();
  const el = {
    tagName,
    className: '',
    children: [],
    options: tagName === 'SELECT' ? [] : undefined,
    dataset: {},
    // Plain properties still work (`el.style.background = ...`); the two
    // methods are what theme-picker.js uses to set the preview's --tp-*
    // custom properties, which is how a selected palette reaches the CSS.
    style: {
      setProperty(name, value) { this[name] = value; },
      getPropertyValue(name) { return this[name] || ''; },
    },
    disabled: false,
    tabIndex: -1,
    isConnected: false,
    appendChild(child) {
      this.children.push(child);
      child.parentNode = this;
      child.isConnected = this.isConnected;
      if (this.tagName === 'SELECT' && child.tagName === 'OPTION') {
        child.index = this.options.length;
        this.options.push(child);
      }
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
      const out = [];
      const walk = (node) => {
        for (const c of node.children) {
          if (sel === '.' + String(c.className).split(' ')[0]) out.push(c);
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
    focus() {},
    classList: {
      _set: new Set(),
      add(c) { this._set.add(c); },
      remove(c) { this._set.delete(c); },
      contains(c) { return this._set.has(c); },
      toggle(c, force) {
        const want = typeof force === 'boolean' ? force : !this._set.has(c);
        if (want) this._set.add(c); else this._set.delete(c);
        return want;
      },
    },
  };
  Object.defineProperty(el, 'className', {
    get() { return Array.from(el.classList._set).join(' '); },
    set(v) { el.classList._set = new Set(String(v).split(' ').filter(Boolean)); },
  });
  Object.defineProperty(el, 'textContent', {
    get() { return el._text || ''; },
    set(v) { el._text = String(v); },
  });
  if (tagName === 'SELECT') {
    let currentValue = '';
    Object.defineProperty(el, 'value', {
      get() { return currentValue; },
      set(v) {
        currentValue = v;
        const idx = el.options.findIndex((o) => o.value === v);
        el.selectedIndex = idx;
      },
    });
    el.selectedIndex = -1;
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

const { readFileSync } = await import('node:fs');
eval(readFileSync('crates/termlab_tauri/frontend/app/features/settings/theme-picker.js', 'utf8'));

const themePicker = window.termlabSettingsThemePicker;
assert.equal(typeof themePicker.normalizeThemeEntries, 'function');
assert.equal(typeof themePicker.buildTerminalThemePicker, 'function');

// --- shared fixture: one raw list_terminal_themes() payload carrying every
// entry kind the command can produce -----------------------------------------
function ansiSet(seed) {
  return Array.from({ length: 16 }, (_, i) => `#${seed}${i.toString(16).padStart(2, '0')}`);
}

const RAW_ENTRIES = [
  // plain built-in
  {
    name: 'TermLab Dark',
    source: 'builtin',
    palettePreview: { bg: '#1e1e2e', fg: '#cdd6f4', ansi: ansiSet('1') },
    shadowsBuiltin: false,
  },
  // plain user theme
  {
    name: 'Gruvbox',
    source: 'user',
    palettePreview: { bg: '#282828', fg: '#ebdbb2', ansi: ansiSet('2') },
    shadowsBuiltin: false,
  },
  // user theme shadowing a built-in of the same name (the built-in itself is
  // already excluded from list_terminal_themes()'s own output per the
  // existing later-dir-wins rule — only the shadowing entry appears)
  {
    name: 'TermLab Light',
    source: 'user',
    palettePreview: { bg: '#ffffff', fg: '#111111', ansi: ansiSet('3') },
    shadowsBuiltin: true,
  },
  // broken file
  { name: 'BrokenTheme', error: 'missing field `colors`' },
  // reserved name (auto.toml) that DOES parse
  {
    name: 'auto',
    source: 'user',
    palettePreview: { bg: '#000000', fg: '#ffffff', ansi: ansiSet('4') },
    shadowsBuiltin: false,
  },
];

// --- normalizeThemeEntries: order, shape, both auto.toml cases -------------
{
  const entries = themePicker.normalizeThemeEntries(RAW_ENTRIES);
  assert.equal(entries.length, 6, 'synthetic Auto + 5 raw entries');
  assert.deepEqual(
    entries.map((e) => e.kind),
    ['auto', 'parsed', 'parsed', 'parsed', 'broken', 'reserved'],
    'kinds in the exact order the raw list arrived, Auto prepended',
  );
  assert.deepEqual(
    entries.map((e) => e.name),
    ['Auto', 'TermLab Dark', 'Gruvbox', 'TermLab Light', 'BrokenTheme', 'auto'],
  );

  const auto = entries[0];
  assert.equal(auto.value, 'auto');
  assert.equal(auto.selectable, true);
  assert.equal(auto.palettePreview, null, 'Auto has no fixed palette to show');

  const builtin = entries[1];
  assert.equal(builtin.selectable, true);
  assert.equal(builtin.source, 'builtin');
  assert.equal(builtin.shadowsBuiltin, false);
  assert.equal(builtin.note, null);
  assert.deepEqual(builtin.palettePreview, RAW_ENTRIES[0].palettePreview);

  const shadowing = entries[3];
  assert.equal(shadowing.selectable, true);
  assert.equal(shadowing.shadowsBuiltin, true);
  assert.equal(shadowing.note, 'Overrides built-in', 'shadowsBuiltin entries are marked');

  const broken = entries[4];
  assert.equal(broken.selectable, false, 'broken entries are not selectable');
  assert.equal(broken.error, 'missing field `colors`');
  assert.equal(broken.palettePreview, null);

  const reserved = entries[5];
  assert.equal(reserved.kind, 'reserved');
  assert.equal(reserved.selectable, false, 'a reserved-name entry is never selectable');
  assert.equal(reserved.value, 'auto', 'shares Auto\'s value (would resolve identically)');
  assert.ok(/reserved/i.test(reserved.note), `note explains the reservation: ${reserved.note}`);
  assert.deepEqual(reserved.palettePreview, RAW_ENTRIES[4].palettePreview,
    'a reserved name that DOES parse still surfaces its real data, per this file\'s doc comment');

  console.log('normalizeThemeEntries: order + shape for every entry kind: ok');
}

// --- the OTHER auto.toml case: a broken file literally named auto.toml ----
// must classify as `reserved`, not `broken` — otherwise its value ("auto")
// would collide with the real Auto entry's "currently selected" check.
{
  const entries = themePicker.normalizeThemeEntries([{ name: 'AUTO', error: 'invalid TOML' }]);
  assert.equal(entries.length, 2);
  const reserved = entries[1];
  assert.equal(reserved.kind, 'reserved', 'reserved-name classification wins over broken');
  assert.equal(reserved.selectable, false);
  assert.equal(reserved.error, 'invalid TOML', 'the parse error is preserved for display');
  assert.ok(/reserved/i.test(reserved.note) && /fail/i.test(reserved.note));
  console.log('normalizeThemeEntries: a broken auto.toml is reserved, not broken: ok');
}

// --- F1b/F4 (branch-review.md): the synthesized `missing` entry -----------
// A currentValue that matches no real entry (a deleted user theme, or a
// hand-edited config naming a theme that was never installed) used to leave
// the native <select> on
// selectedIndex -1: an empty button label, no row marked selected, and
// switching away as the only way out. normalizeThemeEntries now appends a
// synthesized entry carrying the saved name so the combo always has
// something to show and mark selected.
{
  const entries = themePicker.normalizeThemeEntries(RAW_ENTRIES, 'GoneTheme');
  assert.equal(entries.length, 7, 'synthetic Auto + 5 raw entries + 1 synthesized missing entry');
  const missing = entries[6];
  assert.equal(missing.kind, 'missing');
  assert.equal(missing.name, 'GoneTheme');
  assert.equal(missing.value, 'GoneTheme', 'shares the saved value so a <select> assignment matches it');
  assert.equal(missing.label, 'GoneTheme', 'the saved name is shown, not a blank label');
  assert.equal(missing.selectable, false, 'not independently re-selectable — it IS the current selection');
  assert.equal(missing.palettePreview, null, 'greyed palette area: nothing to show for a name with no file');
  assert.equal(missing.note, '(missing)');
  console.log('normalizeThemeEntries: an unmatched currentValue synthesizes a visible missing entry: ok');
}

// A currentValue that DOES match a real entry must not synthesize anything
// extra — the common case (Auto, or any concrete theme that still exists).
{
  const matchedAuto = themePicker.normalizeThemeEntries(RAW_ENTRIES, 'auto');
  assert.equal(matchedAuto.length, 6, 'currentValue "auto" matches the real Auto entry — no synthesis');

  const matchedConcrete = themePicker.normalizeThemeEntries(RAW_ENTRIES, 'Gruvbox');
  assert.equal(matchedConcrete.length, 6, 'currentValue "Gruvbox" matches a real parsed entry — no synthesis');

  console.log('normalizeThemeEntries: a matched currentValue never synthesizes an extra entry: ok');
}

// F4: a hand-edited config can spell the reserved name with any casing.
// effective_theme_name matches it case-insensitively on the Rust side, so
// the picker must not treat "Auto"/"AUTO" as an unmatched value either —
// it resolves to the real Auto entry, not a synthesized one.
{
  for (const cased of ['Auto', 'AUTO', ' auto ']) {
    const entries = themePicker.normalizeThemeEntries(RAW_ENTRIES, cased);
    assert.equal(entries.length, 6, `currentValue "${cased}" must not synthesize a missing entry`);
  }
  console.log('normalizeThemeEntries: a cased "Auto" currentValue never synthesizes: ok');
}

// --- helpers for reading a preview box ------------------------------------
const tp = (preview, name) => preview.style.getPropertyValue(name);
const previewPalette = (preview) => ({
  bg: tp(preview, '--tp-bg'),
  fg: tp(preview, '--tp-fg'),
  ansi: [
    '--tp-black', '--tp-red', '--tp-green', '--tp-yellow',
    '--tp-blue', '--tp-magenta', '--tp-cyan', '--tp-white',
    '--tp-bright-black', '--tp-bright-red', '--tp-bright-green', '--tp-bright-yellow',
    '--tp-bright-blue', '--tp-bright-magenta', '--tp-bright-cyan', '--tp-bright-white',
  ].map((name) => tp(preview, name)),
});

// --- buildTerminalThemePicker: one combo, every entry kind, interaction ----
// The picker renders ONE control now (the <select>/tl-combo half every
// sibling settings row uses). The palette-strip list it used to render
// alongside is gone; each kind's meaning — selectability and the note that
// explained it — moved into the option's own label, asserted below.
{
  const entries = themePicker.normalizeThemeEntries(RAW_ENTRIES);
  let lastOnChangeValue = null;
  let onChangeCalls = 0;
  const built = themePicker.buildTerminalThemePicker(entries, 'auto', (value) => {
    onChangeCalls += 1;
    lastOnChangeValue = value;
  });
  const { select, preview } = built;

  assert.equal(built.list, undefined, 'the parallel palette-strip list is gone');
  assert.ok(preview, 'the picker returns a preview box');

  // --- <select> mirrors `entries` 1:1, in order --------------------------
  assert.equal(select.options.length, 6);
  assert.deepEqual(select.options.map((o) => o.value),
    ['auto', 'TermLab Dark', 'Gruvbox', 'TermLab Light', 'BrokenTheme', 'auto']);
  assert.deepEqual(select.options.map((o) => o.disabled),
    [false, false, false, false, true, true],
    'only the broken and reserved entries are disabled options');
  // Two options share value "auto" (real Auto + the reserved auto.toml);
  // the native .value setter picks the FIRST match in document order, which
  // is the real Auto entry since it is always prepended.
  assert.equal(select.value, 'auto');
  assert.equal(select.selectedIndex, 0);

  // --- option labels carry each kind's meaning ---------------------------
  const labels = select.options.map((o) => o.textContent);
  assert.equal(labels[0], 'Auto (follows appearance)');
  assert.equal(labels[1], 'TermLab Dark', 'a plain entry is just its name');
  assert.equal(labels[2], 'Gruvbox');
  assert.equal(labels[3], 'TermLab Light (overrides built-in)',
    'a user theme shadowing a built-in is marked in its own label');
  assert.equal(labels[4], 'BrokenTheme — Parse error: missing field `colors`',
    'a broken entry is labeled with the ACTUAL parse error, not a generic message');
  assert.ok(/^auto — Reserved name, ignored/.test(labels[5]),
    `the reserved entry's label explains why it can't be picked: ${labels[5]}`);

  console.log('buildTerminalThemePicker: one combo, labels carry every kind\'s meaning: ok');

  // --- the preview shows the CURRENT selection's palette ------------------
  // currentValue is "auto" and the stubbed appearance is dark, so this is
  // TermLab Dark's stubbed palette — not the reserved auto.toml entry's,
  // even though that entry's value is "auto" too.
  assert.deepEqual(previewPalette(preview), RAW_ENTRIES[0].palettePreview,
    'the preview paints the selected entry\'s exact palette into the --tp-* vars');

  console.log('buildTerminalThemePicker: the preview paints the selected palette: ok');

  // --- selection round-trips: select.value -> change -> onChange + preview
  select.value = 'Gruvbox';
  select.dispatchEvent({ type: 'change' });
  assert.equal(onChangeCalls, 1);
  assert.equal(lastOnChangeValue, 'Gruvbox');
  assert.deepEqual(previewPalette(preview), RAW_ENTRIES[1].palettePreview,
    'switching selection repaints the preview live');

  select.value = 'TermLab Light';
  select.dispatchEvent({ type: 'change' });
  assert.equal(lastOnChangeValue, 'TermLab Light');
  assert.deepEqual(previewPalette(preview), RAW_ENTRIES[2].palettePreview,
    'a shadowing user theme previews ITS colors, not the built-in it overrides');

  console.log('buildTerminalThemePicker: selection round-trips and repaints: ok');
}

// --- the preview box's own DOM (recovered fake-terminal shape) ------------
{
  const preview = themePicker.buildThemePreview();
  assert.equal(preview.className, 'tl-settings__theme-preview');

  const swatchRows = preview.children.filter(
    (c) => c.classList.contains('tl-settings__theme-preview-swatches'),
  );
  assert.equal(swatchRows.length, 2, 'a normal row and a bright row');
  assert.equal(swatchRows[0].children.length, 8);
  assert.equal(swatchRows[1].children.length, 8);
  assert.ok(preview.children.some(
    (c) => c.classList.contains('tl-settings__theme-preview-divider')),
    'the swatch divider is present');
  assert.ok(preview.querySelectorAll('.tl-settings__theme-preview-cursor').length === 1,
    'the block cursor is present');

  // Colors reach the box as inline custom properties, never as inline
  // per-element colors — that is what keeps the stylesheet token-only.
  themePicker.updateThemePreview(preview, RAW_ENTRIES[1].palettePreview);
  assert.equal(tp(preview, '--tp-bg'), '#282828');
  assert.equal(tp(preview, '--tp-fg'), '#ebdbb2');
  assert.equal(tp(preview, '--tp-black'), ansiSet('2')[0], 'ANSI 0 -> --tp-black');
  assert.equal(tp(preview, '--tp-bright-white'), ansiSet('2')[15], 'ANSI 15 -> --tp-bright-white');
  assert.equal(tp(preview, '--tp-dim'), ansiSet('2')[8],
    'dim text/border key off bright black: the payload carries no dim_foreground');
  for (const sw of swatchRows[0].children) {
    assert.equal(sw.style.background, undefined,
      'swatch colors come from the box\'s vars via CSS, not per-element inline styles');
  }

  console.log('buildThemePreview/updateThemePreview: fake-terminal DOM + --tp-* painting: ok');
}

// --- Auto previews the appearance-appropriate built-in --------------------
// `auto` has no palette of its own; termlab_core::effective_theme maps it
// onto TermLab Dark or TermLab Light by the app's RESOLVED appearance, and
// both built-ins are already in the list payload — so the preview resolves
// it client-side, with no round trip and no second source of truth.
{
  const entries = themePicker.normalizeThemeEntries(RAW_ENTRIES, 'auto');
  const cases = [
    ['dark', RAW_ENTRIES[0].palettePreview, 'TermLab Dark'],
    ['light', RAW_ENTRIES[2].palettePreview, 'TermLab Light'],
  ];
  for (const [appearance, expected, name] of cases) {
    window.termlabAppearance = { current: () => appearance };
    const { preview } = themePicker.buildTerminalThemePicker(entries, 'auto', () => {});
    assert.deepEqual(previewPalette(preview), expected,
      `Auto under a ${appearance} appearance previews ${name}`);
  }

  // No appearance module at all (or an unresolvable answer) is dark, the
  // same convention effective_theme::DEFAULT_RESOLVED_APPEARANCE uses.
  delete window.termlabAppearance;
  const { preview } = themePicker.buildTerminalThemePicker(entries, 'auto', () => {});
  assert.deepEqual(previewPalette(preview), RAW_ENTRIES[0].palettePreview,
    'an unresolvable appearance falls back to the dark built-in');

  console.log('buildTerminalThemePicker: Auto previews the appearance-appropriate built-in: ok');
}

// --- F1b: an unmatched currentValue never renders a blank combo -----------
// The mutation this proves: if normalizeThemeEntries stopped synthesizing a
// `missing` entry for 'GoneTheme' (i.e. entries came back as the plain 6,
// with no option whose value is 'GoneTheme'), the native <select>.value setter
// below would fail to find a match, select.selectedIndex would land on -1,
// and tl-combo's currentLabel() (app/ui/tl-combo.js: `opt ? opt.textContent
// : ''`) would render an empty button label — the exact regression F1
// reported. Asserting selectedIndex/value/label here reds immediately if
// that synthesis is ever removed.
{
  const entries = themePicker.normalizeThemeEntries(RAW_ENTRIES, 'GoneTheme');
  const { select, preview } = themePicker.buildTerminalThemePicker(entries, 'GoneTheme', () => {});

  assert.notEqual(select.selectedIndex, -1, 'the combo must never land on no selection at all');
  assert.equal(select.value, 'GoneTheme', 'the select carries the saved value, not a blank one');

  const missingOption = select.options[select.options.length - 1];
  assert.equal(missingOption.value, 'GoneTheme');
  assert.equal(missingOption.disabled, true, 'the synthesized entry is not independently selectable');
  assert.equal(missingOption.textContent, 'GoneTheme (missing)',
    'the saved name is shown, marked so it does not read as a theme that exists');

  // The box still renders, showing TermLab Dark — which is what the terminal
  // is ACTUALLY painted with for an unmatched name: resolve_theme falls back
  // to ColorScheme::default(), pinned byte-identical to TermLab Dark.toml.
  assert.deepEqual(previewPalette(preview), RAW_ENTRIES[0].palettePreview,
    'a missing selection previews the real fallback palette, not a blank box');

  console.log('buildTerminalThemePicker: an unmatched currentValue stays visible and selected (F1b): ok');
}

// --- F4: a hand-edited "Auto" (any casing) selects the REAL Auto option ---
{
  const entries = themePicker.normalizeThemeEntries(RAW_ENTRIES, 'Auto');
  assert.equal(entries.length, 6, 'no synthesis for a cased reserved name');
  const { select, preview } = themePicker.buildTerminalThemePicker(entries, 'Auto', () => {});

  assert.equal(select.value, 'auto', 'the select lands on the real (lowercase) Auto option');
  assert.equal(select.selectedIndex, 0);
  // And it resolves as the real Auto entry, not as the same-valued reserved
  // auto.toml — whose own stubbed palette (bg #000000) would show here if
  // the reserved entry were ever treated as the current selection.
  assert.deepEqual(previewPalette(preview), RAW_ENTRIES[0].palettePreview,
    'the reserved auto.toml never stands in for the real Auto entry');
  assert.notEqual(tp(preview, '--tp-bg'), RAW_ENTRIES[4].palettePreview.bg);

  console.log('buildTerminalThemePicker: a cased "Auto" currentValue selects the real Auto entry (F4): ok');
}

// --- renderAppearance: end-to-end through the section renderer -------------
// Minimal, faithful mirrors of renderers.js's addRow/setRowTarget/
// addSectionLabel/addDivider/makeCheckbox/makeInput/makeToggleGroup (cited
// against app/features/settings/renderers.js) — reconstructing the real
// file's full store/search-index wiring here would add mass without adding
// coverage of the picker itself, the same call test_settings_shortcut_
// recorder_escape.mjs makes for tl-dialog's contract.
{
  function addSectionLabel(container, text) {
    const row = document.createElement('div');
    row.textContent = text;
    container.appendChild(row);
    return row;
  }
  function addDivider() {}
  function addRow(container, labelText, descText, controlEl) {
    const row = document.createElement('div');
    row.className = 'tl-settings__row';
    const left = document.createElement('div');
    left.textContent = labelText;
    row.appendChild(left);
    row.appendChild(controlEl);
    container.appendChild(row);
    return row;
  }
  function setRowTarget(row, settingId) {
    if (row && settingId) row.dataset.settingId = settingId;
    return row;
  }
  function makeCheckbox(checked, onChange) {
    const el = document.createElement('input');
    el.addEventListener('change', () => onChange(el.checked));
    return el;
  }
  function makeInput() { return document.createElement('input'); }
  function makeToggleGroup(options, activeValue) {
    const group = document.createElement('div');
    for (const opt of options) {
      const btn = document.createElement('div');
      btn.textContent = opt.label;
      group.appendChild(btn);
    }
    return group;
  }

  window.tlCombo = { attach() { return { button: null, refresh() {} }; } };
  window.tlSpinner = { attach() {} };

  eval(readFileSync('crates/termlab_tauri/frontend/app/features/settings/sections-appearance.js', 'utf8'));
  const sectionsAppearance = window.termlabSettingsSectionsAppearance;
  assert.equal(typeof sectionsAppearance.renderAppearance, 'function');

  const pendingSettings = {
    colors: { theme: 'auto', appearance_mode: 'Dark' },
    termlab: { ui: { font_family: '', font_size: 13, notification_position: 'bottom', disable_animations: false } },
    window: {},
  };

  const container = document.createElement('div');
  const handled = sectionsAppearance.renderAppearance(container, {
    pendingSettings,
    cachedTerminalThemes: RAW_ENTRIES,
    cachedFonts: { all: [] },
    addSectionLabel,
    addRow,
    setRowTarget,
    addDivider,
    makeCheckbox,
    makeInput,
    makeToggleGroup,
  });
  assert.equal(handled, true, 'renderAppearance must report success given a complete deps object');

  // Row order (Appearance rework): Appearance Mode renders FIRST, Terminal
  // Theme second — the terminal row's Auto entry is defined in terms of the
  // appearance above it, so the broader setting leads. Both rows keep their
  // original setRowTarget ids, which is what the settings search index
  // (app/features/settings/constants.js) jumps to.
  const renderedRowIds = container
    .querySelectorAll('.tl-settings__row')
    .map((row) => row.dataset.settingId);
  assert.deepEqual(
    renderedRowIds.slice(0, 2),
    ['appearance:mode', 'appearance:terminal-theme'],
    'Appearance Mode is the first row, Terminal Theme the second',
  );

  eval(readFileSync('crates/termlab_tauri/frontend/app/features/settings/constants.js', 'utf8'));
  const searchIndex = window.termlabSettingsFeatureConstants.SETTINGS_SEARCH_INDEX;
  for (const targetId of ['appearance:mode', 'appearance:terminal-theme']) {
    assert.ok(
      searchIndex.some((item) => item.targetId === targetId),
      `the settings search index still jumps to ${targetId}`,
    );
    assert.ok(renderedRowIds.includes(targetId), `${targetId} resolves to a rendered row`);
  }

  // The row's control is the picker's own <select>, wired to the same
  // addRow/setRowTarget path every sibling settings row uses.
  const themeRow = container.querySelectorAll('.tl-settings__row').find(
    (row) => row.dataset.settingId === 'appearance:terminal-theme',
  );
  assert.ok(themeRow, 'a row is tagged appearance:terminal-theme');
  const select = themeRow.children[1];
  assert.equal(select.tagName, 'SELECT');
  assert.equal(select.options.length, 6, 'Auto + the 5 raw entries');

  // The preview box renders as a sibling of the row, right below it in the
  // section — and no palette-strip list is left behind anywhere.
  const rowIndex = container.children.indexOf(themeRow);
  const preview = container.children[rowIndex + 1];
  assert.ok(preview && preview.classList.contains('tl-settings__theme-preview'),
    'the preview box is appended directly below the Terminal Theme row');
  assert.equal(
    container.querySelectorAll('.tl-settings__theme-picker').length, 0,
    'the old palette-strip list is gone from the section',
  );

  // --- selection round-trips into the saved settings payload --------------
  // (pendingSettings IS what actions.js's applySettings later serializes
  // and sends to save_settings — see store.js's isDirty()/
  // commitPendingAsOriginal() pair, which diff exactly this object.)
  select.value = 'Gruvbox';
  select.dispatchEvent({ type: 'change' });
  assert.equal(pendingSettings.colors.theme, 'Gruvbox',
    'selecting a theme writes colors.theme through the normal pendingSettings path');
  assert.deepEqual(previewPalette(preview), RAW_ENTRIES[1].palettePreview,
    'and the preview below the row follows the new selection');

  console.log('renderAppearance: wired end-to-end, selection round-trips into pendingSettings: ok');
}

console.log('ok');
