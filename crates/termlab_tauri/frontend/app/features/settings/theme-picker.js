// Terminal theme picker — turns list_terminal_themes()'s raw payload into the
// Appearance section's "Terminal Theme" row: ONE control, a normal <select>
// (the existing tl-combo half every sibling settings row uses), plus a
// fake-terminal preview box rendered below the row showing the currently
// selected palette.
//
// The picker used to render a second, parallel half — a visible list of
// palette-strip rows, one per entry. That duplicated the combo's own job,
// made this row look unlike every other settings row, and showed sixteen
// 10px swatches per candidate in place of anything resembling a terminal.
// Entries that can't be picked (broken files, a reserved `auto.toml`, an
// unmatched saved name) keep every bit of their old meaning — they are
// disabled <option>s whose label carries the explanation the strip row's
// note used to.
//
// Split into two functions on purpose:
//   - normalizeThemeEntries: pure, DOM-free. Takes list_terminal_themes()'s
//     untagged union ({name,source,palettePreview,shadowsBuiltin} | {name,
//     error}) plus the synthetic Auto entry (not part of that command's
//     payload — colors.theme's reserved value is a frontend/config concept,
//     see termlab_core::effective_theme) and produces one ordered,
//     render-ready descriptor per entry. Independently unit-testable.
//   - buildTerminalThemePicker: DOM-only, consumes that descriptor array.
(function initTermLabSettingsThemePicker(global) {
  'use strict';

  // Mirrors termlab_core::effective_theme::AUTO_THEME_NAME. Not imported
  // (no module system between Rust and this plain-script frontend) —
  // colors.theme's reserved-name contract, so a literal copy here is the
  // same trade every other cross-boundary constant in this codebase makes.
  const AUTO_VALUE = 'auto';

  // Mirrors termlab_core::effective_theme::TERMLAB_{DARK,LIGHT}_THEME — the
  // two built-ins `auto` picks between, and (for TermLab Dark) the palette
  // ColorScheme::default() falls back to. Same literal-copy trade as above.
  const TERMLAB_DARK_THEME = 'TermLab Dark';
  const TERMLAB_LIGHT_THEME = 'TermLab Light';

  function isReservedName(name) {
    return typeof name === 'string' && name.trim().toLowerCase() === AUTO_VALUE;
  }

  function isBrokenEntry(entry) {
    return entry && typeof entry.error === 'string';
  }

  /**
   * F1b/F4 (branch-review.md): a picker built from `entries` alone renders a
   * blank combo whenever `pendingSettings.colors.theme` names something that
   * isn't among them — a deleted user theme, or a hand-edited config naming
   * a theme that was never installed. A synthesized `missing` entry keeps
   * the combo showing the saved name (not blank) and keeps it selected,
   * while leaving every other entry exactly as selectable as before —
   * switching away is still one pick in the dropdown.
   */
  function synthesizeMissingEntry(currentValue) {
    return {
      kind: 'missing',
      name: currentValue,
      value: currentValue,
      label: currentValue,
      selectable: false,
      source: null,
      shadowsBuiltin: false,
      palettePreview: null,
      error: null,
      note: '(missing)',
    };
  }

  /**
   * Normalize list_terminal_themes()'s raw entries (plus the synthetic Auto
   * entry, always first, and — when `currentValue` matches none of them — a
   * synthesized `missing` entry last, see `synthesizeMissingEntry`) into
   * descriptors of the shape:
   *   { kind: 'auto'|'parsed'|'reserved'|'broken'|'missing', name, value,
   *     label, selectable, source, shadowsBuiltin, palettePreview, error,
   *     note }
   *
   * `auto.toml` decision (Task 3 review Low #2 — a user theme file literally
   * named `auto.toml` is enumerable but unreachable, because
   * effective_theme::effective_theme_name intercepts the reserved name
   * "auto" case-insensitively BEFORE the file lookup ever runs — selecting
   * it could only ever resolve to the real Auto entry above, never to that
   * file's own colors). This picker's own established precedent — carried
   * over verbatim from the Broken variant's design intent ("surfaced rather
   * than silently skipped... grey them out... instead of hiding them",
   * termlab_core::color_scheme::ThemeListEntry's doc comment) — is to show
   * unusable entries with an explanation rather than make the file
   * disappear with no trace, which would leave a user staring at a picker
   * that silently dropped a file they placed in the themes directory. So a
   * name that is reserved (case-insensitively "auto") always renders as its
   * own `reserved` kind: greyed, `selectable: false`, carrying a note that
   * explains why — whether or not the file also happens to parse (checked
   * before the broken/parsed split below, so a broken `auto.toml` is
   * classified `reserved`, not `broken`, and never collides with the real
   * Auto entry's value).
   */
  function normalizeThemeEntries(rawEntries, currentValue) {
    const out = [{
      kind: 'auto',
      name: 'Auto',
      value: AUTO_VALUE,
      label: 'Auto (follows appearance)',
      selectable: true,
      source: null,
      shadowsBuiltin: false,
      palettePreview: null,
      error: null,
      note: 'Follows the app appearance (Dark/Light/System).',
    }];

    for (const entry of (Array.isArray(rawEntries) ? rawEntries : [])) {
      if (!entry || typeof entry.name !== 'string') continue;
      const broken = isBrokenEntry(entry);

      if (isReservedName(entry.name)) {
        out.push({
          kind: 'reserved',
          name: entry.name,
          value: entry.name,
          label: entry.name,
          selectable: false,
          source: broken ? null : (entry.source || null),
          shadowsBuiltin: broken ? false : !!entry.shadowsBuiltin,
          palettePreview: broken ? null : (entry.palettePreview || null),
          error: broken ? entry.error : null,
          note: broken
            ? 'Reserved name, ignored — also fails to parse.'
            : 'Reserved name, ignored (would resolve to Auto). Rename this file to use it.',
        });
        continue;
      }

      if (broken) {
        out.push({
          kind: 'broken',
          name: entry.name,
          value: entry.name,
          label: entry.name,
          selectable: false,
          source: null,
          shadowsBuiltin: false,
          palettePreview: null,
          error: entry.error,
          note: null,
        });
        continue;
      }

      out.push({
        kind: 'parsed',
        name: entry.name,
        value: entry.name,
        label: entry.name,
        selectable: true,
        source: entry.source || null,
        shadowsBuiltin: !!entry.shadowsBuiltin,
        palettePreview: entry.palettePreview || null,
        error: null,
        note: entry.shadowsBuiltin ? 'Overrides built-in' : null,
      });
    }

    // F1b/F4: a reserved-cased currentValue ("Auto", "AUTO", ...) always
    // resolves to the real Auto entry above (see buildTerminalThemePicker's
    // own case-insensitive fold when it assigns select.value) — never
    // synthesized here, and never counted as "no match".
    if (
      typeof currentValue === 'string' && currentValue.length > 0
      && !isReservedName(currentValue)
      && !out.some((entry) => entry.value === currentValue)
    ) {
      out.push(synthesizeMissingEntry(currentValue));
    }

    return out;
  }

  // Which entry `value` currently names. 'reserved' entries are excluded
  // even when their value happens to equal the real Auto entry's value
  // ("auto", for a file literally named auto.toml) — otherwise the picker
  // would read the reserved file's palette as the current selection whenever
  // colors.theme is "auto", the default. Finding the FIRST non-reserved
  // match is exactly what a native <select>.value assignment does with the
  // two same-valued options (the real Auto entry is always prepended).
  function currentEntry(entries, value) {
    return entries.find((entry) => entry.kind !== 'reserved' && entry.value === value) || null;
  }

  /**
   * The text an entry gets as a dropdown option. This is where the deleted
   * palette-strip rows' per-entry note lived; folding it into the option
   * label is what lets a single combo carry every kind's meaning:
   *   - reserved: the "why this can never be picked" note (checked first, so
   *     a broken `auto.toml` explains its reservation rather than its parse
   *     error — the same precedence normalizeThemeEntries applies).
   *   - broken:   the actual parse error, not a generic "invalid".
   *   - missing:  the saved-but-unresolvable name, marked so it doesn't read
   *     as a theme that exists.
   *   - shadowsBuiltin: marked so a user file overriding a built-in is
   *     visibly not the built-in.
   */
  function optionLabel(entry) {
    if (entry.kind === 'reserved') {
      return entry.note ? entry.label + ' — ' + entry.note : entry.label;
    }
    if (entry.error) return entry.label + ' — Parse error: ' + entry.error;
    if (entry.kind === 'missing') return entry.label + ' (missing)';
    if (entry.shadowsBuiltin) return entry.label + ' (overrides built-in)';
    return entry.label;
  }

  // --- Theme preview ------------------------------------------------------
  // Recovered from the pre-Task-4 renderers.js (buildThemePreview/
  // updateThemePreview and their line/span helpers) and its
  // .tl-settings__theme-preview* CSS. One deliberate change: it is driven
  // entirely client-side from the picker entry's own `palettePreview`
  // payload (bg + fg + ansi[16], already in list_terminal_themes()'s
  // response), NOT from the retired `preview_theme_colors` Tauri command —
  // no backend round trip per selection, and nothing to resurrect on the
  // Rust side.
  //
  // Colors are DATA (an arbitrary user-supplied palette), so they cannot
  // live in the design-system stylesheet. They are set as inline CUSTOM
  // PROPERTIES on the box (--tp-*), which the structural CSS then reads via
  // var() — so the stylesheet stays token-only and the boundary script's
  // hex scan has nothing to flag, while the DOM below stays class-driven.

  // ANSI slots 0-15 in payload order (normal 0-7, then bright 8-15), mapped
  // onto the custom property each swatch/text class reads.
  const TP_ANSI_VARS = [
    '--tp-black', '--tp-red', '--tp-green', '--tp-yellow',
    '--tp-blue', '--tp-magenta', '--tp-cyan', '--tp-white',
    '--tp-bright-black', '--tp-bright-red', '--tp-bright-green', '--tp-bright-yellow',
    '--tp-bright-blue', '--tp-bright-magenta', '--tp-bright-cyan', '--tp-bright-white',
  ];

  function span(cls, text) {
    const s = document.createElement('span');
    if (cls) s.className = cls;
    s.textContent = text;
    return s;
  }

  function line(...nodes) {
    const d = document.createElement('div');
    for (const n of nodes) {
      if (typeof n === 'string') d.appendChild(document.createTextNode(n));
      else d.appendChild(n);
    }
    return d;
  }

  function buildThemePreview() {
    const box = document.createElement('div');
    box.className = 'tl-settings__theme-preview';

    const label = document.createElement('div');
    label.textContent = 'PREVIEW';
    label.className = 'tl-settings__theme-preview-label tp-dim';
    box.appendChild(label);

    const prompt = () => [
      span('tp-green tp-bold', 'user@termlab'),
      span('tp-fg', ':'),
      span('tp-blue tp-bold', '~/projects'),
      span('tp-fg', ' $ '),
    ];

    box.appendChild(line(...prompt(), span('tp-fg', 'ls -la')));
    box.appendChild(line(span('tp-fg', 'total 42')));

    // [permissions, links, user, group, size, date, name, nameClass]
    const entries = [
      ['drwxr-xr-x', '5', 'user', 'staff', '160', 'Mar 20 10:01', '.', 'tp-blue tp-bold'],
      ['drwxr-xr-x', '8', 'user', 'staff', '256', 'Mar 19 09:00', '..', 'tp-blue tp-bold'],
      ['-rw-r--r--', '1', 'user', 'staff', '1234', 'Mar 20 10:01', '.gitignore', 'tp-yellow'],
      ['-rw-r--r--', '1', 'user', 'staff', '890', 'Mar 20 10:01', '.env', 'tp-yellow'],
      ['drwxr-xr-x', '3', 'user', 'staff', '96', 'Mar 20 10:01', 'src', 'tp-blue tp-bold'],
      ['-rwxr-xr-x', '1', 'user', 'staff', '8192', 'Mar 20 10:01', 'build.sh', 'tp-red tp-bold'],
      ['-rw-r--r--', '1', 'user', 'staff', '512', 'Mar 20 10:01', 'config.toml', 'tp-green'],
      ['-rw-r--r--', '1', 'user', 'staff', '256', 'Mar 20 10:01', 'README.md', 'tp-fg'],
    ];
    for (const [perm, links, user, group, size, date, name, nameClass] of entries) {
      box.appendChild(line(
        span('tp-dim', perm + ' '),
        span('tp-cyan', links + ' '),
        span('tp-dim', user + ' ' + group + ' '),
        span('tp-cyan', size.padStart(6) + ' '),
        span('tp-dim', date + ' '),
        span(nameClass, name),
      ));
    }

    box.appendChild(line(
      ...prompt(),
      span('tp-magenta', 'echo'),
      span('tp-fg', ' '),
      span('tp-yellow', '"hello world"'),
    ));
    box.appendChild(line(span('tp-fg', 'hello world')));

    const cursorLine = line(...prompt());
    const cursor = document.createElement('span');
    cursor.className = 'tl-settings__theme-preview-cursor';
    cursor.textContent = ' ';
    cursorLine.appendChild(cursor);
    box.appendChild(cursorLine);

    const dividerEl = document.createElement('div');
    dividerEl.className = 'tl-settings__theme-preview-divider';
    box.appendChild(dividerEl);

    const rows = [
      ['tl-settings__theme-preview-swatches tl-settings__theme-preview-swatches--normal',
        ['tp-sw-black', 'tp-sw-red', 'tp-sw-green', 'tp-sw-yellow',
          'tp-sw-blue', 'tp-sw-magenta', 'tp-sw-cyan', 'tp-sw-white']],
      ['tl-settings__theme-preview-swatches',
        ['tp-sw-bright-black', 'tp-sw-bright-red', 'tp-sw-bright-green', 'tp-sw-bright-yellow',
          'tp-sw-bright-blue', 'tp-sw-bright-magenta', 'tp-sw-bright-cyan', 'tp-sw-bright-white']],
    ];
    for (const [rowClass, swatchClasses] of rows) {
      const row = document.createElement('div');
      row.className = rowClass;
      for (const cls of swatchClasses) {
        const sw = document.createElement('div');
        sw.className = cls + ' tl-settings__theme-preview-swatch';
        row.appendChild(sw);
      }
      box.appendChild(row);
    }

    return box;
  }

  /**
   * Paint `palette` (a list_terminal_themes palettePreview: {bg, fg,
   * ansi[16]}) onto a box built by buildThemePreview, by setting the --tp-*
   * custom properties its CSS reads.
   */
  function updateThemePreview(box, palette) {
    if (!box || !palette) return;
    const ansi = Array.isArray(palette.ansi) ? palette.ansi : [];
    if (palette.bg) box.style.setProperty('--tp-bg', palette.bg);
    if (palette.fg) box.style.setProperty('--tp-fg', palette.fg);
    for (let i = 0; i < TP_ANSI_VARS.length; i += 1) {
      if (ansi[i]) box.style.setProperty(TP_ANSI_VARS[i], ansi[i]);
    }
    // The payload carries no dim_foreground (PalettePreview is bg + fg + the
    // 16 ANSI slots), so dim text, the container border and the swatch
    // divider all key off bright black — the conventional stand-in, and the
    // one palette entry guaranteed to sit between bg and fg in both a light
    // and a dark theme.
    const dim = ansi[8] || palette.fg;
    if (dim) box.style.setProperty('--tp-dim', dim);
  }

  // The app's resolved appearance ('dark' | 'light'), which is what `auto`
  // follows. Same convention as termlab_core::effective_theme: 'light' is
  // the only affirmative light answer, anything unresolvable is dark.
  function resolvedAppearance() {
    const appearance = global.termlabAppearance;
    if (appearance && typeof appearance.current === 'function') {
      return String(appearance.current() || '').trim().toLowerCase() === 'light' ? 'light' : 'dark';
    }
    return 'dark';
  }

  /**
   * The palette the preview box should show for `entry`.
   *
   * - `auto`: whichever built-in it would resolve to right now — both ship
   *   in the frontend `themes/` dir, so both are already in `entries` with
   *   their own palettePreview. No round trip, no second source of truth.
   * - anything with its own palettePreview: that palette, verbatim.
   * - the synthesized `missing` entry: TermLab Dark's palette, because that
   *   is genuinely what the terminal is showing — resolve_theme falls back
   *   to ColorScheme::default() for an unmatched name, and that Default impl
   *   is pinned byte-identical to `themes/TermLab Dark.toml`
   *   (color_scheme::tests::resolve_theme_termlab_dark_is_byte_identical_to_the_hardcoded_default).
   *   Broken/reserved entries are disabled options and so can never be the
   *   selection, but they take the same path if they somehow are.
   */
  function previewPaletteFor(entries, entry) {
    const builtin = (name) => {
      const found = entries.find((e) => e.value === name && e.palettePreview);
      return found ? found.palettePreview : null;
    };
    if (!entry) return builtin(TERMLAB_DARK_THEME);
    if (entry.kind === 'auto') {
      return builtin(resolvedAppearance() === 'light' ? TERMLAB_LIGHT_THEME : TERMLAB_DARK_THEME)
        || builtin(TERMLAB_DARK_THEME);
    }
    return entry.palettePreview || builtin(TERMLAB_DARK_THEME);
  }

  /**
   * Build the row's control (a <select>, meant for global.tlCombo.attach())
   * and the preview box that sits under it in the section.
   * `onChange(value)` fires once per committed selection.
   */
  function buildTerminalThemePicker(entries, currentValue, onChange) {
    const select = document.createElement('select');
    const preview = buildThemePreview();

    for (const entry of entries) {
      const opt = document.createElement('option');
      opt.value = entry.value;
      opt.textContent = optionLabel(entry);
      opt.disabled = !entry.selectable;
      select.appendChild(opt);
    }

    function refresh() {
      updateThemePreview(preview, previewPaletteFor(entries, currentEntry(entries, select.value)));
    }

    // F4: a hand-edited config can spell the reserved name with any casing
    // ("Auto", "AUTO", ...) — effective_theme_name matches it
    // case-insensitively on the Rust side (same fold as isReservedName
    // above), so the picker must land on the real Auto <option> (value
    // AUTO_VALUE, always lowercase) rather than trying to match the literal
    // typed casing against no option at all.
    //
    // F1b: every OTHER currentValue that names no entry now has a
    // synthesized `missing` <option> of its own (see
    // normalizeThemeEntries/synthesizeMissingEntry) with `value ===
    // currentValue`, so this assignment always lands on a real option — the
    // combo can no longer render blank (selectedIndex -1).
    select.value = isReservedName(currentValue) ? AUTO_VALUE : currentValue;
    select.addEventListener('change', () => {
      refresh();
      if (typeof onChange === 'function') onChange(select.value);
    });
    refresh();

    return { select, preview, refresh };
  }


  global.termlabSettingsThemePicker = {
    normalizeThemeEntries,
    buildTerminalThemePicker,
    optionLabel,
    buildThemePreview,
    updateThemePreview,
  };
})(window);
