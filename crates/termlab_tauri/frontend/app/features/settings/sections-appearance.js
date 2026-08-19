(function initTermLabSettingsSectionsAppearance(global) {
  'use strict';

  function renderAppearance(container, deps) {
    if (!container) return false;
    const d = deps || {};

    const pendingSettings = d.pendingSettings;
    if (!pendingSettings || !pendingSettings.termlab || !pendingSettings.window || !pendingSettings.colors) {
      return false;
    }

    const cachedThemes = Array.isArray(d.cachedThemes) ? d.cachedThemes : [];
    const cachedFonts = d.cachedFonts && typeof d.cachedFonts === 'object'
      ? d.cachedFonts
      : { all: [] };

    const addSectionLabel = typeof d.addSectionLabel === 'function' ? d.addSectionLabel : null;
    const addRow = typeof d.addRow === 'function' ? d.addRow : null;
    const setRowTarget = typeof d.setRowTarget === 'function' ? d.setRowTarget : null;
    const addDivider = typeof d.addDivider === 'function' ? d.addDivider : null;
    const buildThemePreview = typeof d.buildThemePreview === 'function' ? d.buildThemePreview : null;
    const updateThemePreview = typeof d.updateThemePreview === 'function' ? d.updateThemePreview : null;
    const invoke = typeof d.invoke === 'function' ? d.invoke : null;
    const makeCheckbox = typeof d.makeCheckbox === 'function' ? d.makeCheckbox : null;
    const makeInput = typeof d.makeInput === 'function' ? d.makeInput : null;
    const makeToggleGroup = typeof d.makeToggleGroup === 'function' ? d.makeToggleGroup : null;
    if (!addSectionLabel || !addRow || !setRowTarget || !addDivider || !buildThemePreview || !updateThemePreview || !invoke || !makeCheckbox || !makeInput || !makeToggleGroup) {
      return false;
    }

    const heading = document.createElement('h3');
    heading.textContent = 'Appearance';
    container.appendChild(heading);

    addSectionLabel(container, 'Theme & Color');

    const themeSelect = document.createElement('select');
    for (const theme of cachedThemes) {
      const opt = document.createElement('option');
      opt.value = theme;
      opt.textContent = theme;
      if (theme === pendingSettings.colors.theme) opt.selected = true;
      themeSelect.appendChild(opt);
    }
    const themeRow = addRow(container, 'Theme', 'Color theme for the terminal and UI', themeSelect);
    global.tlCombo.attach(themeSelect);
    setRowTarget(themeRow, 'appearance:theme');

    const previewBox = buildThemePreview();
    container.appendChild(previewBox);

    let previewSeq = 0;
    invoke('preview_theme_colors', { name: pendingSettings.colors.theme })
      .then((tc) => updateThemePreview(previewBox, tc))
      .catch(() => {});

    themeSelect.addEventListener('change', () => {
      pendingSettings.colors.theme = themeSelect.value;
      const seq = ++previewSeq;
      invoke('preview_theme_colors', { name: themeSelect.value })
        .then((tc) => {
          if (seq === previewSeq) updateThemePreview(previewBox, tc);
        })
        .catch(() => {});
    });

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
