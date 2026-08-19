(function initTermLabSettingsSectionsAppearance(global) {
  'use strict';

  function renderAppearance(container, deps) {
    if (!container) return false;
    const d = deps || {};

    const pendingSettings = d.pendingSettings;
    if (!pendingSettings || !pendingSettings.termlab || !pendingSettings.window || !pendingSettings.colors) {
      return false;
    }

    const cachedTerminalThemes = Array.isArray(d.cachedTerminalThemes) ? d.cachedTerminalThemes : [];
    const cachedFonts = d.cachedFonts && typeof d.cachedFonts === 'object'
      ? d.cachedFonts
      : { all: [] };

    const addSectionLabel = typeof d.addSectionLabel === 'function' ? d.addSectionLabel : null;
    const addRow = typeof d.addRow === 'function' ? d.addRow : null;
    const setRowTarget = typeof d.setRowTarget === 'function' ? d.setRowTarget : null;
    const addDivider = typeof d.addDivider === 'function' ? d.addDivider : null;
    const themePicker = global.termlabSettingsThemePicker;
    const makeCheckbox = typeof d.makeCheckbox === 'function' ? d.makeCheckbox : null;
    const makeInput = typeof d.makeInput === 'function' ? d.makeInput : null;
    const makeToggleGroup = typeof d.makeToggleGroup === 'function' ? d.makeToggleGroup : null;
    if (!addSectionLabel || !addRow || !setRowTarget || !addDivider
      || !themePicker || typeof themePicker.normalizeThemeEntries !== 'function' || typeof themePicker.buildTerminalThemePicker !== 'function'
      || !makeCheckbox || !makeInput || !makeToggleGroup) {
      return false;
    }

    const heading = document.createElement('h3');
    heading.textContent = 'Appearance';
    container.appendChild(heading);

    addSectionLabel(container, 'Theme & Color');

    // Appearance Mode comes FIRST: it is the broader setting (it restyles the
    // whole app), and the Terminal Theme row below reads as a refinement of
    // it — its Auto entry is defined in terms of this row's value. Ordering
    // only; the two remain fully decoupled (product rule 1), and the search
    // index's jump targets (`appearance:mode`, `appearance:terminal-theme`)
    // are unchanged.
    const modeToggle = makeToggleGroup(
      [
        { label: 'Dark', value: 'Dark' },
        { label: 'Light', value: 'Light' },
        { label: 'System', value: 'System' },
      ],
      pendingSettings.colors.appearance_mode,
      (val) => { pendingSettings.colors.appearance_mode = val; }
    );
    setRowTarget(addRow(container, 'Appearance Mode', null, modeToggle), 'appearance:mode');

    // Terminal theme picker: Auto + built-ins + user themes
    // (~/.config/termlab/themes/*.toml), each entry carrying its own
    // palette_preview so every candidate renders a swatch strip without a
    // per-entry round trip (list_terminal_themes returns them all at once).
    // This REPLACES the old plain <select> + single "current selection"
    // preview panel (which re-fetched a theme-color preview per selection)
    // — see Task 3's report/review for why that stopgap existed and that it
    // was always meant to be superseded here (Task 5 removed the stopgap's
    // now-caller-less backend command entirely). Fully decoupled from app
    // appearance (product rule 1): this picks colors.theme, never
    // colors.appearance_mode above.
    const terminalThemeEntries = themePicker.normalizeThemeEntries(cachedTerminalThemes, pendingSettings.colors.theme);
    const { select: terminalThemeSelect, list: terminalThemeList } = themePicker.buildTerminalThemePicker(
      terminalThemeEntries,
      pendingSettings.colors.theme,
      (value) => { pendingSettings.colors.theme = value; }
    );
    const terminalThemeRow = addRow(
      container,
      'Terminal Theme',
      'Color palette for the terminal. Auto follows the app appearance above.',
      terminalThemeSelect
    );
    global.tlCombo.attach(terminalThemeSelect);
    setRowTarget(terminalThemeRow, 'appearance:terminal-theme');
    container.appendChild(terminalThemeList);

    addDivider(container);

    addSectionLabel(container, 'Notifications');

    const normalizedPosition = (pendingSettings.termlab.ui.notification_position || 'bottom').toLowerCase();
    const posToggle = makeToggleGroup(
      [
        { label: 'Bottom', value: 'bottom' },
        { label: 'Top', value: 'top' },
      ],
      normalizedPosition,
      (val) => { pendingSettings.termlab.ui.notification_position = val; }
    );
    setRowTarget(addRow(container, 'Notification Position', 'Where toast notifications appear on screen', posToggle), 'appearance:notification-position');

    const nativeCheckbox = makeCheckbox(
      pendingSettings.termlab.ui.native_notifications !== false,
      (val) => { pendingSettings.termlab.ui.native_notifications = val; }
    );
    setRowTarget(
      addRow(container, 'Native Notifications', 'Use system notifications when the app is not focused', nativeCheckbox),
      'appearance:native-notifications'
    );

    const animationsCheckbox = makeCheckbox(
      pendingSettings.termlab.ui.disable_animations !== true,
      (val) => { pendingSettings.termlab.ui.disable_animations = !val; }
    );
    setRowTarget(
      addRow(
        container,
        'Animations',
        'Enable UI motion and toast animations.',
        animationsCheckbox
      ),
      'appearance:animations'
    );

    addDivider(container);

    addSectionLabel(container, 'Interface Typography');

    const fontSelect = document.createElement('select');
    const defaultOpt = document.createElement('option');
    defaultOpt.value = '';
    defaultOpt.textContent = 'System Default';
    if (!pendingSettings.termlab.ui.font_family) defaultOpt.selected = true;
    fontSelect.appendChild(defaultOpt);

    for (const font of cachedFonts.all || []) {
      const opt = document.createElement('option');
      opt.value = font;
      opt.textContent = font;
      if (font === pendingSettings.termlab.ui.font_family) opt.selected = true;
      fontSelect.appendChild(opt);
    }
    fontSelect.addEventListener('change', () => {
      pendingSettings.termlab.ui.font_family = fontSelect.value;
    });
    const fontRow = addRow(container, 'UI Font Family', null, fontSelect);
    global.tlCombo.attach(fontSelect);
    setRowTarget(fontRow, 'appearance:ui-font-family');

    const sizeInput = makeInput('number', pendingSettings.termlab.ui.font_size, {
      min: '6',
      max: '72',
      step: '0.5',
    });
    sizeInput.addEventListener('change', () => {
      const value = parseFloat(sizeInput.value);
      if (!isNaN(value) && value > 0) pendingSettings.termlab.ui.font_size = value;
    });
    const sizeRow = addRow(container, 'UI Font Size', null, sizeInput);
    global.tlSpinner.attach(sizeInput);
    setRowTarget(sizeRow, 'appearance:ui-font-size');

    return true;
  }

  global.termlabSettingsSectionsAppearance = {
    renderAppearance,
  };
})(window);
