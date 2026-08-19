// Terminal theme picker (Task 4 of the terminal-themes plan) — turns
// list_terminal_themes()'s raw payload into the Appearance section's
// "Terminal Theme" row: a normal <select> (the existing tl-combo half every
// other settings row uses) paired with a visible list of palette-strip rows,
// one per entry, so every candidate is comparable without opening the
// dropdown. Both halves are built from the same ordered `entries` array so
// they can never drift out of sync with each other.
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

  function isReservedName(name) {
    return typeof name === 'string' && name.trim().toLowerCase() === AUTO_VALUE;
  }

  function isBrokenEntry(entry) {
    return entry && typeof entry.error === 'string';
  }

  /**
   * Normalize list_terminal_themes()'s raw entries (plus the synthetic Auto
   * entry, always first) into descriptors of the shape:
   *   { kind: 'auto'|'parsed'|'reserved'|'broken', name, value, label,
   *     selectable, source, shadowsBuiltin, palettePreview, error, note }
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
  function normalizeThemeEntries(rawEntries) {
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

    return out;
  }

  // 'reserved' entries are excluded even when their value happens to equal
  // the real Auto entry's value ("auto", for a file literally named
  // auto.toml) — otherwise both rows would render as "currently selected"
  // whenever colors.theme is "auto", the default.
  function isCurrent(entry, currentValue) {
    return entry.kind !== 'reserved' && entry.value === currentValue;
  }

  function appendSwatch(parent, className, color) {
    const el = document.createElement('span');
    el.className = className;
    // Data-driven color, not a design token: only inline style carries a
    // parsed theme's actual colors (see the .tl-settings__theme-picker*
    // rules' own comment in components/settings.css for the structural-only
    // CSS this pairs with), so the boundary script's hex scan (which only
    // checks stylesheets) has nothing to flag here.
    if (color) el.style.background = color;
    parent.appendChild(el);
    return el;
  }

  /**
   * Build the row's control (a <select>, meant for global.tlCombo.attach())
   * and the palette-strip list that sits beside/under it in the DOM.
   * `onChange(value)` fires once per committed selection, whether it came
   * from clicking a row or a native <select> interaction — both paths route
   * through the same select.value + dispatchEvent('change'), exactly how
   * tl-combo.js's own popup selection commits (app/ui/tl-combo.js).
   */
  function buildTerminalThemePicker(entries, currentValue, onChange) {
    const select = document.createElement('select');
    const list = document.createElement('div');
    list.className = 'tl-settings__theme-picker';
    list.setAttribute('role', 'radiogroup');
    list.setAttribute('aria-label', 'Terminal theme');

    const rows = [];

    for (const entry of entries) {
      const opt = document.createElement('option');
      opt.value = entry.value;
      opt.textContent = entry.label;
      opt.disabled = !entry.selectable;
      select.appendChild(opt);

      const row = document.createElement('div');
      row.className = 'tl-settings__theme-picker-row' + (entry.selectable ? '' : ' is-disabled');
      row.dataset.themeName = entry.name;
      row.dataset.themeKind = entry.kind;
      row.setAttribute('role', 'radio');
      if (entry.selectable) row.tabIndex = 0;

      const info = document.createElement('div');
      info.className = 'tl-settings__theme-picker-info';
      const nameEl = document.createElement('span');
      nameEl.className = 'tl-settings__theme-picker-name';
      nameEl.textContent = entry.label;
      info.appendChild(nameEl);
      const noteText = entry.error ? ('Parse error: ' + entry.error) : entry.note;
      if (noteText) {
        const noteEl = document.createElement('span');
        noteEl.className = 'tl-settings__theme-picker-note' + (entry.error ? ' is-error' : '');
        noteEl.textContent = noteText;
        info.appendChild(noteEl);
      }
      row.appendChild(info);

      const swatches = document.createElement('div');
      swatches.className = 'tl-settings__theme-picker-swatches';
      if (entry.palettePreview) {
        appendSwatch(swatches, 'tl-settings__theme-picker-chip', entry.palettePreview.bg);
        appendSwatch(swatches, 'tl-settings__theme-picker-chip', entry.palettePreview.fg);
        for (const color of entry.palettePreview.ansi) {
          appendSwatch(swatches, 'tl-settings__theme-picker-swatch', color);
        }
      }
      row.appendChild(swatches);

      if (entry.selectable) {
        const activate = () => {
          select.value = entry.value;
          select.dispatchEvent(new Event('change', { bubbles: true }));
        };
        row.addEventListener('click', activate);
        row.addEventListener('keydown', (event) => {
          if (event.key !== 'Enter' && event.key !== ' ') return;
          event.preventDefault();
          activate();
        });
      }

      list.appendChild(row);
      rows.push({ entry, row });
    }

    function refresh() {
      for (const { entry, row } of rows) {
        const current = isCurrent(entry, select.value);
        row.classList.toggle('is-selected', current);
        row.setAttribute('aria-checked', current ? 'true' : 'false');
      }
    }

    // A currentValue with no matching <option> (e.g. a theme file deleted
    // after being selected) leaves the native select on whatever it
    // defaults to; nothing here rewrites pendingSettings until the user
    // actually picks something — matching the pre-existing plain <select>
    // this replaces, which had the same gap.
    select.value = currentValue;
    select.addEventListener('change', () => {
      refresh();
      if (typeof onChange === 'function') onChange(select.value);
    });
    refresh();

    return { select, list, refresh };
  }

  global.termlabSettingsThemePicker = {
    normalizeThemeEntries,
    buildTerminalThemePicker,
  };
})(window);
