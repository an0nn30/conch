(function initConchSettingsSectionsTerminal(global) {
  'use strict';

  function renderTerminal(container, deps) {
    const d = deps || {};
    const pendingSettings = d.pendingSettings;
    const cachedFonts = d.cachedFonts || { monospace: [] };
    if (!container || !pendingSettings) return;

    const addSectionLabel = d.addSectionLabel || function () {};
    const addDivider = d.addDivider || function () {};
    const addRow = d.addRow || function () {};
    const setRowTarget = d.setRowTarget || function (row) { return row; };
    const makeInput = d.makeInput || function () { return document.createElement('input'); };

    const h = document.createElement('h3');
    h.textContent = 'Terminal';
    container.appendChild(h);

    addSectionLabel(container, 'Typography');

    const fontFamilySelect = document.createElement('select');
    fontFamilySelect.className = 'settings-select';
    const defaultOpt = document.createElement('option');
    defaultOpt.value = '';
    defaultOpt.textContent = 'System Default';
    if (!pendingSettings.terminal.font.normal.family) defaultOpt.selected = true;
    fontFamilySelect.appendChild(defaultOpt);
    for (const f of cachedFonts.monospace || []) {
      const opt = document.createElement('option');
      opt.value = f;
      opt.textContent = f;
      if (f === pendingSettings.terminal.font.normal.family) opt.selected = true;
      fontFamilySelect.appendChild(opt);
    }
    fontFamilySelect.addEventListener('change', () => {
      pendingSettings.terminal.font.normal.family = fontFamilySelect.value;
    });
    setRowTarget(addRow(container, 'Terminal Font Family', null, fontFamilySelect), 'terminal:font-family');

    const fontSizeInput = makeInput('number', pendingSettings.terminal.font.size);
    fontSizeInput.addEventListener('input', () => {
      const value = parseFloat(fontSizeInput.value);
      if (!isNaN(value)) pendingSettings.terminal.font.size = value;
    });
    setRowTarget(addRow(container, 'Terminal Font Size', null, fontSizeInput), 'terminal:font-size');

    const offsetXInput = makeInput('number', pendingSettings.terminal.font.offset.x, { step: '0.5' });
    offsetXInput.addEventListener('input', () => {
      const value = parseFloat(offsetXInput.value);
      if (!isNaN(value)) pendingSettings.terminal.font.offset.x = value;
    });
    setRowTarget(addRow(container, 'Font Offset X', null, offsetXInput), 'terminal:font-offset-x');

    const offsetYInput = makeInput('number', pendingSettings.terminal.font.offset.y, { step: '0.5' });
    offsetYInput.addEventListener('input', () => {
      const value = parseFloat(offsetYInput.value);
      if (!isNaN(value)) pendingSettings.terminal.font.offset.y = value;
    });
    setRowTarget(addRow(container, 'Font Offset Y', null, offsetYInput), 'terminal:font-offset-y');

    addDivider(container);
    addSectionLabel(container, 'Scrolling');

    const scrollInput = makeInput('number', pendingSettings.terminal.scroll_sensitivity, {
      step: '0.05',
      min: 0,
      max: 1,
    });
    scrollInput.addEventListener('input', () => {
      const value = parseFloat(scrollInput.value);
      if (!isNaN(value)) pendingSettings.terminal.scroll_sensitivity = value;
    });
    setRowTarget(
      addRow(container, 'Scroll Sensitivity', '0.0 to 1.0 (tuned for macOS trackpads)', scrollInput),
      'terminal:scroll-sensitivity'
    );
  }

  function renderShell(container, deps) {
    const d = deps || {};
    const pendingSettings = d.pendingSettings;
    if (!container || !pendingSettings) return;

    const addSectionLabel = d.addSectionLabel || function () {};
    const addDivider = d.addDivider || function () {};
    const addRow = d.addRow || function () {};
    const setRowTarget = d.setRowTarget || function (row) { return row; };
    const makeInput = d.makeInput || function () { return document.createElement('input'); };

    const h = document.createElement('h3');
    h.textContent = 'Shell & Environment';
    container.appendChild(h);

    addSectionLabel(container, 'Launch');

    const shellInput = makeInput('text', pendingSettings.terminal.shell.program, {
      placeholder: 'Uses $SHELL login shell',
    });
    shellInput.addEventListener('input', () => {
      pendingSettings.terminal.shell.program = shellInput.value;
    });
    setRowTarget(addRow(container, 'Shell Program', null, shellInput), 'shell:program');

    const argsInput = makeInput('text', (pendingSettings.terminal.shell.args || []).join(', '));
    argsInput.addEventListener('input', () => {
      pendingSettings.terminal.shell.args = argsInput.value
        .split(',')
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
    });
    setRowTarget(addRow(container, 'Arguments', 'Comma-separated (e.g. -l, -c, echo ok)', argsInput), 'shell:args');

    addDivider(container);
    addSectionLabel(container, 'Environment Variables');

    const envContainer = document.createElement('div');
    envContainer.dataset.settingId = 'shell:env';
    envContainer.className = 'settings-env-container';
    container.appendChild(envContainer);

    function renderEnvRows() {
      envContainer.innerHTML = '';
      const env = pendingSettings.terminal.env || {};
      const keys = Object.keys(env);

      for (const oldKey of keys) {
        const row = document.createElement('div');
        row.className = 'settings-env-row';

        const keyInput = makeInput('text', oldKey, { style: 'width:120px;' });
        row.appendChild(keyInput);

        const eqLabel = document.createElement('span');
        eqLabel.className = 'settings-env-eq';
        eqLabel.textContent = '=';
        row.appendChild(eqLabel);

        const valInput = makeInput('text', env[oldKey], { style: 'flex:1;' });
        row.appendChild(valInput);

        const removeBtn = document.createElement('button');
        removeBtn.className = 'ssh-form-btn settings-env-remove';
        removeBtn.textContent = 'X';
        removeBtn.addEventListener('click', () => {
          delete pendingSettings.terminal.env[oldKey];
          renderEnvRows();
        });
        row.appendChild(removeBtn);

        keyInput.addEventListener('change', () => {
          const newKey = keyInput.value.trim();
          const val = pendingSettings.terminal.env[oldKey];
          delete pendingSettings.terminal.env[oldKey];
          if (newKey) pendingSettings.terminal.env[newKey] = val;
          renderEnvRows();
        });

        valInput.addEventListener('input', () => {
          const currentKey = keyInput.value.trim() || oldKey;
          pendingSettings.terminal.env[currentKey] = valInput.value;
        });

        envContainer.appendChild(row);
      }

      const addBtn = document.createElement('button');
      addBtn.className = 'ssh-form-btn settings-env-add';
      addBtn.textContent = '+ Add Variable';
      addBtn.addEventListener('click', () => {
        if (!pendingSettings.terminal.env) pendingSettings.terminal.env = {};
        let newKey = '';
        let i = 0;
        while (Object.prototype.hasOwnProperty.call(pendingSettings.terminal.env, newKey)) {
          i++;
          newKey = 'VAR_' + i;
        }
        pendingSettings.terminal.env[newKey] = '';
        renderEnvRows();
      });
      envContainer.appendChild(addBtn);

      const note = document.createElement('div');
      note.className = 'settings-row-desc';
      note.style.marginTop = '8px';
      note.textContent = 'TERM and COLORTERM are always set to xterm-256color and truecolor.';
      envContainer.appendChild(note);
    }

    renderEnvRows();
  }

  function renderCursor(container, deps) {
    const d = deps || {};
    const pendingSettings = d.pendingSettings;
    if (!container || !pendingSettings) return;

    const addSectionLabel = d.addSectionLabel || function () {};
    const addDivider = d.addDivider || function () {};
    const addRow = d.addRow || function () {};
    const setRowTarget = d.setRowTarget || function (row) { return row; };
    const makeSwitch = d.makeSwitch || function () { return document.createElement('span'); };
    const makeToggleGroup = d.makeToggleGroup || function () { return document.createElement('span'); };

    const h = document.createElement('h3');
    h.textContent = 'Cursor';
    container.appendChild(h);

    addSectionLabel(container, 'Primary Cursor');

    const shapeToggle = makeToggleGroup(
      [
        { label: 'Block', value: 'Block' },
        { label: 'Underline', value: 'Underline' },
        { label: 'Beam', value: 'Beam' },
      ],
      pendingSettings.terminal.cursor.style.shape,
      (val) => { pendingSettings.terminal.cursor.style.shape = val; }
    );
    setRowTarget(addRow(container, 'Cursor Shape', null, shapeToggle), 'cursor:shape');

    const blinkSwitch = makeSwitch(
      pendingSettings.terminal.cursor.style.blinking,
      (val) => { pendingSettings.terminal.cursor.style.blinking = val; }
    );
    setRowTarget(addRow(container, 'Cursor Blinking', null, blinkSwitch), 'cursor:blinking');

    addDivider(container);
    addSectionLabel(container, 'Vi Mode Override');

    const viNote = document.createElement('div');
    viNote.dataset.settingId = 'cursor:vi-mode';
    viNote.className = 'settings-row-desc';
    viNote.style.marginBottom = '8px';
    viNote.textContent = 'Optional cursor style when vi mode is active in your shell.';
    container.appendChild(viNote);

    const viStyle = pendingSettings.terminal.cursor.vi_mode_style;
    const viActiveShape = viStyle ? viStyle.shape : null;

    const viBlinkRow = document.createElement('div');
    viBlinkRow.id = 'vi-blink-row';

    const viShapeToggle = makeToggleGroup(
      [
        { label: 'None', value: null },
        { label: 'Block', value: 'Block' },
        { label: 'Underline', value: 'Underline' },
        { label: 'Beam', value: 'Beam' },
      ],
      viActiveShape,
      (val) => {
        if (val === null) {
          pendingSettings.terminal.cursor.vi_mode_style = null;
          viBlinkRow.style.display = 'none';
        } else {
          if (!pendingSettings.terminal.cursor.vi_mode_style) {
            pendingSettings.terminal.cursor.vi_mode_style = { shape: val, blinking: false };
          } else {
            pendingSettings.terminal.cursor.vi_mode_style.shape = val;
          }
          viBlinkRow.style.display = '';
        }
      }
    );
    setRowTarget(addRow(container, 'Vi Mode Override', null, viShapeToggle), 'cursor:vi-mode');

    const viBlinkSwitch = makeSwitch(
      viStyle ? viStyle.blinking : false,
      (val) => {
        if (pendingSettings.terminal.cursor.vi_mode_style) {
          pendingSettings.terminal.cursor.vi_mode_style.blinking = val;
        }
      }
    );
    viBlinkRow.style.display = viStyle ? '' : 'none';
    addRow(viBlinkRow, 'Blinking', null, viBlinkSwitch);
    container.appendChild(viBlinkRow);
  }

  global.conchSettingsSectionsTerminal = {
    renderTerminal,
    renderShell,
    renderCursor,
  };
})(window);
