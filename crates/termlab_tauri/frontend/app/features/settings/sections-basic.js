(function initTermLabSettingsSectionsBasic(global) {
  'use strict';

  function renderAdvanced(container, deps) {
    const d = deps || {};
    const pendingSettings = d.pendingSettings;
    if (!container || !pendingSettings) return;

    const addSectionLabel = d.addSectionLabel || function () {};
    const addDivider = d.addDivider || function () {};
    const addRow = d.addRow || function () {};
    const setRowTarget = d.setRowTarget || function (row) { return row; };
    const makeCheckbox = d.makeCheckbox || function () { return document.createElement('span'); };
    const makeInput = d.makeInput || function () { return document.createElement('input'); };

    const h = document.createElement('h3');
    h.textContent = 'Advanced';
    container.appendChild(h);

    addSectionLabel(container, 'Startup & Updates');

    const updateCheckbox = makeCheckbox(
      pendingSettings.termlab.check_for_updates !== false,
      (val) => { pendingSettings.termlab.check_for_updates = val; }
    );
    setRowTarget(
      addRow(
        container,
        'Check for Updates',
        'Automatically check for new versions when the app starts (macOS and Windows)',
        updateCheckbox
      ),
      'advanced:check-for-updates'
    );

    addDivider(container);

    addSectionLabel(container, 'Interface Density');
    const densityAnchor = document.createElement('div');
    densityAnchor.dataset.settingId = 'advanced:ui-chrome-font-sizes';
    container.appendChild(densityAnchor);

    const fontNote = document.createElement('div');
    fontNote.className = 'tl-settings__row-desc';
    fontNote.style.marginBottom = '8px';
    fontNote.textContent = 'Fine-tune text sizes for different UI elements (in points)';
    container.appendChild(fontNote);

    const smallInput = makeInput('number', pendingSettings.termlab.ui.font.small, { step: '0.5' });
    smallInput.addEventListener('input', () => {
      const value = parseFloat(smallInput.value);
      if (!isNaN(value)) pendingSettings.termlab.ui.font.small = value;
    });
    addRow(container, 'Small', 'Tab titles, badges, compact labels', smallInput);
    global.tlSpinner.attach(smallInput);

    const listInput = makeInput('number', pendingSettings.termlab.ui.font.list, { step: '0.5' });
    listInput.addEventListener('input', () => {
      const value = parseFloat(listInput.value);
      if (!isNaN(value)) pendingSettings.termlab.ui.font.list = value;
    });
    addRow(container, 'List', 'Tree nodes, table rows, file explorer', listInput);
    global.tlSpinner.attach(listInput);

    const normalInput = makeInput('number', pendingSettings.termlab.ui.font.normal, { step: '0.5' });
    normalInput.addEventListener('input', () => {
      const value = parseFloat(normalInput.value);
      if (!isNaN(value)) pendingSettings.termlab.ui.font.normal = value;
    });
    addRow(container, 'Normal', 'Body text, buttons, inputs, dialogs', normalInput);
    global.tlSpinner.attach(normalInput);

    const resetLink = document.createElement('div');
    resetLink.textContent = 'Reset to Default';
    resetLink.className = 'tl-settings__link';
    resetLink.style.cssText = 'display:block;margin-top:4px;text-align:right';
    resetLink.addEventListener('click', () => {
      pendingSettings.termlab.ui.font.small = 12.0;
      pendingSettings.termlab.ui.font.list = 14.0;
      pendingSettings.termlab.ui.font.normal = 14.0;
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
    const makeCheckbox = d.makeCheckbox || function () { return document.createElement('span'); };

    const h = document.createElement('h3');
    h.textContent = 'Files';
    container.appendChild(h);

    addSectionLabel(container, 'Explorer');
    const followCheckbox = makeCheckbox(
      pendingSettings.termlab.files.follow_path !== false,
      (val) => { pendingSettings.termlab.files.follow_path = val; }
    );
    setRowTarget(
      addRow(
        container,
        'Follow Path',
        'Automatically follow the active terminal working directory in local and remote file panes.',
        followCheckbox
      ),
      'files:follow-path'
    );
  }

  global.termlabSettingsSectionsBasic = {
    renderAdvanced,
    renderFiles,
  };
})(window);
