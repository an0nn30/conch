(function initConchSettingsSectionsAppearance(global) {
  'use strict';

  function renderAppearance(container, deps) {
    if (!container) return false;
    const d = deps || {};

    const pendingSettings = d.pendingSettings;
    if (!pendingSettings || !pendingSettings.conch || !pendingSettings.window || !pendingSettings.colors) {
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
    const makeSwitch = typeof d.makeSwitch === 'function' ? d.makeSwitch : null;

    if (!addSectionLabel || !addRow || !setRowTarget || !addDivider || !buildThemePreview || !updateThemePreview || !invoke || !makeSwitch) {
      return false;
    }

    const heading = document.createElement('h3');
    heading.textContent = 'Appearance';
    container.appendChild(heading);

    addSectionLabel(container, 'Theme & Color');

    const themeSelect = document.createElement('select');
    themeSelect.className = 'settings-select';
    for (const theme of cachedThemes) {
      const opt = document.createElement('option');
      opt.value = theme;
      opt.textContent = theme;
      if (theme === pendingSettings.colors.theme) opt.selected = true;
      themeSelect.appendChild(opt);
    }
    setRowTarget(addRow(container, 'Theme', 'Color theme for the terminal and UI', themeSelect), 'appearance:theme');

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

    const modes = ['Dark', 'Light', 'System'];
    const toggleGroup = document.createElement('div');
    toggleGroup.className = 'settings-toggle-group';
    for (const mode of modes) {
      const btn = document.createElement('button');
      btn.className = 'settings-toggle';
      if (pendingSettings.colors.appearance_mode === mode) btn.classList.add('active');
      btn.textContent = mode;
      btn.addEventListener('click', () => {
        pendingSettings.colors.appearance_mode = mode;
        for (const node of toggleGroup.querySelectorAll('.settings-toggle')) {
          node.classList.toggle('active', node.textContent === mode);
        }
      });
      toggleGroup.appendChild(btn);
    }
    setRowTarget(addRow(container, 'Appearance Mode', null, toggleGroup), 'appearance:mode');

    addDivider(container);

    addSectionLabel(container, 'Notifications');

    const posOptions = ['Bottom', 'Top'];
    const posGroup = document.createElement('div');
    posGroup.className = 'settings-toggle-group';
    for (const pos of posOptions) {
      const btn = document.createElement('button');
      btn.className = 'settings-toggle';
      if ((pendingSettings.conch.ui.notification_position || 'bottom').toLowerCase() === pos.toLowerCase()) {
        btn.classList.add('active');
      }
      btn.textContent = pos;
      btn.addEventListener('click', () => {
        pendingSettings.conch.ui.notification_position = pos.toLowerCase();
        for (const node of posGroup.querySelectorAll('.settings-toggle')) {
          node.classList.toggle('active', node.textContent === pos);
        }
      });
      posGroup.appendChild(btn);
    }
    setRowTarget(addRow(container, 'Notification Position', 'Where toast notifications appear on screen', posGroup), 'appearance:notification-position');

    const nativeSwitch = makeSwitch(
      pendingSettings.conch.ui.native_notifications !== false,
      (val) => { pendingSettings.conch.ui.native_notifications = val; }
    );
    setRowTarget(
      addRow(container, 'Native Notifications', 'Use system notifications when the app is not focused', nativeSwitch),
      'appearance:native-notifications'
    );

    const animationsSwitch = makeSwitch(
      pendingSettings.conch.ui.disable_animations === true,
      (val) => { pendingSettings.conch.ui.disable_animations = val; }
    );
    setRowTarget(
      addRow(
        container,
        'Disable Animations',
        'Turn off UI motion and toast animations for a snappier experience on lower-end machines.',
        animationsSwitch
      ),
      'appearance:disable-animations'
    );

    addDivider(container);

    addSectionLabel(container, 'Window Chrome');

    const decoOptions = ['Full', 'Transparent', 'Buttonless', 'None'];
    const decoSelect = document.createElement('select');
    decoSelect.className = 'settings-select';
    for (const deco of decoOptions) {
      const opt = document.createElement('option');
      opt.value = deco;
      opt.textContent = deco;
      if (deco === pendingSettings.window.decorations) opt.selected = true;
      decoSelect.appendChild(opt);
    }
    decoSelect.addEventListener('change', () => {
      pendingSettings.window.decorations = decoSelect.value;
    });
    setRowTarget(addRow(container, 'Window Decorations', 'Window title bar style', decoSelect), 'appearance:window-decorations');

    if (typeof navigator !== 'undefined' && navigator.platform.includes('Mac')) {
      const sw = document.createElement('label');
      sw.className = 'settings-switch';
      const cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.checked = !!pendingSettings.conch.ui.native_menu_bar;
      cb.addEventListener('change', () => {
        pendingSettings.conch.ui.native_menu_bar = cb.checked;
      });
      const slider = document.createElement('span');
      slider.className = 'slider';
      sw.appendChild(cb);
      sw.appendChild(slider);
      setRowTarget(addRow(container, 'Native Menu Bar', 'Use the system menu bar instead of in-app menu', sw), 'appearance:native-menu-bar');
    }

    addDivider(container);

    addSectionLabel(container, 'Interface Typography');

    const fontSelect = document.createElement('select');
    fontSelect.className = 'settings-select';
    const defaultOpt = document.createElement('option');
    defaultOpt.value = '';
    defaultOpt.textContent = 'System Default';
    if (!pendingSettings.conch.ui.font_family) defaultOpt.selected = true;
    fontSelect.appendChild(defaultOpt);

    for (const font of cachedFonts.all || []) {
      const opt = document.createElement('option');
      opt.value = font;
      opt.textContent = font;
      if (font === pendingSettings.conch.ui.font_family) opt.selected = true;
      fontSelect.appendChild(opt);
    }
    fontSelect.addEventListener('change', () => {
      pendingSettings.conch.ui.font_family = fontSelect.value;
    });
    setRowTarget(addRow(container, 'UI Font Family', null, fontSelect), 'appearance:ui-font-family');

    const sizeInput = document.createElement('input');
    sizeInput.type = 'number';
    sizeInput.className = 'settings-input';
    sizeInput.style.width = '70px';
    sizeInput.value = pendingSettings.conch.ui.font_size;
    sizeInput.min = '6';
    sizeInput.max = '72';
    sizeInput.step = '0.5';
    sizeInput.addEventListener('change', () => {
      const value = parseFloat(sizeInput.value);
      if (!isNaN(value) && value > 0) pendingSettings.conch.ui.font_size = value;
    });
    setRowTarget(addRow(container, 'UI Font Size', null, sizeInput), 'appearance:ui-font-size');

    return true;
  }

  global.conchSettingsSectionsAppearance = {
    renderAppearance,
  };
})(window);
