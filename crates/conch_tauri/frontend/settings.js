// Settings Dialog — sidebar navigation, content area switching, Apply/Cancel.

(function (exports) {
  'use strict';

  let invoke = null;
  let listenFn = null;
  let escapeHandler = null;
  let currentSection = 'appearance';
  let pendingSettings = null;
  let originalSettings = null;
  let cachedThemes = [];
  let cachedPlugins = [];

  const SECTIONS = [
    { group: 'General', items: [
      { id: 'appearance', label: 'Appearance' },
      { id: 'keyboard', label: 'Keyboard Shortcuts' },
    ]},
    { group: 'Editor', items: [
      { id: 'terminal', label: 'Terminal' },
      { id: 'shell', label: 'Shell' },
      { id: 'cursor', label: 'Cursor' },
    ]},
    { group: 'Extensions', items: [
      { id: 'plugins', label: 'Plugins' },
    ]},
    { group: 'Advanced', items: [
      { id: 'advanced', label: 'Advanced' },
    ]},
  ];

  function init(opts) {
    invoke = opts.invoke;
    listenFn = opts.listen;
  }

  async function open() {
    if (document.getElementById('settings-overlay')) { close(); return; }

    try {
      const [settings, themes, plugins] = await Promise.all([
        invoke('get_all_settings'),
        invoke('list_themes'),
        invoke('scan_plugins'),
      ]);
      originalSettings = JSON.parse(JSON.stringify(settings));
      pendingSettings = JSON.parse(JSON.stringify(settings));
      cachedThemes = themes;
      cachedPlugins = plugins;
      currentSection = 'appearance';
      renderDialog();
    } catch (e) {
      if (window.toast) window.toast.error('Settings', 'Failed to load settings: ' + e);
    }
  }

  function close() {
    stopRecording();
    const el = document.getElementById('settings-overlay');
    if (el) el.remove();
    if (escapeHandler) {
      document.removeEventListener('keydown', escapeHandler, true);
      escapeHandler = null;
    }
    pendingSettings = null;
    originalSettings = null;
  }

  function renderDialog() {
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.id = 'settings-overlay';

    const dialog = document.createElement('div');
    dialog.className = 'ssh-form settings-dialog';

    // Title
    const title = document.createElement('div');
    title.className = 'ssh-form-title';
    title.textContent = 'Settings';
    dialog.appendChild(title);

    // Body = sidebar + content
    const body = document.createElement('div');
    body.className = 'settings-body';

    // Sidebar
    const sidebar = document.createElement('div');
    sidebar.className = 'settings-sidebar';
    sidebar.id = 'settings-sidebar';
    for (const group of SECTIONS) {
      const groupEl = document.createElement('div');
      groupEl.className = 'settings-sidebar-group';
      groupEl.textContent = group.group;
      sidebar.appendChild(groupEl);
      for (const item of group.items) {
        const itemEl = document.createElement('div');
        itemEl.className = 'settings-sidebar-item' + (item.id === currentSection ? ' active' : '');
        itemEl.textContent = item.label;
        itemEl.dataset.section = item.id;
        itemEl.addEventListener('click', () => selectSection(item.id));
        sidebar.appendChild(itemEl);
      }
    }
    body.appendChild(sidebar);

    // Content area
    const content = document.createElement('div');
    content.className = 'settings-content';
    content.id = 'settings-content';
    body.appendChild(content);

    dialog.appendChild(body);

    // Footer
    const footer = document.createElement('div');
    footer.className = 'ssh-form-buttons settings-footer';
    const cancelBtn = document.createElement('button');
    cancelBtn.className = 'ssh-form-btn';
    cancelBtn.textContent = 'Cancel';
    cancelBtn.addEventListener('click', close);
    const applyBtn = document.createElement('button');
    applyBtn.className = 'ssh-form-btn primary';
    applyBtn.textContent = 'Apply';
    applyBtn.addEventListener('click', applySettings);
    footer.appendChild(cancelBtn);
    footer.appendChild(applyBtn);
    dialog.appendChild(footer);

    overlay.appendChild(dialog);

    // Click outside to close
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) close(); });

    document.body.appendChild(overlay);

    // Escape handler (capture phase, before xterm.js)
    escapeHandler = function (e) {
      if (e.key === 'Escape') {
        // If a shortcut is being recorded, let the recording handler handle Escape
        if (recordingEl) return;
        e.preventDefault();
        e.stopPropagation();
        close();
      }
    };
    document.addEventListener('keydown', escapeHandler, true);

    // Render initial section
    renderCurrentSection();
  }

  function selectSection(id) {
    currentSection = id;
    // Update sidebar active state
    const sidebar = document.getElementById('settings-sidebar');
    if (sidebar) {
      for (const item of sidebar.querySelectorAll('.settings-sidebar-item')) {
        item.classList.toggle('active', item.dataset.section === id);
      }
    }
    renderCurrentSection();
  }

  function renderCurrentSection() {
    const content = document.getElementById('settings-content');
    if (!content) return;
    content.innerHTML = '';

    switch (currentSection) {
      case 'appearance': renderAppearance(content); break;
      case 'keyboard': renderKeyboard(content); break;
      case 'terminal': renderTerminal(content); break;
      case 'shell': renderShell(content); break;
      case 'cursor': renderCursor(content); break;
      case 'plugins': renderPlugins(content); break;
      case 'advanced': renderAdvanced(content); break;
    }
  }

  // --- Shared layout helpers (reused by all section renderers) ---

  function addSectionLabel(container, text) {
    const label = document.createElement('div');
    label.className = 'settings-section-label';
    label.textContent = text;
    container.appendChild(label);
  }

  function addDivider(container) {
    const hr = document.createElement('hr');
    hr.className = 'settings-divider';
    container.appendChild(hr);
  }

  function addRow(container, labelText, descText, controlEl) {
    const row = document.createElement('div');
    row.className = 'settings-row';
    const left = document.createElement('div');
    const lbl = document.createElement('div');
    lbl.className = 'settings-row-label';
    lbl.textContent = labelText;
    left.appendChild(lbl);
    if (descText) {
      const desc = document.createElement('div');
      desc.className = 'settings-row-desc';
      desc.textContent = descText;
      left.appendChild(desc);
    }
    row.appendChild(left);
    row.appendChild(controlEl);
    container.appendChild(row);
  }

  // --- Appearance section ---

  function renderAppearance(c) {
    const h = document.createElement('h3');
    h.textContent = 'Appearance';
    c.appendChild(h);

    // Sub-group: Color Theme
    addSectionLabel(c, 'Color Theme');

    // Theme dropdown
    const themeSelect = document.createElement('select');
    themeSelect.className = 'settings-select';
    for (const t of cachedThemes) {
      const opt = document.createElement('option');
      opt.value = t;
      opt.textContent = t;
      if (t === pendingSettings.colors.theme) opt.selected = true;
      themeSelect.appendChild(opt);
    }
    themeSelect.addEventListener('change', () => {
      pendingSettings.colors.theme = themeSelect.value;
    });
    addRow(c, 'Theme', 'Color theme for the terminal and UI', themeSelect);

    // Appearance Mode toggle group
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
        for (const b of toggleGroup.querySelectorAll('.settings-toggle')) {
          b.classList.toggle('active', b.textContent === mode);
        }
      });
      toggleGroup.appendChild(btn);
    }
    addRow(c, 'Appearance Mode', null, toggleGroup);

    addDivider(c);

    // Sub-group: Window
    addSectionLabel(c, 'Window');

    // Window Decorations dropdown
    const decoOptions = ['Full', 'Transparent', 'Buttonless', 'None'];
    const decoSelect = document.createElement('select');
    decoSelect.className = 'settings-select';
    for (const d of decoOptions) {
      const opt = document.createElement('option');
      opt.value = d;
      opt.textContent = d;
      if (d === pendingSettings.window.decorations) opt.selected = true;
      decoSelect.appendChild(opt);
    }
    decoSelect.addEventListener('change', () => {
      pendingSettings.window.decorations = decoSelect.value;
    });
    addRow(c, 'Window Decorations', 'Window title bar style', decoSelect);

    // Native Menu Bar (macOS only)
    if (navigator.platform.includes('Mac')) {
      const sw = document.createElement('label');
      sw.className = 'settings-switch';
      const cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.checked = pendingSettings.conch.ui.native_menu_bar;
      cb.addEventListener('change', () => {
        pendingSettings.conch.ui.native_menu_bar = cb.checked;
      });
      const slider = document.createElement('span');
      slider.className = 'slider';
      sw.appendChild(cb);
      sw.appendChild(slider);
      addRow(c, 'Native Menu Bar', 'Use the system menu bar instead of in-app menu', sw);
    }

    addDivider(c);

    // Sub-group: UI Font
    addSectionLabel(c, 'UI Font');

    // Font Family
    const fontInput = document.createElement('input');
    fontInput.type = 'text';
    fontInput.className = 'settings-input';
    fontInput.placeholder = 'System Default';
    fontInput.value = pendingSettings.conch.ui.font_family || '';
    fontInput.addEventListener('input', () => {
      pendingSettings.conch.ui.font_family = fontInput.value;
    });
    addRow(c, 'Font Family', null, fontInput);

    // Font Size
    const sizeInput = document.createElement('input');
    sizeInput.type = 'number';
    sizeInput.className = 'settings-input';
    sizeInput.style.width = '70px';
    sizeInput.value = pendingSettings.conch.ui.font_size;
    sizeInput.min = '6';
    sizeInput.max = '72';
    sizeInput.step = '0.5';
    sizeInput.addEventListener('change', () => {
      const v = parseFloat(sizeInput.value);
      if (!isNaN(v) && v > 0) pendingSettings.conch.ui.font_size = v;
    });
    addRow(c, 'Font Size', null, sizeInput);
  }

  // --- Keyboard Shortcuts section ---

  const isMac = typeof navigator !== 'undefined' && navigator.platform.includes('Mac');

  /** Convert config shortcut string to display string, e.g. "cmd+shift+t" -> "⌘ ⇧ T" */
  function formatShortcut(combo) {
    if (!combo) return '';
    const parts = combo.split('+');
    const display = [];
    for (const p of parts) {
      switch (p) {
        case 'cmd':   display.push(isMac ? '\u2318' : 'Ctrl'); break;
        case 'shift': display.push('\u21E7'); break;
        case 'alt':   display.push(isMac ? '\u2325' : 'Alt'); break;
        case 'ctrl':  display.push(isMac ? '\u2303' : 'Ctrl'); break;
        default:      display.push(p.toUpperCase()); break;
      }
    }
    return display.join(' ');
  }

  /** Normalize a keydown event into config format, e.g. "cmd+shift+z" */
  function normalizeKeyEvent(e) {
    const parts = [];
    if (e.metaKey) parts.push('cmd');
    if (e.ctrlKey) parts.push('ctrl');
    if (e.altKey) parts.push('alt');
    if (e.shiftKey) parts.push('shift');
    // Ignore bare modifier keys
    const key = e.key.toLowerCase();
    if (['meta', 'control', 'alt', 'shift'].includes(key)) return null;
    parts.push(key);
    return parts.join('+');
  }

  // Currently recording shortcut state
  let recordingEl = null;
  let recordingKey = null;
  let recordingHandler = null;

  function stopRecording() {
    if (recordingEl) {
      recordingEl.classList.remove('recording');
      recordingEl.textContent = formatShortcut(
        pendingSettings.conch.keyboard[recordingKey]
      );
    }
    if (recordingHandler) {
      document.removeEventListener('keydown', recordingHandler, true);
      recordingHandler = null;
    }
    recordingEl = null;
    recordingKey = null;
  }

  function startRecording(el, settingsKey) {
    // Stop any existing recording first
    stopRecording();

    recordingEl = el;
    recordingKey = settingsKey;
    el.classList.add('recording');
    el.textContent = 'Press keys...';

    recordingHandler = function (e) {
      e.preventDefault();
      e.stopPropagation();

      // Escape cancels recording
      if (e.key === 'Escape') {
        stopRecording();
        return;
      }

      const combo = normalizeKeyEvent(e);
      if (!combo) return; // bare modifier, keep waiting

      pendingSettings.conch.keyboard[settingsKey] = combo;
      stopRecording();
    };
    document.addEventListener('keydown', recordingHandler, true);
  }

  function renderKeyboard(c) {
    // Stop any lingering recording when re-rendering
    stopRecording();

    const h = document.createElement('h3');
    h.textContent = 'Keyboard Shortcuts';
    c.appendChild(h);

    const groups = [
      {
        label: 'Tab & Window',
        shortcuts: [
          { key: 'new_tab', label: 'New Tab' },
          { key: 'close_tab', label: 'Close Tab' },
          { key: 'new_window', label: 'New Window' },
          { key: 'quit', label: 'Quit' },
        ],
      },
      {
        label: 'View',
        shortcuts: [
          { key: 'zen_mode', label: 'Zen Mode' },
          { key: 'toggle_left_panel', label: 'Toggle File Explorer' },
          { key: 'toggle_right_panel', label: 'Toggle Sessions Panel' },
          { key: 'toggle_bottom_panel', label: 'Toggle Bottom Panel' },
        ],
      },
    ];

    for (let gi = 0; gi < groups.length; gi++) {
      const group = groups[gi];
      addSectionLabel(c, group.label);

      for (const sc of group.shortcuts) {
        const keyBox = document.createElement('span');
        keyBox.className = 'settings-shortcut-key';
        keyBox.textContent = formatShortcut(pendingSettings.conch.keyboard[sc.key]);
        keyBox.addEventListener('click', () => startRecording(keyBox, sc.key));
        addRow(c, sc.label, null, keyBox);
      }

      if (gi < groups.length - 1) addDivider(c);
    }
  }
  function renderTerminal(c) {
    c.innerHTML = '<h3>Terminal</h3><p style="color:var(--dim-fg)">Coming soon...</p>';
  }
  function renderShell(c) {
    c.innerHTML = '<h3>Shell</h3><p style="color:var(--dim-fg)">Coming soon...</p>';
  }
  function renderCursor(c) {
    c.innerHTML = '<h3>Cursor</h3><p style="color:var(--dim-fg)">Coming soon...</p>';
  }
  function renderPlugins(c) {
    c.innerHTML = '<h3>Plugins</h3><p style="color:var(--dim-fg)">Coming soon...</p>';
  }
  function renderAdvanced(c) {
    c.innerHTML = '<h3>Advanced</h3><p style="color:var(--dim-fg)">Coming soon...</p>';
  }

  async function applySettings() {
    try {
      const result = await invoke('save_settings', { settings: pendingSettings });
      close();
      if (result && result.restart_required) {
        window.toast && window.toast.show('Some changes require a restart to take effect', 'info', 5000);
      }
    } catch (e) {
      window.toast && window.toast.show('Failed to save settings: ' + e, 'error');
    }
  }

  exports.settings = { init, open, close };
})(window);
