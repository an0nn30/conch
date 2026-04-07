(function initConchSettingsSectionsBasic(global) {
  'use strict';

  function renderAdvanced(container, deps) {
    const d = deps || {};
    const pendingSettings = d.pendingSettings;
    if (!container || !pendingSettings) return;

    const addSectionLabel = d.addSectionLabel || function () {};
    const addDivider = d.addDivider || function () {};
    const addRow = d.addRow || function () {};
    const setRowTarget = d.setRowTarget || function (row) { return row; };
    const makeSwitch = d.makeSwitch || function () { return document.createElement('span'); };
    const makeInput = d.makeInput || function () { return document.createElement('input'); };

    const h = document.createElement('h3');
    h.textContent = 'Advanced';
    container.appendChild(h);

    addSectionLabel(container, 'Startup & Updates');

    const updateSwitch = makeSwitch(
      pendingSettings.conch.check_for_updates !== false,
      (val) => { pendingSettings.conch.check_for_updates = val; }
    );
    setRowTarget(
      addRow(
        container,
        'Check for Updates',
        'Automatically check for new versions when the app starts (macOS and Windows)',
        updateSwitch
      ),
      'advanced:check-for-updates'
    );

    addDivider(container);

    addSectionLabel(container, 'Window Defaults');
    const windowDefaultsAnchor = document.createElement('div');
    windowDefaultsAnchor.dataset.settingId = 'advanced:window-size';
    container.appendChild(windowDefaultsAnchor);

    const colsInput = makeInput('number', pendingSettings.window.dimensions.columns);
    colsInput.addEventListener('input', () => {
      const value = parseInt(colsInput.value, 10);
      if (!isNaN(value)) pendingSettings.window.dimensions.columns = value;
    });
    addRow(container, 'Columns', 'Width in character cells (0 = system default)', colsInput);

    const linesInput = makeInput('number', pendingSettings.window.dimensions.lines);
    linesInput.addEventListener('input', () => {
      const value = parseInt(linesInput.value, 10);
      if (!isNaN(value)) pendingSettings.window.dimensions.lines = value;
    });
    addRow(container, 'Lines', 'Height in character cells (0 = system default)', linesInput);

    addDivider(container);

    addSectionLabel(container, 'Interface Density');
    const densityAnchor = document.createElement('div');
    densityAnchor.dataset.settingId = 'advanced:ui-chrome-font-sizes';
    container.appendChild(densityAnchor);

    const fontNote = document.createElement('div');
    fontNote.className = 'settings-row-desc';
    fontNote.style.marginBottom = '8px';
    fontNote.textContent = 'Fine-tune text sizes for different UI elements (in points)';
    container.appendChild(fontNote);

    const smallInput = makeInput('number', pendingSettings.conch.ui.font.small, { step: '0.5' });
    smallInput.addEventListener('input', () => {
      const value = parseFloat(smallInput.value);
      if (!isNaN(value)) pendingSettings.conch.ui.font.small = value;
    });
    addRow(container, 'Small', 'Tab titles, badges, compact labels', smallInput);

    const listInput = makeInput('number', pendingSettings.conch.ui.font.list, { step: '0.5' });
    listInput.addEventListener('input', () => {
      const value = parseFloat(listInput.value);
      if (!isNaN(value)) pendingSettings.conch.ui.font.list = value;
    });
    addRow(container, 'List', 'Tree nodes, table rows, file explorer', listInput);

    const normalInput = makeInput('number', pendingSettings.conch.ui.font.normal, { step: '0.5' });
    normalInput.addEventListener('input', () => {
      const value = parseFloat(normalInput.value);
      if (!isNaN(value)) pendingSettings.conch.ui.font.normal = value;
    });
    addRow(container, 'Normal', 'Body text, buttons, inputs, dialogs', normalInput);

    const resetLink = document.createElement('div');
    resetLink.textContent = 'Reset to Default';
    resetLink.style.cssText = 'font-size:var(--ui-font-small);color:var(--blue);cursor:pointer;margin-top:4px;text-align:right';
    resetLink.addEventListener('click', () => {
      pendingSettings.conch.ui.font.small = 12.0;
      pendingSettings.conch.ui.font.list = 14.0;
      pendingSettings.conch.ui.font.normal = 14.0;
      smallInput.value = 12.0;
      listInput.value = 14.0;
      normalInput.value = 14.0;
    });
    container.appendChild(resetLink);
  }

  function renderFiles(container, deps) {
    const d = deps || {};
    const pendingSettings = d.pendingSettings;
    if (!container || !pendingSettings) return;

    const addSectionLabel = d.addSectionLabel || function () {};
    const addRow = d.addRow || function () {};
    const setRowTarget = d.setRowTarget || function (row) { return row; };
    const makeSwitch = d.makeSwitch || function () { return document.createElement('span'); };

    const h = document.createElement('h3');
    h.textContent = 'Files';
    container.appendChild(h);

    addSectionLabel(container, 'Explorer');
    const followSwitch = makeSwitch(
      pendingSettings.conch.files.follow_path !== false,
      (val) => { pendingSettings.conch.files.follow_path = val; }
    );
    setRowTarget(
      addRow(
        container,
        'Follow Path',
        'Automatically follow the active terminal working directory in local and remote file panes.',
        followSwitch
      ),
      'files:follow-path'
    );
  }

  global.conchSettingsSectionsBasic = {
    renderAdvanced,
    renderFiles,
  };
})(window);
