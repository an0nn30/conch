// Settings → Window.
//
// Everything about the window itself, gathered from where it used to be
// scattered: the default size lived under Advanced ("Window Defaults") and the
// chrome rows under Appearance ("Window Chrome"), which meant three places to
// look for one topic.
//
// Follows the same shape as the sibling section modules: a render function
// returning truthy when it handled the container, so renderers.js can report a
// missing module rather than silently drawing nothing.
(function initTermLabSettingsSectionsWindow(global) {
  'use strict';

  function renderWindow(container, d) {
    const addSectionLabel = d.addSectionLabel || function () {};
    const addRow = d.addRow;
    const addDivider = d.addDivider || function () {};
    const setRowTarget = d.setRowTarget || function () {};
    const makeInput = d.makeInput;
    const makeCheckbox = d.makeCheckbox;
    const pendingSettings = d.pendingSettings;

    if (!addRow || !makeInput || !makeCheckbox || !pendingSettings) return false;

    addSectionLabel(container, 'Default Size');

    const colsInput = makeInput('number', pendingSettings.window.dimensions.columns);
    colsInput.addEventListener('input', () => {
      const value = parseInt(colsInput.value, 10);
      if (!isNaN(value)) pendingSettings.window.dimensions.columns = value;
    });
    setRowTarget(
      addRow(container, 'Columns', 'Width in character cells (0 = leave it to the system)', colsInput),
      'window:columns'
    );
    if (global.tlSpinner) global.tlSpinner.attach(colsInput);

    const linesInput = makeInput('number', pendingSettings.window.dimensions.lines);
    linesInput.addEventListener('input', () => {
      const value = parseInt(linesInput.value, 10);
      if (!isNaN(value)) pendingSettings.window.dimensions.lines = value;
    });
    setRowTarget(
      addRow(container, 'Lines', 'Height in character cells (0 = leave it to the system)', linesInput),
      'window:lines'
    );
    if (global.tlSpinner) global.tlSpinner.attach(linesInput);

    addDivider(container);
    addSectionLabel(container, 'Window Chrome');

    const decoOptions = ['Full', 'Transparent', 'Buttonless', 'None'];
    const decoSelect = document.createElement('select');
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
    const decoRow = addRow(container, 'Window Decorations', 'Window title bar style', decoSelect);
    if (global.tlCombo) global.tlCombo.attach(decoSelect);
    setRowTarget(decoRow, 'window:decorations');

    const zenNewWindowCheckbox = makeCheckbox(
      pendingSettings.window.new_window_zen_mode !== false,
      (val) => { pendingSettings.window.new_window_zen_mode = val; }
    );
    setRowTarget(
      addRow(
        container,
        'New windows open in zen mode',
        'Extra windows (⇧⌘N) start with the panels hidden, whatever this window shows',
        zenNewWindowCheckbox
      ),
      'window:new-window-zen-mode'
    );

    if (typeof navigator !== 'undefined' && navigator.platform.includes('Mac')) {
      const menuBarCheckbox = makeCheckbox(
        !!pendingSettings.termlab.ui.native_menu_bar,
        (val) => { pendingSettings.termlab.ui.native_menu_bar = val; }
      );
      setRowTarget(
        addRow(container, 'Native Menu Bar', 'Use the system menu bar instead of in-app menu', menuBarCheckbox),
        'window:native-menu-bar'
      );
    }

    return true;
  }

  global.termlabSettingsSectionsWindow = { renderWindow };
})(window);
