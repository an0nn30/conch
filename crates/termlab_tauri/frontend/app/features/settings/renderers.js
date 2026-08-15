// Settings feature — shared DOM helpers, dialog/standalone shells, theme
// preview, shortcut recording widgets, and section render dispatch.

(function initTermLabSettingsRenderers(global) {
  'use strict';

  const searchFeature = global.termlabSettingsFeatureSearch || {};

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
    if (labelText) row.dataset.searchLabel = searchFeature.normalizeSearchText(labelText);
    if (descText) row.dataset.searchDesc = searchFeature.normalizeSearchText(descText);
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
    return row;
  }

  function setRowTarget(row, settingId) {
    if (row && settingId) row.dataset.settingId = settingId;
    return row;
  }

  function applyRowSearchHighlight(row, labelText, descText, query) {
    if (!row || !query) return;
    const labelEl = row.querySelector('.settings-row-label');
    if (labelEl) {
      labelEl.textContent = '';
      searchFeature.appendHighlightedText(labelEl, labelText, query);
    }
    const descEl = row.querySelector('.settings-row-desc');
    if (descEl && descText) {
      descEl.textContent = '';
      searchFeature.appendHighlightedText(descEl, descText, query);
    }
  }

  function addSearchInput(container, placeholder, value, onInput) {
    const wrap = document.createElement('div');
    wrap.className = 'settings-search-wrap';
    const input = document.createElement('input');
    input.type = 'search';
    input.className = 'settings-input settings-search-input';
    input.placeholder = placeholder;
    input.value = value || '';
    input.addEventListener('input', () => onInput(input.value));
    wrap.appendChild(input);
    container.appendChild(wrap);
    return input;
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
    group.setAttribute('role', 'radiogroup');
    const setActive = (activeBtn) => {
      for (const child of group.children) {
        child.classList.remove('active');
        child.setAttribute('aria-checked', child === activeBtn ? 'true' : 'false');
      }
      activeBtn.classList.add('active');
      activeBtn.setAttribute('aria-checked', 'true');
    };
    for (const opt of options) {
      const btn = document.createElement('div');
      btn.className = 'settings-toggle' + (opt.value === activeValue ? ' active' : '');
      btn.textContent = opt.label;
      btn.setAttribute('role', 'radio');
      btn.setAttribute('aria-checked', opt.value === activeValue ? 'true' : 'false');
      btn.tabIndex = 0;
      const activate = () => {
        onChange(opt.value);
        setActive(btn);
      };
      btn.addEventListener('click', activate);
      btn.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        activate();
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
      span('tp-green tp-bold', 'user@termlab'),
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
      span('tp-green tp-bold', 'user@termlab'),
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
      span('tp-green tp-bold', 'user@termlab'),
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

  // --- Keyboard shortcut display/recording ---

  const isMac = typeof navigator !== 'undefined' && navigator.platform.includes('Mac');

  /** Convert config shortcut string to display string, e.g. "cmd+shift+t" -> "⌘ ⇧ T" */
  function formatShortcut(combo) {
    if (!combo) return '';
    const parts = combo.split('+');
    const display = [];
    for (const p of parts) {
      switch (p) {
        case 'cmd':   display.push(isMac ? '⌘' : 'Ctrl'); break;
        case 'shift': display.push('⇧'); break;
        case 'alt':   display.push(isMac ? '⌥' : 'Alt'); break;
        case 'ctrl':  display.push(isMac ? '⌃' : 'Ctrl'); break;
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

  function shortcutText(value) {
    const formatted = formatShortcut(value);
    return formatted || 'Unassigned';
  }

  /**
   * Shortcut-recording controller.
   * deps: { getShortcutValue, setShortcutValue, registerGlobalKeyHandler }
   */
  function createShortcutRecorder(deps) {
    const d = deps || {};

    // Currently recording shortcut state
    let recordingEl = null;
    let recordingRef = null;
    let recordingUnregister = null;

    function stopRecording() {
      if (recordingEl) {
        recordingEl.classList.remove('recording');
        recordingEl.textContent = shortcutText(d.getShortcutValue(recordingRef));
      }
      if (typeof recordingUnregister === 'function') {
        recordingUnregister();
        recordingUnregister = null;
      }
      recordingEl = null;
      recordingRef = null;
    }

    function startRecording(el, settingsRef) {
      // Stop any existing recording first
      stopRecording();

      recordingEl = el;
      recordingRef = settingsRef;
      el.classList.add('recording');
      el.textContent = 'Press keys...';

      recordingUnregister = d.registerGlobalKeyHandler('settings-shortcut-recorder', (e) => {
        e.preventDefault();
        e.stopPropagation();

        // Escape cancels recording
        if (e.key === 'Escape') {
          stopRecording();
          return true;
        }

        const combo = normalizeKeyEvent(e);
        if (!combo) return true; // bare modifier, keep waiting

        d.setShortcutValue(settingsRef, combo);
        stopRecording();
        return true;
      }, () => !!recordingEl && !!recordingRef);
    }

    function makeShortcutKeyBox(ref) {
      const keyBox = document.createElement('span');
      keyBox.className = 'settings-shortcut-key';
      keyBox.setAttribute('role', 'button');
      keyBox.tabIndex = 0;
      keyBox.setAttribute('aria-label', 'Record shortcut');
      keyBox.textContent = shortcutText(d.getShortcutValue(ref));
      keyBox.addEventListener('click', () => startRecording(keyBox, ref));
      keyBox.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        startRecording(keyBox, ref);
      });
      return keyBox;
    }

    return {
      makeShortcutKeyBox,
      startRecording,
      stopRecording,
      isRecording: () => !!recordingEl,
    };
  }

  // --- Search jump highlighting ---

  function applyPendingSettingsJump(root, jump) {
    if (!jump || !root) return false;
    let row = null;
    if (jump.targetId) {
      row = root.querySelector(`[data-setting-id="${jump.targetId}"]`);
    }
    if (!row && jump.label) {
      const normalized = searchFeature.normalizeSearchText(jump.label);
      row = root.querySelector(`.settings-row[data-search-label="${normalized}"]`);
    }
    if (!row && jump.query) {
      const q = searchFeature.normalizeSearchText(jump.query);
      row = Array.from(root.querySelectorAll('.settings-row')).find((el) => {
        const label = el.dataset.searchLabel || '';
        const desc = el.dataset.searchDesc || '';
        return label.includes(q) || desc.includes(q);
      }) || null;
    }
    if (!row && jump.targetId && jump.targetId.startsWith('plugin-setting:')) {
      const parts = jump.targetId.split(':');
      const widgetId = parts.length >= 3 ? parts[parts.length - 1] : '';
      if (widgetId) {
        const widget = root.querySelector(`[data-pw-id="${CSS.escape(widgetId)}"]`);
        if (widget) {
          const highlightTarget =
            widget.closest(`[data-plugin-setting-id="${CSS.escape(widgetId)}"]`)
            || widget.closest('.plugin-settings-content')
            || widget;
          highlightTarget.scrollIntoView({ behavior: 'smooth', block: 'center' });
          if (typeof widget.focus === 'function' && !widget.disabled) {
            widget.focus({ preventScroll: true });
          }
          highlightTarget.classList.remove('plugin-setting-jump-highlight');
          void highlightTarget.offsetWidth;
          highlightTarget.classList.add('plugin-setting-jump-highlight');
          return true;
        }
      }
    }
    if (row) {
      row.scrollIntoView({ behavior: 'smooth', block: 'center' });
      row.classList.remove('settings-row-jump-highlight');
      void row.offsetWidth;
      row.classList.add('settings-row-jump-highlight');
      return true;
    }
    return false;
  }

  // --- Dialog / standalone window shells ---

  /**
   * Build the in-app modal dialog shell and attach it to document.body.
   * deps: { close, applySettings, renderSidebarInto, isRecording, setSidebarQuery }
   */
  function renderDialogShell(deps) {
    const d = deps || {};

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.id = 'settings-overlay';
    overlay.setAttribute('role', 'dialog');
    overlay.setAttribute('aria-modal', 'true');
    overlay.setAttribute('aria-label', 'Settings');

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
    d.renderSidebarInto(sidebar);
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
    cancelBtn.addEventListener('click', d.close);
    const applyBtn = document.createElement('button');
    applyBtn.className = 'ssh-form-btn primary';
    applyBtn.textContent = 'Apply';
    applyBtn.addEventListener('click', d.applySettings);
    footer.appendChild(cancelBtn);
    footer.appendChild(applyBtn);
    dialog.appendChild(footer);

    overlay.appendChild(dialog);

    // Click outside to close
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) d.close(); });

    dialog.addEventListener('keydown', (e) => {
      if (d.isRecording()) return;
      if (!searchFeature.isPrintableKeyEvent(e)) return;
      const active = document.activeElement;
      if (active && active.closest && active.closest('#settings-overlay') && searchFeature.isTextLikeElement(active)) return;
      const input = dialog.querySelector('.settings-sidebar-search');
      if (!input) return;
      e.preventDefault();
      e.stopPropagation();
      input.focus();
      input.value = (input.value || '') + e.key;
      d.setSidebarQuery(input.value);
      d.renderSidebarInto(document.getElementById('settings-sidebar'));
      const nextInput = document.querySelector('#settings-sidebar .settings-sidebar-search');
      if (nextInput) {
        nextInput.focus();
        nextInput.setSelectionRange(nextInput.value.length, nextInput.value.length);
      }
    }, true);

    document.body.appendChild(overlay);
    return overlay;
  }

  /**
   * Build the standalone-window shell (no overlay, no modal) into rootEl.
   * deps: { close, applySettings, renderSidebarInto, isRecording, setSidebarQuery }
   */
  function renderStandaloneShell(root, deps) {
    const d = deps || {};
    root.innerHTML = '';

    // Title bar (also serves as drag region)
    const title = document.createElement('div');
    title.className = 'settings-title';
    title.textContent = 'Settings';
    title.setAttribute('data-tauri-drag-region', '');
    root.appendChild(title);

    // Body = sidebar + content
    const body = document.createElement('div');
    body.className = 'settings-body';

    const sidebar = document.createElement('div');
    sidebar.className = 'settings-sidebar';
    sidebar.id = 'settings-sidebar';
    d.renderSidebarInto(sidebar);
    body.appendChild(sidebar);

    const content = document.createElement('div');
    content.className = 'settings-content';
    content.id = 'settings-content';
    body.appendChild(content);

    root.appendChild(body);

    // Footer
    const footer = document.createElement('div');
    footer.className = 'settings-footer';
    const cancelBtn = document.createElement('button');
    cancelBtn.className = 'ssh-form-btn';
    cancelBtn.textContent = 'Cancel';
    cancelBtn.addEventListener('click', d.close);
    const applyBtn = document.createElement('button');
    applyBtn.className = 'ssh-form-btn primary';
    applyBtn.textContent = 'Apply';
    applyBtn.addEventListener('click', d.applySettings);
    footer.appendChild(cancelBtn);
    footer.appendChild(applyBtn);
    root.appendChild(footer);

    root.addEventListener('keydown', (e) => {
      if (d.isRecording()) return;
      if (!searchFeature.isPrintableKeyEvent(e)) return;
      const active = document.activeElement;
      if (searchFeature.isTextLikeElement(active)) return;
      const input = root.querySelector('.settings-sidebar-search');
      if (!input) return;
      e.preventDefault();
      e.stopPropagation();
      input.focus();
      input.value = (input.value || '') + e.key;
      d.setSidebarQuery(input.value);
      const sidebarEl = document.getElementById('settings-sidebar');
      if (sidebarEl) d.renderSidebarInto(sidebarEl);
      const nextInput = root.querySelector('.settings-sidebar-search');
      if (nextInput) {
        nextInput.focus();
        nextInput.setSelectionRange(nextInput.value.length, nextInput.value.length);
      }
    }, true);
  }

  // --- Section render dispatch ---

  /**
   * Build the per-section render dispatcher.
   * deps: {
   *   store, recorder, getInvoke, renderCurrentSection, renderSidebarInto,
   *   refreshPluginInventory, confirmPluginPermissions, invalidateCommandPaletteCache
   * }
   */
  function createSectionRenderers(deps) {
    const d = deps || {};
    const store = d.store;
    const recorder = d.recorder;

    function moduleUnavailable(label) {
      if (global.toast && typeof global.toast.error === 'function') {
        global.toast.error('Settings Error', label + ' section module is unavailable.');
      }
    }

    function renderAppearance(c) {
      const settingsSectionsAppearance = global.termlabSettingsSectionsAppearance || {};
      if (settingsSectionsAppearance && typeof settingsSectionsAppearance.renderAppearance === 'function') {
        const handled = settingsSectionsAppearance.renderAppearance(c, {
          pendingSettings: store.getPendingSettings(),
          cachedThemes: store.getCachedThemes(),
          cachedFonts: store.getCachedFonts(),
          addSectionLabel,
          addRow,
          setRowTarget,
          addDivider,
          buildThemePreview,
          updateThemePreview,
          invoke: d.getInvoke(),
          makeSwitch,
        });
        if (handled) return;
      }
      moduleUnavailable('Appearance');
    }

    function renderKeyboard(c) {
      const settingsSectionsKeyboard = global.termlabSettingsSectionsKeyboard || {};
      if (settingsSectionsKeyboard && typeof settingsSectionsKeyboard.renderKeyboard === 'function') {
        const handled = settingsSectionsKeyboard.renderKeyboard(c, {
          stopRecording: () => recorder.stopRecording(),
          addSearchInput,
          normalizeSearchText: searchFeature.normalizeSearchText,
          getFuzzyMatchScore: searchFeature.getFuzzyMatchScore,
          getKeyboardSearchQuery: () => store.getKeyboardSearchQuery(),
          setKeyboardSearchQuery: (value) => {
            store.setKeyboardSearchQuery(value);
          },
          renderCurrentSection: () => d.renderCurrentSection(),
          KEYBOARD_CORE_GROUPS: store.KEYBOARD_CORE_GROUPS,
          KEYBOARD_CORE_LABELS: store.KEYBOARD_CORE_LABELS,
          getPendingKeyboardMap: () => store.getPendingKeyboardMap(),
          addSectionLabel,
          addRow,
          setRowTarget,
          applyRowSearchHighlight,
          addDivider,
          makeShortcutKeyBox: (ref) => recorder.makeShortcutKeyBox(ref),
          getToolWindowItems: () => (
            global.toolWindowManager && typeof global.toolWindowManager.listWindows === 'function'
              ? global.toolWindowManager.listWindows().slice().sort((a, b) => {
                  const typeCmp = String(a.type || '').localeCompare(String(b.type || ''));
                  if (typeCmp !== 0) return typeCmp;
                  return String(a.title || '').localeCompare(String(b.title || ''));
                })
              : []
          ),
          getPluginMenuItems: () => store.getCachedPluginMenuItems() || [],
        });
        if (handled) return;
      }
      moduleUnavailable('Keyboard');
    }

    function renderTerminal(c) {
      const settingsSectionsTerminal = global.termlabSettingsSectionsTerminal || {};
      if (!settingsSectionsTerminal || typeof settingsSectionsTerminal.renderTerminal !== 'function') {
        moduleUnavailable('Terminal');
        return;
      }
      settingsSectionsTerminal.renderTerminal(c, {
        pendingSettings: store.getPendingSettings(),
        cachedFonts: store.getCachedFonts(),
        addSectionLabel,
        addDivider,
        addRow,
        setRowTarget,
        makeInput,
      });
    }

    function renderShell(c) {
      const settingsSectionsTerminal = global.termlabSettingsSectionsTerminal || {};
      if (!settingsSectionsTerminal || typeof settingsSectionsTerminal.renderShell !== 'function') {
        moduleUnavailable('Shell');
        return;
      }
      settingsSectionsTerminal.renderShell(c, {
        pendingSettings: store.getPendingSettings(),
        addSectionLabel,
        addDivider,
        addRow,
        setRowTarget,
        makeInput,
      });
    }

    function renderCursor(c) {
      const settingsSectionsTerminal = global.termlabSettingsSectionsTerminal || {};
      if (!settingsSectionsTerminal || typeof settingsSectionsTerminal.renderCursor !== 'function') {
        moduleUnavailable('Cursor');
        return;
      }
      settingsSectionsTerminal.renderCursor(c, {
        pendingSettings: store.getPendingSettings(),
        addSectionLabel,
        addDivider,
        addRow,
        setRowTarget,
        makeSwitch,
        makeToggleGroup,
      });
    }

    function renderAdvanced(c) {
      const settingsSectionsBasic = global.termlabSettingsSectionsBasic || {};
      if (!settingsSectionsBasic || typeof settingsSectionsBasic.renderAdvanced !== 'function') {
        moduleUnavailable('Advanced');
        return;
      }
      settingsSectionsBasic.renderAdvanced(c, {
        pendingSettings: store.getPendingSettings(),
        addSectionLabel,
        addDivider,
        addRow,
        setRowTarget,
        makeSwitch,
        makeInput,
      });
    }

    function renderFiles(c) {
      const settingsSectionsBasic = global.termlabSettingsSectionsBasic || {};
      if (!settingsSectionsBasic || typeof settingsSectionsBasic.renderFiles !== 'function') {
        moduleUnavailable('Files');
        return;
      }
      settingsSectionsBasic.renderFiles(c, {
        pendingSettings: store.getPendingSettings(),
        addSectionLabel,
        addRow,
        setRowTarget,
        makeSwitch,
      });
    }

    function renderPlugins(c) {
      const settingsPluginsSection = global.termlabSettingsPluginsSection || {};
      if (settingsPluginsSection && typeof settingsPluginsSection.createRenderer === 'function') {
        const renderer = settingsPluginsSection.createRenderer({
          invoke: d.getInvoke(),
          getPendingSettings: () => store.getPendingSettings(),
          getCachedPlugins: () => store.getCachedPlugins(),
          setCachedPlugins: (next) => store.setCachedPlugins(next),
          setCachedPluginMenuItems: (next) => store.setCachedPluginMenuItems(next),
          setCachedPluginSettingsSections: (next) => store.setCachedPluginSettingsSections(next),
          refreshPluginInventory: () => d.refreshPluginInventory(),
          onPluginInventoryUpdated: () => {
            if (!store.getSectionById(store.getCurrentSection())) {
              store.setCurrentSection('plugins');
            }
            const sidebar = document.getElementById('settings-sidebar');
            if (sidebar) d.renderSidebarInto(sidebar);
            d.renderCurrentSection();
          },
          confirmPluginPermissions: (pluginName, permissions) => d.confirmPluginPermissions(pluginName, permissions),
          invalidateCommandPaletteCache: (reason) => d.invalidateCommandPaletteCache(reason),
          addSectionLabel,
          addDivider,
          addRow,
          setRowTarget,
          makeInput,
          makeSwitch,
        });
        renderer.renderPlugins(c);
        return;
      }

      const fallback = document.createElement('div');
      fallback.className = 'settings-row-desc';
      fallback.textContent = 'Plugin settings UI module is unavailable.';
      c.appendChild(fallback);
    }

    function renderPluginSettings(c, sectionKey) {
      const section = store.getPluginSettingsSectionByKey(sectionKey);
      if (!section) {
        const fallback = document.createElement('div');
        fallback.className = 'settings-row-desc';
        fallback.textContent = 'Plugin settings section is unavailable.';
        c.appendChild(fallback);
        return;
      }

      const host = document.createElement('div');
      host.className = 'plugin-settings-content';
      host.dataset.pluginName = section.plugin_name || '';
      host.dataset.pluginViewId = section.view_id || '';
      c.appendChild(host);

      const loading = document.createElement('div');
      loading.className = 'settings-row-desc';
      loading.textContent = 'Loading plugin settings…';
      host.appendChild(loading);

      const pluginWidgets = global.pluginWidgets;
      if (!pluginWidgets || typeof pluginWidgets.renderWidgets !== 'function') {
        host.innerHTML = '';
        const missing = document.createElement('div');
        missing.className = 'settings-row-desc';
        missing.textContent = 'Plugin widget runtime is unavailable.';
        host.appendChild(missing);
        return;
      }

      const pluginName = section.plugin_name || '';
      const viewId = section.view_id || section.section_id || '';
      d.getInvoke()('request_plugin_render', { pluginName, viewId })
        .then((widgetsJson) => {
          host.innerHTML = '';
          pluginWidgets.renderWidgets(host, widgetsJson || '[]', pluginName, viewId);
          const jump = store.getPendingSettingsJump();
          if (jump && jump.section === sectionKey) {
            requestAnimationFrame(() => {
              const root = document.getElementById('settings-content');
              if (!root) return;
              if (applyPendingSettingsJump(root, store.getPendingSettingsJump())) {
                store.clearPendingSettingsJump();
              }
            });
          }
        })
        .catch((error) => {
          host.innerHTML = '';
          const failed = document.createElement('div');
          failed.className = 'settings-row-desc';
          failed.textContent = 'Failed to load plugin settings UI: ' + String(error);
          host.appendChild(failed);
          const jump = store.getPendingSettingsJump();
          if (jump && jump.section === sectionKey) {
            store.clearPendingSettingsJump();
          }
        });
    }

    function renderSection(content, sectionId) {
      switch (sectionId) {
        case 'appearance': renderAppearance(content); break;
        case 'keyboard': renderKeyboard(content); break;
        case 'files': renderFiles(content); break;
        case 'terminal': renderTerminal(content); break;
        case 'shell': renderShell(content); break;
        case 'cursor': renderCursor(content); break;
        case 'plugins': renderPlugins(content); break;
        case 'advanced': renderAdvanced(content); break;
        default:
          if (store.isPluginSettingsSectionId(sectionId)) {
            renderPluginSettings(content, sectionId);
            break;
          }
          renderAppearance(content);
          break;
      }
    }

    return { renderSection };
  }

  global.termlabSettingsRenderers = {
    addSectionLabel,
    addDivider,
    addRow,
    setRowTarget,
    applyRowSearchHighlight,
    addSearchInput,
    makeInput,
    makeToggleGroup,
    makeSwitch,
    buildThemePreview,
    updateThemePreview,
    formatShortcut,
    normalizeKeyEvent,
    shortcutText,
    createShortcutRecorder,
    applyPendingSettingsJump,
    renderDialogShell,
    renderStandaloneShell,
    createSectionRenderers,
  };
})(window);
