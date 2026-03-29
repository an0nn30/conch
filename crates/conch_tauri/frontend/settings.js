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
  let cachedFonts = { all: [], monospace: [] };

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
      const [settings, themes, plugins, fonts] = await Promise.all([
        invoke('get_all_settings'),
        invoke('list_themes'),
        invoke('scan_plugins'),
        invoke('list_system_fonts'),
      ]);
      originalSettings = JSON.parse(JSON.stringify(settings));
      pendingSettings = JSON.parse(JSON.stringify(settings));
      cachedThemes = themes;
      cachedPlugins = plugins;
      cachedFonts = fonts;
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

  // --- Theme preview helpers ---

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
    box.className = 'tp-container';

    // "PREVIEW" label
    const label = document.createElement('div');
    label.textContent = 'PREVIEW';
    label.className = 'tp-label tp-dim';
    box.appendChild(label);

    // Prompt line
    box.appendChild(line(
      span('tp-green tp-bold', 'user@conch'),
      span('tp-fg', ':'),
      span('tp-blue tp-bold', '~/projects'),
      span('tp-fg', ' $ '),
      span('tp-fg', 'ls -la'),
    ));

    // total line
    box.appendChild(line(span('tp-fg', 'total 42')));

    // File listing entries: [permissions, links, user, group, size, date, name]
    const entries = [
      // [perm, links, user, group, size, date, name, nameClass]
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

    // echo command line
    box.appendChild(line(
      span('tp-green tp-bold', 'user@conch'),
      span('tp-fg', ':'),
      span('tp-blue tp-bold', '~/projects'),
      span('tp-fg', ' $ '),
      span('tp-magenta', 'echo'),
      span('tp-fg', ' '),
      span('tp-yellow', '"hello world"'),
    ));

    // output line
    box.appendChild(line(span('tp-fg', 'hello world')));

    // cursor prompt line
    const cursorLine = line(
      span('tp-green tp-bold', 'user@conch'),
      span('tp-fg', ':'),
      span('tp-blue tp-bold', '~/projects'),
      span('tp-fg', ' $ '),
    );
    const cursor = document.createElement('span');
    cursor.className = 'tp-cursor';
    cursor.textContent = ' ';
    cursorLine.appendChild(cursor);
    box.appendChild(cursorLine);

    // Swatch divider
    const dividerEl = document.createElement('div');
    dividerEl.className = 'tp-swatch-divider';
    box.appendChild(dividerEl);

    // Normal swatches row
    const normalRow = document.createElement('div');
    normalRow.className = 'tp-swatch-row tp-swatch-row--normal';
    const normalClasses = ['tp-sw-black','tp-sw-red','tp-sw-green','tp-sw-yellow','tp-sw-blue','tp-sw-magenta','tp-sw-cyan','tp-sw-white'];
    for (const cls of normalClasses) {
      const sw = document.createElement('div');
      sw.className = cls + ' tp-swatch';
      normalRow.appendChild(sw);
    }
    box.appendChild(normalRow);

    // Bright swatches row
    const brightRow = document.createElement('div');
    brightRow.className = 'tp-swatch-row';
    const brightClasses = ['tp-sw-bright-black','tp-sw-bright-red','tp-sw-bright-green','tp-sw-bright-yellow','tp-sw-bright-blue','tp-sw-bright-magenta','tp-sw-bright-cyan','tp-sw-bright-white'];
    for (const cls of brightClasses) {
      const sw = document.createElement('div');
      sw.className = cls + ' tp-swatch';
      brightRow.appendChild(sw);
    }
    box.appendChild(brightRow);

    return box;
  }

  function updateThemePreview(container, tc) {
    if (!tc) return;

    // Container background and border
    container.style.background = tc.background || '';
    container.style.borderColor = tc.tab_border || '';

    // Text color classes
    const colorMap = {
      '.tp-fg':      tc.foreground,
      '.tp-dim':     tc.dim_fg,
      '.tp-green':   tc.green,
      '.tp-blue':    tc.blue,
      '.tp-cyan':    tc.cyan,
      '.tp-red':     tc.red,
      '.tp-yellow':  tc.yellow,
      '.tp-magenta': tc.magenta,
    };
    for (const [sel, color] of Object.entries(colorMap)) {
      if (!color) continue;
      for (const el of container.querySelectorAll(sel)) {
        el.style.color = color;
      }
    }

    // Bold elements
    for (const el of container.querySelectorAll('.tp-bold')) {
      el.style.fontWeight = 'bold';
    }

    // Cursor block
    const cursorEl = container.querySelector('.tp-cursor');
    if (cursorEl) {
      cursorEl.style.background = tc.cursor_color || tc.foreground || '';
      cursorEl.style.color = tc.cursor_text || tc.background || '';
    }

    // Normal swatches
    const normalSwatches = [
      ['.tp-sw-black',   tc.black],
      ['.tp-sw-red',     tc.red],
      ['.tp-sw-green',   tc.green],
      ['.tp-sw-yellow',  tc.yellow],
      ['.tp-sw-blue',    tc.blue],
      ['.tp-sw-magenta', tc.magenta],
      ['.tp-sw-cyan',    tc.cyan],
      ['.tp-sw-white',   tc.white],
    ];
    for (const [sel, color] of normalSwatches) {
      if (!color) continue;
      const el = container.querySelector(sel);
      if (el) el.style.background = color;
    }

    // Bright swatches
    const brightSwatches = [
      ['.tp-sw-bright-black',   tc.bright_black],
      ['.tp-sw-bright-red',     tc.bright_red],
      ['.tp-sw-bright-green',   tc.bright_green],
      ['.tp-sw-bright-yellow',  tc.bright_yellow],
      ['.tp-sw-bright-blue',    tc.bright_blue],
      ['.tp-sw-bright-magenta', tc.bright_magenta],
      ['.tp-sw-bright-cyan',    tc.bright_cyan],
      ['.tp-sw-bright-white',   tc.bright_white],
    ];
    for (const [sel, color] of brightSwatches) {
      if (!color) continue;
      const el = container.querySelector(sel);
      if (el) el.style.background = color;
    }

    // Swatch divider border
    const divider = container.querySelector('.tp-swatch-divider');
    if (divider) divider.style.borderTopColor = tc.active_highlight || '';
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
    addRow(c, 'Theme', 'Color theme for the terminal and UI', themeSelect);

    // Theme preview box
    const previewBox = buildThemePreview();
    c.appendChild(previewBox);

    // Initialize preview from pending selection (not persisted config)
    let previewSeq = 0;
    invoke('preview_theme_colors', { name: pendingSettings.colors.theme })
      .then(tc => updateThemePreview(previewBox, tc))
      .catch(() => {});

    // Single change handler: update pending + preview with race guard
    themeSelect.addEventListener('change', () => {
      pendingSettings.colors.theme = themeSelect.value;
      const seq = ++previewSeq;
      invoke('preview_theme_colors', { name: themeSelect.value })
        .then(tc => { if (seq === previewSeq) updateThemePreview(previewBox, tc); })
        .catch(() => {});
    });

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

    // Sub-group: Notifications
    addSectionLabel(c, 'Notifications');

    // Notification position toggle
    const posOptions = ['Bottom', 'Top'];
    const posGroup = document.createElement('div');
    posGroup.className = 'settings-toggle-group';
    for (const pos of posOptions) {
      const btn = document.createElement('button');
      btn.className = 'settings-toggle';
      if ((pendingSettings.conch.ui.notification_position || 'bottom').toLowerCase() === pos.toLowerCase()) btn.classList.add('active');
      btn.textContent = pos;
      btn.addEventListener('click', () => {
        pendingSettings.conch.ui.notification_position = pos.toLowerCase();
        for (const b of posGroup.querySelectorAll('.settings-toggle')) {
          b.classList.toggle('active', b.textContent === pos);
        }
      });
      posGroup.appendChild(btn);
    }
    addRow(c, 'Notification Position', 'Where toast notifications appear on screen', posGroup);

    // Native notifications toggle
    const nativeSwitch = makeSwitch(
      pendingSettings.conch.ui.native_notifications !== false,
      (val) => { pendingSettings.conch.ui.native_notifications = val; }
    );
    addRow(c, 'Native notifications', 'Use system notifications when the app is not focused', nativeSwitch);

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
    const fontSelect = document.createElement('select');
    fontSelect.className = 'settings-select';
    const defaultOpt = document.createElement('option');
    defaultOpt.value = '';
    defaultOpt.textContent = 'System Default';
    if (!pendingSettings.conch.ui.font_family) defaultOpt.selected = true;
    fontSelect.appendChild(defaultOpt);
    for (const f of cachedFonts.all) {
      const opt = document.createElement('option');
      opt.value = f;
      opt.textContent = f;
      if (f === pendingSettings.conch.ui.font_family) opt.selected = true;
      fontSelect.appendChild(opt);
    }
    fontSelect.addEventListener('change', () => {
      pendingSettings.conch.ui.font_family = fontSelect.value;
    });
    addRow(c, 'Font Family', null, fontSelect);

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
          { key: 'rename_tab', label: 'Rename Tab' },
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
      {
        label: 'Split Panes',
        shortcuts: [
          { key: 'split_vertical', label: 'Split Pane Vertically' },
          { key: 'split_horizontal', label: 'Split Pane Horizontally' },
          { key: 'close_pane', label: 'Close Pane' },
          { key: 'navigate_pane_up', label: 'Navigate Pane Up' },
          { key: 'navigate_pane_down', label: 'Navigate Pane Down' },
          { key: 'navigate_pane_left', label: 'Navigate Pane Left' },
          { key: 'navigate_pane_right', label: 'Navigate Pane Right' },
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
  // --- Shared control helpers ---

  function makeInput(type, value, opts = {}) {
    const input = document.createElement('input');
    input.type = type;
    input.className = 'settings-input';
    input.value = value ?? '';
    if (opts.placeholder) input.placeholder = opts.placeholder;
    if (opts.step) input.step = opts.step;
    if (opts.min !== undefined) input.min = opts.min;
    if (opts.max !== undefined) input.max = opts.max;
    if (opts.style) input.style.cssText = opts.style;
    return input;
  }

  function makeToggleGroup(options, activeValue, onChange) {
    const group = document.createElement('div');
    group.className = 'settings-toggle-group';
    for (const opt of options) {
      const btn = document.createElement('div');
      btn.className = 'settings-toggle' + (opt.value === activeValue ? ' active' : '');
      btn.textContent = opt.label;
      btn.addEventListener('click', () => {
        onChange(opt.value);
        for (const child of group.children) child.classList.remove('active');
        btn.classList.add('active');
      });
      group.appendChild(btn);
    }
    return group;
  }

  function makeSwitch(checked, onChange) {
    const label = document.createElement('label');
    label.className = 'settings-switch';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = checked;
    cb.addEventListener('change', () => onChange(cb.checked));
    const slider = document.createElement('span');
    slider.className = 'slider';
    label.appendChild(cb);
    label.appendChild(slider);
    return label;
  }

  // --- Terminal section ---

  function renderTerminal(c) {
    const h = document.createElement('h3');
    h.textContent = 'Terminal';
    c.appendChild(h);

    // Sub-group: Font
    addSectionLabel(c, 'Font');

    const fontFamilySelect = document.createElement('select');
    fontFamilySelect.className = 'settings-select';
    const defaultOpt = document.createElement('option');
    defaultOpt.value = '';
    defaultOpt.textContent = 'System Default';
    if (!pendingSettings.terminal.font.normal.family) defaultOpt.selected = true;
    fontFamilySelect.appendChild(defaultOpt);
    for (const f of cachedFonts.monospace) {
      const opt = document.createElement('option');
      opt.value = f;
      opt.textContent = f;
      if (f === pendingSettings.terminal.font.normal.family) opt.selected = true;
      fontFamilySelect.appendChild(opt);
    }
    fontFamilySelect.addEventListener('change', () => {
      pendingSettings.terminal.font.normal.family = fontFamilySelect.value;
    });
    addRow(c, 'Font Family', null, fontFamilySelect);

    const fontSizeInput = makeInput('number', pendingSettings.terminal.font.size);
    fontSizeInput.addEventListener('input', () => {
      const v = parseFloat(fontSizeInput.value);
      if (!isNaN(v)) pendingSettings.terminal.font.size = v;
    });
    addRow(c, 'Font Size', null, fontSizeInput);

    const offsetXInput = makeInput('number', pendingSettings.terminal.font.offset.x, { step: '0.5' });
    offsetXInput.addEventListener('input', () => {
      const v = parseFloat(offsetXInput.value);
      if (!isNaN(v)) pendingSettings.terminal.font.offset.x = v;
    });
    addRow(c, 'Font Offset X', null, offsetXInput);

    const offsetYInput = makeInput('number', pendingSettings.terminal.font.offset.y, { step: '0.5' });
    offsetYInput.addEventListener('input', () => {
      const v = parseFloat(offsetYInput.value);
      if (!isNaN(v)) pendingSettings.terminal.font.offset.y = v;
    });
    addRow(c, 'Font Offset Y', null, offsetYInput);

    addDivider(c);

    // Sub-group: Scrolling
    addSectionLabel(c, 'Scrolling');

    const scrollInput = makeInput('number', pendingSettings.terminal.scroll_sensitivity, {
      step: '0.05', min: 0, max: 1,
    });
    scrollInput.addEventListener('input', () => {
      const v = parseFloat(scrollInput.value);
      if (!isNaN(v)) pendingSettings.terminal.scroll_sensitivity = v;
    });
    addRow(c, 'Scroll Sensitivity', '0.0 to 1.0 (tuned for macOS trackpads)', scrollInput);
  }

  // --- Shell section ---

  function renderShell(c) {
    const h = document.createElement('h3');
    h.textContent = 'Shell';
    c.appendChild(h);

    // Sub-group: Program
    addSectionLabel(c, 'Program');

    const shellInput = makeInput('text', pendingSettings.terminal.shell.program, {
      placeholder: 'Uses $SHELL login shell',
    });
    shellInput.addEventListener('input', () => {
      pendingSettings.terminal.shell.program = shellInput.value;
    });
    addRow(c, 'Shell Program', null, shellInput);

    const argsInput = makeInput('text', (pendingSettings.terminal.shell.args || []).join(', '));
    argsInput.addEventListener('input', () => {
      pendingSettings.terminal.shell.args = argsInput.value
        .split(',')
        .map(s => s.trim())
        .filter(s => s.length > 0);
    });
    addRow(c, 'Arguments', 'Comma-separated (e.g. -l, -c, echo ok)', argsInput);

    addDivider(c);

    // Sub-group: Environment Variables
    addSectionLabel(c, 'Environment Variables');

    const envContainer = document.createElement('div');
    envContainer.className = 'settings-env-container';
    c.appendChild(envContainer);

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

        // When key changes, rename the entry in the env object
        keyInput.addEventListener('change', () => {
          const newKey = keyInput.value.trim();
          const val = pendingSettings.terminal.env[oldKey];
          delete pendingSettings.terminal.env[oldKey];
          if (newKey) pendingSettings.terminal.env[newKey] = val;
          renderEnvRows();
        });

        // When value changes, update in place
        valInput.addEventListener('input', () => {
          const currentKey = keyInput.value.trim() || oldKey;
          pendingSettings.terminal.env[currentKey] = valInput.value;
        });

        envContainer.appendChild(row);
      }

      // Add variable button
      const addBtn = document.createElement('button');
      addBtn.className = 'ssh-form-btn settings-env-add';
      addBtn.textContent = '+ Add Variable';
      addBtn.addEventListener('click', () => {
        if (!pendingSettings.terminal.env) pendingSettings.terminal.env = {};
        // Find a unique empty key name
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

      // Note about TERM / COLORTERM
      const note = document.createElement('div');
      note.className = 'settings-row-desc';
      note.style.marginTop = '8px';
      note.textContent = 'TERM and COLORTERM are always set to xterm-256color and truecolor.';
      envContainer.appendChild(note);
    }

    renderEnvRows();
  }

  // --- Cursor section ---

  function renderCursor(c) {
    const h = document.createElement('h3');
    h.textContent = 'Cursor';
    c.appendChild(h);

    // Sub-group: Style
    addSectionLabel(c, 'Style');

    const shapeToggle = makeToggleGroup(
      [
        { label: 'Block', value: 'Block' },
        { label: 'Underline', value: 'Underline' },
        { label: 'Beam', value: 'Beam' },
      ],
      pendingSettings.terminal.cursor.style.shape,
      (val) => { pendingSettings.terminal.cursor.style.shape = val; }
    );
    addRow(c, 'Shape', null, shapeToggle);

    const blinkSwitch = makeSwitch(
      pendingSettings.terminal.cursor.style.blinking,
      (val) => { pendingSettings.terminal.cursor.style.blinking = val; }
    );
    addRow(c, 'Blinking', null, blinkSwitch);

    addDivider(c);

    // Sub-group: Vi Mode Override
    addSectionLabel(c, 'Vi Mode Override');

    const viNote = document.createElement('div');
    viNote.className = 'settings-row-desc';
    viNote.style.marginBottom = '8px';
    viNote.textContent = 'Optional cursor style when vi mode is active in your shell.';
    c.appendChild(viNote);

    const viStyle = pendingSettings.terminal.cursor.vi_mode_style;
    const viActiveShape = viStyle ? viStyle.shape : null;

    // Container for vi mode blinking toggle (shown/hidden based on shape)
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
    addRow(c, 'Shape', null, viShapeToggle);

    // Vi mode blinking toggle
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
    c.appendChild(viBlinkRow);
  }

  // --- Plugins section ---

  function renderPlugins(c) {
    const h = document.createElement('h3');
    h.textContent = 'Plugins';
    c.appendChild(h);

    // Sub-group: Plugin System
    addSectionLabel(c, 'Plugin System');

    const enablePluginsSwitch = makeSwitch(
      pendingSettings.conch.plugins.enabled,
      (val) => { pendingSettings.conch.plugins.enabled = val; }
    );
    addRow(c, 'Enable Plugins', 'Master switch \u2014 disable to run as pure terminal', enablePluginsSwitch);

    addDivider(c);

    // Sub-group: Plugin Types
    addSectionLabel(c, 'Plugin Types');

    const luaSwitch = makeSwitch(
      pendingSettings.conch.plugins.lua,
      (val) => { pendingSettings.conch.plugins.lua = val; }
    );
    addRow(c, 'Lua Plugins', null, luaSwitch);

    const javaSwitch = makeSwitch(
      pendingSettings.conch.plugins.java,
      (val) => { pendingSettings.conch.plugins.java = val; }
    );
    addRow(c, 'Java Plugins', 'Disabling avoids JVM startup overhead', javaSwitch);

    addDivider(c);

    // Sub-group: Search Paths
    addSectionLabel(c, 'Search Paths');

    const pathsContainer = document.createElement('div');
    c.appendChild(pathsContainer);

    function renderSearchPaths() {
      pathsContainer.innerHTML = '';
      const paths = pendingSettings.conch.plugins.search_paths || [];

      for (let i = 0; i < paths.length; i++) {
        const row = document.createElement('div');
        row.style.cssText = 'display:flex; align-items:center; gap:6px; margin-bottom:4px;';

        const pathInput = makeInput('text', paths[i], { style: 'flex:1;' });
        pathInput.addEventListener('input', () => {
          pendingSettings.conch.plugins.search_paths[i] = pathInput.value;
        });
        row.appendChild(pathInput);

        const removeBtn = document.createElement('button');
        removeBtn.className = 'ssh-form-btn settings-env-remove';
        removeBtn.textContent = 'X';
        removeBtn.addEventListener('click', () => {
          pendingSettings.conch.plugins.search_paths.splice(i, 1);
          renderSearchPaths();
        });
        row.appendChild(removeBtn);

        pathsContainer.appendChild(row);
      }

      const addBtn = document.createElement('button');
      addBtn.className = 'ssh-form-btn settings-env-add';
      addBtn.textContent = '+ Add Path';
      addBtn.addEventListener('click', () => {
        if (!pendingSettings.conch.plugins.search_paths) {
          pendingSettings.conch.plugins.search_paths = [];
        }
        pendingSettings.conch.plugins.search_paths.push('');
        renderSearchPaths();
      });
      pathsContainer.appendChild(addBtn);
    }

    renderSearchPaths();

    addDivider(c);

    // Sub-group: Installed Plugins
    const installedHeader = document.createElement('div');
    installedHeader.style.cssText = 'display:flex; justify-content:space-between; align-items:center;';
    const installedLabel = document.createElement('div');
    installedLabel.className = 'settings-section-label';
    installedLabel.textContent = 'Installed Plugins';
    installedHeader.appendChild(installedLabel);

    const rescanBtn = document.createElement('button');
    rescanBtn.className = 'ssh-form-btn';
    rescanBtn.textContent = 'Rescan';
    rescanBtn.addEventListener('click', async () => {
      rescanBtn.disabled = true;
      try {
        cachedPlugins = await invoke('scan_plugins');
      } catch (e) {
        if (window.toast) window.toast.error('Plugin Scan Failed', String(e));
      }
      rescanBtn.disabled = false;
      renderPluginList();
    });
    installedHeader.appendChild(rescanBtn);
    c.appendChild(installedHeader);

    const pluginListContainer = document.createElement('div');
    c.appendChild(pluginListContainer);

    function renderPluginList() {
      pluginListContainer.innerHTML = '';

      if (!cachedPlugins || cachedPlugins.length === 0) {
        const empty = document.createElement('div');
        empty.style.cssText = 'padding:16px; text-align:center; color:var(--dim-fg); font-size:12px;';
        empty.textContent = 'No plugins found in search paths';
        pluginListContainer.appendChild(empty);
        return;
      }

      for (const plugin of cachedPlugins) {
        const row = document.createElement('div');
        row.style.cssText = 'background:var(--bg); border-radius:6px; padding:8px 10px; margin-bottom:6px; display:flex; justify-content:space-between; align-items:center;';

        const left = document.createElement('div');
        left.style.cssText = 'display:flex; align-items:center; gap:8px; min-width:0;';

        // Type badge
        const badge = document.createElement('span');
        const pType = (plugin.plugin_type || '').toLowerCase();
        if (pType === 'lua') {
          badge.style.cssText = 'background:#a6e3a1; color:#1e1e2e; font-size:9px; padding:1px 6px; border-radius:3px; text-transform:uppercase; font-weight:600; flex-shrink:0;';
        } else {
          badge.style.cssText = 'background:#f9e2af; color:#1e1e2e; font-size:9px; padding:1px 6px; border-radius:3px; text-transform:uppercase; font-weight:600; flex-shrink:0;';
        }
        badge.textContent = pType;
        left.appendChild(badge);

        // Name + meta
        const info = document.createElement('div');
        info.style.cssText = 'min-width:0;';
        const nameEl = document.createElement('div');
        nameEl.style.fontWeight = 'bold';
        nameEl.textContent = plugin.name;
        info.appendChild(nameEl);
        const meta = document.createElement('div');
        meta.style.cssText = 'color:var(--dim-fg); font-size:10px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap;';
        meta.textContent = (plugin.version || '') + ' \u2014 ' + (plugin.path || '');
        info.appendChild(meta);
        left.appendChild(info);

        row.appendChild(left);

        // Enable/Disable button
        const actionBtn = document.createElement('button');
        actionBtn.className = 'ssh-form-btn';
        actionBtn.style.cssText = 'flex-shrink:0; margin-left:8px;';
        actionBtn.textContent = plugin.loaded ? 'Disable' : 'Enable';
        actionBtn.addEventListener('click', async () => {
          actionBtn.disabled = true;
          try {
            if (plugin.loaded) {
              await invoke('disable_plugin', { name: plugin.name, source: plugin.source });
              await invoke('rebuild_menu').catch(() => {});
              if (window.toast) window.toast.info('Plugin Disabled', plugin.name);
            } else {
              await invoke('enable_plugin', { name: plugin.name, source: plugin.source, path: plugin.path });
              await invoke('rebuild_menu').catch(() => {});
              if (window.toast) window.toast.success('Plugin Enabled', plugin.name);
            }
            cachedPlugins = await invoke('scan_plugins');
          } catch (e) {
            if (window.toast) window.toast.error('Plugin Action Failed', String(e));
          }
          actionBtn.disabled = false;
          renderPluginList();
        });
        row.appendChild(actionBtn);

        pluginListContainer.appendChild(row);
      }
    }

    renderPluginList();
  }

  // --- Advanced section ---

  function renderAdvanced(c) {
    const h = document.createElement('h3');
    h.textContent = 'Advanced';
    c.appendChild(h);

    // Updates
    addSectionLabel(c, 'Updates');

    const updateSwitch = makeSwitch(
      pendingSettings.conch.check_for_updates !== false,
      (val) => { pendingSettings.conch.check_for_updates = val; }
    );
    addRow(c, 'Check for updates on startup', 'Automatically check for new versions when the app starts (macOS and Windows)', updateSwitch);

    addDivider(c);

    // Sub-group: Initial Window Size
    addSectionLabel(c, 'Initial Window Size');

    const colsInput = makeInput('number', pendingSettings.window.dimensions.columns);
    colsInput.addEventListener('input', () => {
      const v = parseInt(colsInput.value, 10);
      if (!isNaN(v)) pendingSettings.window.dimensions.columns = v;
    });
    addRow(c, 'Columns', 'Width in character cells (0 = system default)', colsInput);

    const linesInput = makeInput('number', pendingSettings.window.dimensions.lines);
    linesInput.addEventListener('input', () => {
      const v = parseInt(linesInput.value, 10);
      if (!isNaN(v)) pendingSettings.window.dimensions.lines = v;
    });
    addRow(c, 'Lines', 'Height in character cells (0 = system default)', linesInput);

    addDivider(c);

    // Sub-group: UI Chrome Font Sizes
    addSectionLabel(c, 'UI Chrome Font Sizes');

    const fontNote = document.createElement('div');
    fontNote.className = 'settings-row-desc';
    fontNote.style.marginBottom = '8px';
    fontNote.textContent = 'Fine-tune text sizes for different UI elements (in points)';
    c.appendChild(fontNote);

    const smallInput = makeInput('number', pendingSettings.conch.ui.font.small, { step: '0.5' });
    smallInput.addEventListener('input', () => {
      const v = parseFloat(smallInput.value);
      if (!isNaN(v)) pendingSettings.conch.ui.font.small = v;
    });
    addRow(c, 'Small', 'Tab titles, badges, compact labels', smallInput);

    const listInput = makeInput('number', pendingSettings.conch.ui.font.list, { step: '0.5' });
    listInput.addEventListener('input', () => {
      const v = parseFloat(listInput.value);
      if (!isNaN(v)) pendingSettings.conch.ui.font.list = v;
    });
    addRow(c, 'List', 'Tree nodes, table rows, file explorer', listInput);

    const normalInput = makeInput('number', pendingSettings.conch.ui.font.normal, { step: '0.5' });
    normalInput.addEventListener('input', () => {
      const v = parseFloat(normalInput.value);
      if (!isNaN(v)) pendingSettings.conch.ui.font.normal = v;
    });
    addRow(c, 'Normal', 'Body text, buttons, inputs, dialogs', normalInput);

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
    c.appendChild(resetLink);
  }

  async function applySettings() {
    try {
      const result = await invoke('save_settings', { settings: pendingSettings });
      close();
      if (result && result.restart_required) {
        if (window.toast) window.toast.warn('Restart Required', 'Some changes require a restart to take effect.');
      }
    } catch (e) {
      if (window.toast) window.toast.error('Settings Error', 'Failed to save settings: ' + e);
    }
  }

  exports.settings = { init, open, close };
})(window);
