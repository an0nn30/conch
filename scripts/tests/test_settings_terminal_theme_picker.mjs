// Run: node scripts/tests/test_settings_terminal_theme_picker.mjs
//
// terminal-themes plan, Task 4: the settings "Terminal Theme" picker
// (app/features/settings/theme-picker.js), and its wiring into the
// Appearance section (app/features/settings/sections-appearance.js), which
// this task made the sole consumer of list_terminal_themes()'s payload,
// replacing the old plain <select> + per-selection theme-color preview
// round trip (see
// test_appearance.mjs's "the settings preview round trip was retired, not
// left half-removed" for that retirement's own pin).
//
// No jsdom in this repo (see test_tl_combo.mjs/test_tl_dialog.mjs for the
// precedent) — this stubs just enough of `window`/`document` for the real
// theme-picker.js and sections-appearance.js to load via eval(), then
// exercises:
//   - normalizeThemeEntries: pure, one fixture carrying every entry kind
//     list_terminal_themes() can produce (builtin, plain user, a user theme
//     shadowing a builtin, a broken file) plus the two ways a reserved
//     `auto.toml` can show up (parses fine / also broken) — order asserted.
//   - buildTerminalThemePicker: the <select> and the palette-strip list stay
//     index-aligned with that order; broken entries are unselectable; a
//     click round-trips into the value onChange receives; palette strips
//     render the exact stubbed colors via inline style (not a CSS class);
//     the synthetic Auto entry never collides with a same-valued reserved
//     entry for "currently selected" purposes.
//   - renderAppearance: end-to-end through the section renderer with a
//     stubbed list_terminal_themes payload, confirming the row is wired
//     into the same pendingSettings.colors.theme + addRow/setRowTarget path
//     every sibling settings row uses.
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
    style: {},
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

// --- buildTerminalThemePicker: DOM shape + swatch colors + interaction ----
{
  const entries = themePicker.normalizeThemeEntries(RAW_ENTRIES);
  let lastOnChangeValue = null;
  let onChangeCalls = 0;
  const { select, list } = themePicker.buildTerminalThemePicker(entries, 'auto', (value) => {
    onChangeCalls += 1;
    lastOnChangeValue = value;
  });

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

  // --- palette-strip list mirrors `entries` 1:1, in order ----------------
  assert.equal(list.children.length, 6);

  const rows = list.children;
  const swatchColors = (row) => row.children[1].children.map((sw) => sw.style.background);

  // Auto (index 0): no palette data to show, not disabled.
  assert.equal(rows[0].classList.contains('is-disabled'), false);
  assert.equal(rows[0].children[1].children.length, 0, 'Auto renders no swatches');
  assert.equal(rows[0].classList.contains('is-selected'), true, 'currentValue "auto" selects the real Auto row');

  // TermLab Dark (index 1): 2 chips (bg, fg) + 16 ANSI swatches, EXACT colors.
  assert.equal(swatchColors(rows[1]).length, 18);
  assert.equal(swatchColors(rows[1])[0], '#1e1e2e', 'bg chip');
  assert.equal(swatchColors(rows[1])[1], '#cdd6f4', 'fg chip');
  assert.deepEqual(swatchColors(rows[1]).slice(2), ansiSet('1'), 'all 16 ANSI swatches, exact stubbed colors');

  // TermLab Light (index 3): shadowsBuiltin note, own (not the built-in's) colors.
  assert.equal(rows[3].children[0].children[1].textContent, 'Overrides built-in');
  assert.equal(swatchColors(rows[3])[0], '#ffffff');

  // BrokenTheme (index 4): disabled, error text, no swatches.
  assert.equal(rows[4].classList.contains('is-disabled'), true);
  assert.equal(rows[4].children[0].children[1].textContent, 'Parse error: missing field `colors`');
  assert.equal(rows[4].children[1].children.length, 0);

  // reserved auto.toml (index 5): disabled but NOT "currently selected" even
  // though currentValue is "auto" too — and it still shows its real palette
  // (18 swatches), per the "surface, don't hide" decision.
  assert.equal(rows[5].classList.contains('is-disabled'), true);
  assert.equal(rows[5].classList.contains('is-selected'), false,
    'the reserved entry must never appear selected, even when its value matches the current value');
  assert.equal(swatchColors(rows[5]).length, 18, 'a reserved name that parses still shows its own colors');
  assert.equal(swatchColors(rows[5])[0], '#000000');

  console.log('buildTerminalThemePicker: DOM shape + exact swatch colors: ok');

  // --- broken entry is not selectable -------------------------------------
  rows[4].dispatchEvent({ type: 'click' });
  assert.equal(select.value, 'auto', 'clicking the broken row must not change the selection');
  assert.equal(onChangeCalls, 0, 'clicking the broken row must not fire onChange');

  // --- the reserved entry is not selectable either ------------------------
  rows[5].dispatchEvent({ type: 'click' });
  assert.equal(select.value, 'auto');
  assert.equal(onChangeCalls, 0);

  console.log('buildTerminalThemePicker: broken and reserved rows are inert: ok');

  // --- selection round-trips: click -> select.value -> onChange -----------
  rows[2].dispatchEvent({ type: 'click' }); // Gruvbox
  assert.equal(select.value, 'Gruvbox');
  assert.equal(onChangeCalls, 1);
  assert.equal(lastOnChangeValue, 'Gruvbox');
  assert.equal(rows[2].classList.contains('is-selected'), true);
  assert.equal(rows[0].classList.contains('is-selected'), false, 'the old selection (Auto) is cleared');

  console.log('buildTerminalThemePicker: selection round-trips via click: ok');
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

  // The row's control is the picker's own <select>, wired to the same
  // addRow/setRowTarget path every sibling settings row uses.
  const themeRow = container.querySelectorAll('.tl-settings__row').find(
    (row) => row.dataset.settingId === 'appearance:terminal-theme',
  );
  assert.ok(themeRow, 'a row is tagged appearance:terminal-theme');
  const select = themeRow.children[1];
  assert.equal(select.tagName, 'SELECT');
  assert.equal(select.options.length, 6, 'Auto + the 5 raw entries');

  // The palette-strip list renders as a sibling of the row, in the section.
  const pickerList = container.children.find((c) => c.className === 'tl-settings__theme-picker');
  assert.ok(pickerList, 'the palette-strip list is appended to the section');
  assert.equal(pickerList.children.length, 6);

  // --- selection round-trips into the saved settings payload --------------
  // (pendingSettings IS what actions.js's applySettings later serializes
  // and sends to save_settings — see store.js's isDirty()/
  // commitPendingAsOriginal() pair, which diff exactly this object.)
  const gruvboxRow = pickerList.children[2];
  assert.equal(gruvboxRow.children[0].children[0].textContent, 'Gruvbox');
  gruvboxRow.dispatchEvent({ type: 'click' });
  assert.equal(pendingSettings.colors.theme, 'Gruvbox',
    'selecting a theme writes colors.theme through the normal pendingSettings path');

  console.log('renderAppearance: wired end-to-end, selection round-trips into pendingSettings: ok');
}

console.log('ok');
