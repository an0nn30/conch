// Settings Dialog — sidebar navigation, content area switching, Apply/Cancel.

(function (exports) {
  'use strict';

  let invoke = null;
  let escapeHandler = null;
  let standaloneEscapeHandler = null;
  let currentSection = 'appearance';
  let pendingSettings = null;
  let originalSettings = null;
  let cachedThemes = [];
  let cachedPlugins = [];
  let cachedPluginMenuItems = [];
  let cachedFonts = { all: [], monospace: [] };
  let standaloneMode = false;   // true when running in its own window
  let standaloneRoot = null;    // root element in standalone mode
  let settingsSidebarQuery = '';
  let keyboardSearchQuery = '';
  let settingsSearchAutofocusTimer = null;
  let settingsSidebarResults = [];
  let settingsSidebarSelectionIndex = -1;
  let pendingSettingsJump = null;
  const settingsFeatureConstants = exports.conchSettingsFeatureConstants || {};
  const settingsDataService = exports.conchSettingsFeatureDataService || {};
  const settingsSearchFeature = exports.conchSettingsFeatureSearch || {};
  const settingsSidebarFeature = exports.conchSettingsSidebar || {};
  const settingsSectionsAppearance = exports.conchSettingsSectionsAppearance || {};
  const settingsSectionsBasic = exports.conchSettingsSectionsBasic || {};
  const settingsSectionsKeyboard = exports.conchSettingsSectionsKeyboard || {};
  const settingsSectionsTerminal = exports.conchSettingsSectionsTerminal || {};
  const settingsPluginsSection = exports.conchSettingsPluginsSection || {};
  const SECTION_DEFS = Array.isArray(settingsFeatureConstants.SECTION_DEFS)
    ? settingsFeatureConstants.SECTION_DEFS
    : [];
  const SETTINGS_SEARCH_INDEX = Array.isArray(settingsFeatureConstants.SETTINGS_SEARCH_INDEX)
    ? settingsFeatureConstants.SETTINGS_SEARCH_INDEX
    : [];

  function registerGlobalKeyHandler(name, onKeyDown, isActive) {
    const keyboardRouter = window.conchKeyboardRouter;
    if (keyboardRouter && typeof keyboardRouter.register === 'function') {
      return keyboardRouter.register({
        name: name || 'settings-key-handler',
        priority: 210,
        isActive: typeof isActive === 'function' ? isActive : null,
        onKeyDown: (event) => onKeyDown(event) === true,
      });
    }

    console.warn('settings: keyboard router unavailable, skipping handler registration:', name || 'settings-key-handler');
    return () => {};
  }

  function clearSettingsAutofocusTimer() {
    if (settingsSearchAutofocusTimer) {
      clearTimeout(settingsSearchAutofocusTimer);
      settingsSearchAutofocusTimer = null;
    }
  }

  function init(opts) {
    invoke = opts.invoke;
  }

  async function loadSettingsRuntimeData() {
    if (settingsDataService && typeof settingsDataService.loadRuntimeData === 'function') {
      return settingsDataService.loadRuntimeData(invoke);
    }
    const [settings, themes, plugins, pluginMenuItems, fonts] = await Promise.all([
      invoke('get_all_settings'),
      invoke('list_themes'),
      invoke('scan_plugins'),
      invoke('get_plugin_menu_items').catch(() => []),
      invoke('list_system_fonts'),
    ]);
    return {
      settings,
      themes,
      plugins: Array.isArray(plugins) ? plugins : [],
      pluginMenuItems: Array.isArray(pluginMenuItems) ? pluginMenuItems : [],
      fonts: fonts && typeof fonts === 'object' ? fonts : { all: [], monospace: [] },
    };
  }

  async function refreshPluginInventory() {
    if (settingsDataService && typeof settingsDataService.refreshPluginInventory === 'function') {
      return settingsDataService.refreshPluginInventory(invoke);
    }
    const [plugins, pluginMenuItems] = await Promise.all([
      invoke('scan_plugins'),
      invoke('get_plugin_menu_items').catch(() => []),
    ]);
    return {
      plugins: Array.isArray(plugins) ? plugins : [],
      pluginMenuItems: Array.isArray(pluginMenuItems) ? pluginMenuItems : [],
    };
  }

  function applyLoadedSettingsData(payload) {
    const loaded = payload || {};
    originalSettings = JSON.parse(JSON.stringify(loaded.settings || {}));
    pendingSettings = JSON.parse(JSON.stringify(loaded.settings || {}));
    ensureSettingsShape(originalSettings);
    ensureSettingsShape(pendingSettings);
    cachedThemes = Array.isArray(loaded.themes) ? loaded.themes : [];
    cachedPlugins = Array.isArray(loaded.plugins) ? loaded.plugins : [];
    cachedPluginMenuItems = Array.isArray(loaded.pluginMenuItems) ? loaded.pluginMenuItems : [];
    cachedFonts = loaded.fonts && typeof loaded.fonts === 'object' ? loaded.fonts : { all: [], monospace: [] };
    settingsSidebarQuery = '';
    keyboardSearchQuery = '';
    currentSection = 'appearance';
  }

  function invalidateCommandPaletteCache(reason) {
    if (typeof window.__conchInvalidateCommandPaletteCache === 'function') {
      window.__conchInvalidateCommandPaletteCache(reason || 'settings');
    }
  }

  function ensureSettingsShape(settings) {
    if (!settings.conch) settings.conch = {};
    if (!settings.conch.ui || typeof settings.conch.ui !== 'object') {
      settings.conch.ui = {};
    }
    if (typeof settings.conch.ui.disable_animations !== 'boolean') {
      settings.conch.ui.disable_animations = false;
    }
    if (!settings.conch.files || typeof settings.conch.files !== 'object') {
      settings.conch.files = {};
    }
    if (typeof settings.conch.files.follow_path !== 'boolean') {
      settings.conch.files.follow_path = true;
    }
  }

  function normalizeSearchText(value) {
    if (settingsSearchFeature && typeof settingsSearchFeature.normalizeSearchText === 'function') {
      return settingsSearchFeature.normalizeSearchText(value);
    }
    return String(value || '').trim().toLowerCase();
  }

  function tokenizeSearchText(value) {
    if (settingsSearchFeature && typeof settingsSearchFeature.tokenizeSearchText === 'function') {
      return settingsSearchFeature.tokenizeSearchText(value);
    }
    return normalizeSearchText(value).split(/[\s:_-]+/).filter(Boolean);
  }

  function levenshteinDistance(a, b) {
    if (settingsSearchFeature && typeof settingsSearchFeature.levenshteinDistance === 'function') {
      return settingsSearchFeature.levenshteinDistance(a, b);
    }
    const left = String(a || '');
    const right = String(b || '');
    if (!left) return right.length;
    if (!right) return left.length;
    const prev = new Array(right.length + 1);
    const curr = new Array(right.length + 1);
    for (let j = 0; j <= right.length; j++) prev[j] = j;
    for (let i = 1; i <= left.length; i++) {
      curr[0] = i;
      for (let j = 1; j <= right.length; j++) {
        const cost = left.charCodeAt(i - 1) === right.charCodeAt(j - 1) ? 0 : 1;
        curr[j] = Math.min(
          prev[j] + 1,
          curr[j - 1] + 1,
          prev[j - 1] + cost,
        );
      }
      for (let j = 0; j <= right.length; j++) prev[j] = curr[j];
    }
    return prev[right.length];
  }

  function getFuzzyMatchScore(query, haystack, extraTokens) {
    if (settingsSearchFeature && typeof settingsSearchFeature.getFuzzyMatchScore === 'function') {
      return settingsSearchFeature.getFuzzyMatchScore(query, haystack, extraTokens);
    }
    const q = normalizeSearchText(query);
    const text = normalizeSearchText(haystack);
    if (!q || !text) return Number.POSITIVE_INFINITY;
    if (text.includes(q)) return 0;

    const tokens = new Set([
      ...tokenizeSearchText(text),
      ...(Array.isArray(extraTokens) ? extraTokens.flatMap((item) => tokenizeSearchText(item)) : []),
    ]);
    if (tokens.size === 0) return Number.POSITIVE_INFINITY;

    let best = Number.POSITIVE_INFINITY;
    for (const token of tokens) {
      if (!token) continue;
      if (token.startsWith(q) || q.startsWith(token)) {
        best = Math.min(best, 1);
        continue;
      }
      if (token.includes(q) || q.includes(token)) {
        best = Math.min(best, 1);
        continue;
      }
      if (q.length >= 4 && token.length >= 4) {
        const distance = levenshteinDistance(q, token);
        if (distance <= 2) {
          best = Math.min(best, 2 + distance);
        }
      }
    }
    return best;
  }

  function isPrintableKeyEvent(event) {
    if (settingsSearchFeature && typeof settingsSearchFeature.isPrintableKeyEvent === 'function') {
      return settingsSearchFeature.isPrintableKeyEvent(event);
    }
    return !!(
      event &&
      !event.metaKey &&
      !event.ctrlKey &&
      !event.altKey &&
      typeof event.key === 'string' &&
      event.key.length === 1
    );
  }

  function isTextLikeElement(el) {
    if (settingsSearchFeature && typeof settingsSearchFeature.isTextLikeElement === 'function') {
      return settingsSearchFeature.isTextLikeElement(el);
    }
    if (!el) return false;
    const tag = String(el.tagName || '').toLowerCase();
    return (
      tag === 'input' ||
      tag === 'textarea' ||
      tag === 'select' ||
      el.isContentEditable
    );
  }

  function escapeRegExp(value) {
    if (settingsSearchFeature && typeof settingsSearchFeature.escapeRegExp === 'function') {
      return settingsSearchFeature.escapeRegExp(value);
    }
    return String(value || '').replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }

  function appendHighlightedText(container, text, query) {
    if (settingsSearchFeature && typeof settingsSearchFeature.appendHighlightedText === 'function') {
      return settingsSearchFeature.appendHighlightedText(container, text, query);
    }
    const raw = String(text || '');
    const q = normalizeSearchText(query);
    if (!q) {
      container.textContent = raw;
      return;
    }
    const re = new RegExp(`(${escapeRegExp(q)})`, 'ig');
    let lastIndex = 0;
    for (const match of raw.matchAll(re)) {
      const idx = match.index == null ? -1 : match.index;
      if (idx < 0) continue;
      if (idx > lastIndex) {
        container.appendChild(document.createTextNode(raw.slice(lastIndex, idx)));
      }
      const mark = document.createElement('mark');
      mark.className = 'settings-search-highlight';
      mark.textContent = raw.slice(idx, idx + match[0].length);
      container.appendChild(mark);
      lastIndex = idx + match[0].length;
    }
    if (lastIndex < raw.length) {
      container.appendChild(document.createTextNode(raw.slice(lastIndex)));
    }
  }

  function getSectionById(id) {
    for (const group of SECTION_DEFS) {
      for (const item of group.items) {
        if (item.id === id) return item;
      }
    }
    return null;
  }

  function buildSettingsSearchIndex() {
    const entries = SETTINGS_SEARCH_INDEX.map((entry) => ({ ...entry }));

    for (const group of KEYBOARD_CORE_GROUPS) {
      for (const key of group.keys) {
        const label = KEYBOARD_CORE_LABELS[key] || toTitleCaseWords(key);
        entries.push({
          section: 'keyboard',
          label,
          keywords: `keymap keyboard shortcut ${group.label} ${key} ${label}`,
          path: `Keymap > ${group.label}`,
          kind: 'core-shortcut',
          targetId: `keyboard:core:${key}`,
        });
      }
    }

    const toolWindowItems = window.toolWindowManager && typeof window.toolWindowManager.listWindows === 'function'
      ? window.toolWindowManager.listWindows()
      : [];
    for (const item of toolWindowItems) {
      const title = item.title || item.id;
      const zoneText = String(item.zone || '').replace('-', ' ');
      entries.push({
        section: 'keyboard',
        label: title,
        keywords: `keymap tool windows ${title} ${item.id} ${item.type || ''} ${zoneText}`,
        path: 'Keymap > Tool Windows',
        kind: 'tool-window',
        targetKey: item.id,
      });
    }

    return entries;
  }

  function getSidebarSearchResults(query) {
    const q = normalizeSearchText(query);
    if (!q) return [];

    const results = [];
    const seen = new Set();
    for (const entry of buildSettingsSearchIndex()) {
      const haystack = `${entry.label} ${entry.keywords || ''}`;
      const score = getFuzzyMatchScore(q, haystack, [entry.path, entry.section, entry.kind, entry.targetKey, entry.targetId]);
      if (!Number.isFinite(score)) continue;
      const section = getSectionById(entry.section);
      const sig = `${entry.section}:${entry.label}:${entry.path || ''}`;
      if (seen.has(sig)) continue;
      seen.add(sig);
      results.push({
        section: entry.section,
        label: entry.label,
        sectionLabel: section ? section.label : entry.section,
        path: entry.path || (section ? section.label : entry.section),
        kind: entry.kind || 'setting',
        targetKey: entry.targetKey || null,
        targetId: entry.targetId || (entry.kind === 'tool-window' ? `keyboard:tool-window:${entry.targetKey || ''}` : null),
        score,
      });
    }
    results.sort((a, b) => a.score - b.score || String(a.label).localeCompare(String(b.label)));
    return results;
  }

  function focusSettingsSearchInput(selectAll) {
    clearSettingsAutofocusTimer();
    settingsSearchAutofocusTimer = setTimeout(() => {
      const input = document.querySelector('#settings-sidebar .settings-sidebar-search');
      if (!input) return;
      input.focus();
      if (selectAll) input.select();
    }, 0);
  }

  function moveSidebarSearchSelection(delta) {
    if (settingsSidebarResults.length === 0) return;
    if (settingsSidebarSelectionIndex < 0) {
      settingsSidebarSelectionIndex = delta > 0 ? 0 : settingsSidebarResults.length - 1;
    } else {
      settingsSidebarSelectionIndex = Math.max(0, Math.min(settingsSidebarResults.length - 1, settingsSidebarSelectionIndex + delta));
    }
    const sidebar = document.getElementById('settings-sidebar');
    if (!sidebar) return;
    renderSidebarInto(sidebar);
    const selectedEl = sidebar.querySelector('.settings-sidebar-item.selected');
    if (selectedEl) selectedEl.scrollIntoView({ block: 'nearest' });
    const input = sidebar.querySelector('.settings-sidebar-search');
    if (input) {
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    }
  }

  function registerPendingSettingsJump(match) {
    pendingSettingsJump = match ? {
      section: match.section,
      label: match.label || '',
      targetId: match.targetId || null,
      query: settingsSidebarQuery || '',
    } : null;
  }

  function onSidebarSearchResultSelected(match) {
    if (match.section === 'keyboard' && match.kind === 'tool-window') {
      keyboardSearchQuery = match.label;
    } else if (match.section === 'keyboard') {
      keyboardSearchQuery = match.label;
    }
    registerPendingSettingsJump(match);
    selectSection(match.section);
  }

  function renderSidebarInto(sidebar) {
    if (!settingsSidebarFeature || typeof settingsSidebarFeature.renderSidebarInto !== "function") {
      if (window.toast && typeof window.toast.error === "function") {
        window.toast.error("Settings Error", "Sidebar section module is unavailable.");
      }
      return;
    }
    settingsSidebarFeature.renderSidebarInto(sidebar, {
      sectionDefs: SECTION_DEFS,
      normalizeSearchText,
      getFuzzyMatchScore,
      getSidebarSearchResults,
      appendHighlightedText,
      getSidebarQuery: () => settingsSidebarQuery,
      setSidebarQuery: (value) => { settingsSidebarQuery = value; },
      getSidebarSelectionIndex: () => settingsSidebarSelectionIndex,
      setSidebarSelectionIndex: (value) => { settingsSidebarSelectionIndex = value; },
      getSidebarResults: () => settingsSidebarResults,
      setSidebarResults: (results) => { settingsSidebarResults = Array.isArray(results) ? results : []; },
      getCurrentSection: () => currentSection,
      moveSidebarSearchSelection,
      onSidebarSearchResultSelected,
      selectSection,
    });
  }

  async function open() {
    if (document.getElementById('settings-overlay')) { close(); return; }

    try {
      const loaded = await loadSettingsRuntimeData();
      applyLoadedSettingsData(loaded);
      renderDialog();
    } catch (e) {
      if (window.toast) window.toast.error('Settings', 'Failed to load settings: ' + e);
    }
  }

  /** Open settings in a standalone window (called from settings.html). */
  async function openInWindow(rootEl) {
    standaloneMode = true;
    standaloneRoot = rootEl;

    try {
      const loaded = await loadSettingsRuntimeData();
      applyLoadedSettingsData(loaded);
      renderStandalone();
    } catch (e) {
      if (window.toast) window.toast.error('Settings', 'Failed to load settings: ' + e);
    }
  }

  /** Render settings as a full-window layout (no overlay, no modal). */
  function renderStandalone() {
    const root = standaloneRoot;
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
    renderSidebarInto(sidebar);
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
    cancelBtn.addEventListener('click', close);
    const applyBtn = document.createElement('button');
    applyBtn.className = 'ssh-form-btn primary';
    applyBtn.textContent = 'Apply';
    applyBtn.addEventListener('click', applySettings);
    footer.appendChild(cancelBtn);
    footer.appendChild(applyBtn);
    root.appendChild(footer);

    if (standaloneEscapeHandler) {
      standaloneEscapeHandler();
      standaloneEscapeHandler = null;
    }
    standaloneEscapeHandler = registerGlobalKeyHandler(
      'settings-standalone-escape',
      (event) => {
        if (event.key !== 'Escape') return false;
        if (recordingEl) return false; // let recording handler handle it
        close();
        return true;
      },
      () => standaloneMode && !!standaloneRoot && standaloneRoot.isConnected
    );

    root.addEventListener('keydown', (e) => {
      if (recordingEl) return;
      if (!isPrintableKeyEvent(e)) return;
      const active = document.activeElement;
      if (isTextLikeElement(active)) return;
      const input = root.querySelector('.settings-sidebar-search');
      if (!input) return;
      e.preventDefault();
      e.stopPropagation();
      input.focus();
      input.value = (input.value || '') + e.key;
      settingsSidebarQuery = input.value;
      const sidebarEl = document.getElementById('settings-sidebar');
      if (sidebarEl) renderSidebarInto(sidebarEl);
      const nextInput = root.querySelector('.settings-sidebar-search');
      if (nextInput) {
        nextInput.focus();
        nextInput.setSelectionRange(nextInput.value.length, nextInput.value.length);
      }
    }, true);

    renderCurrentSection();
    focusSettingsSearchInput(true);
  }

  function close() {
    stopRecording();
    clearSettingsAutofocusTimer();
    if (standaloneMode) {
      if (standaloneEscapeHandler) {
        standaloneEscapeHandler();
        standaloneEscapeHandler = null;
      }
      // In standalone window mode, close the window itself.
      const tauri = window.__TAURI__;
      if (tauri) {
        tauri.window.getCurrentWindow().close();
      }
      return;
    }
    const el = document.getElementById('settings-overlay');
    if (el) el.remove();
    if (escapeHandler) {
      escapeHandler();
      escapeHandler = null;
    }
    pendingSettings = null;
    originalSettings = null;
  }

  function renderDialog() {
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
    renderSidebarInto(sidebar);
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

    dialog.addEventListener('keydown', (e) => {
      if (recordingEl) return;
      if (!isPrintableKeyEvent(e)) return;
      const active = document.activeElement;
      if (active && active.closest && active.closest('#settings-overlay') && isTextLikeElement(active)) return;
      const input = dialog.querySelector('.settings-sidebar-search');
      if (!input) return;
      e.preventDefault();
      e.stopPropagation();
      input.focus();
      input.value = (input.value || '') + e.key;
      settingsSidebarQuery = input.value;
      renderSidebarInto(document.getElementById('settings-sidebar'));
      const nextInput = document.querySelector('#settings-sidebar .settings-sidebar-search');
      if (nextInput) {
        nextInput.focus();
        nextInput.setSelectionRange(nextInput.value.length, nextInput.value.length);
      }
    }, true);

    document.body.appendChild(overlay);

    // Escape handler (capture phase, before xterm.js)
    if (escapeHandler) {
      escapeHandler();
      escapeHandler = null;
    }
    escapeHandler = registerGlobalKeyHandler(
      'settings-dialog-escape',
      (event) => {
        if (event.key !== 'Escape') return false;
        // If a shortcut is being recorded, let the recording handler handle Escape.
        if (recordingEl) return false;
        close();
        return true;
      },
      () => !!document.getElementById('settings-overlay')
    );

    // Render initial section
    renderCurrentSection();
    focusSettingsSearchInput(true);
  }

  function selectSection(id) {
    currentSection = id;
    const sidebar = document.getElementById('settings-sidebar');
    if (sidebar) renderSidebarInto(sidebar);
    renderCurrentSection();
  }

  function renderCurrentSection() {
    const content = document.getElementById('settings-content');
    if (!content) return;
    content.innerHTML = '';

    switch (currentSection) {
      case 'appearance': renderAppearance(content); break;
      case 'keyboard': renderKeyboard(content); break;
      case 'files': renderFiles(content); break;
      case 'terminal': renderTerminal(content); break;
      case 'shell': renderShell(content); break;
      case 'cursor': renderCursor(content); break;
      case 'plugins': renderPlugins(content); break;
      case 'advanced': renderAdvanced(content); break;
    }
    if (pendingSettingsJump && pendingSettingsJump.section === currentSection) {
      requestAnimationFrame(() => {
        const root = document.getElementById('settings-content');
        if (!root) return;
        let row = null;
        if (pendingSettingsJump.targetId) {
          row = root.querySelector(`[data-setting-id="${pendingSettingsJump.targetId}"]`);
        }
        if (!row && pendingSettingsJump.label) {
          const normalized = normalizeSearchText(pendingSettingsJump.label);
          row = root.querySelector(`.settings-row[data-search-label="${normalized}"]`);
        }
        if (!row && pendingSettingsJump.query) {
          const q = normalizeSearchText(pendingSettingsJump.query);
          row = Array.from(root.querySelectorAll('.settings-row')).find((el) => {
            const label = el.dataset.searchLabel || '';
            const desc = el.dataset.searchDesc || '';
            return label.includes(q) || desc.includes(q);
          }) || null;
        }
        if (row) {
          row.scrollIntoView({ behavior: 'smooth', block: 'center' });
          row.classList.remove('settings-row-jump-highlight');
          void row.offsetWidth;
          row.classList.add('settings-row-jump-highlight');
        }
        pendingSettingsJump = null;
      });
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
    if (labelText) row.dataset.searchLabel = normalizeSearchText(labelText);
    if (descText) row.dataset.searchDesc = normalizeSearchText(descText);
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
      appendHighlightedText(labelEl, labelText, query);
    }
    const descEl = row.querySelector('.settings-row-desc');
    if (descEl && descText) {
      descEl.textContent = '';
      appendHighlightedText(descEl, descText, query);
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
    if (settingsSectionsAppearance && typeof settingsSectionsAppearance.renderAppearance === 'function') {
      const handled = settingsSectionsAppearance.renderAppearance(c, {
        pendingSettings,
        cachedThemes,
        cachedFonts,
        addSectionLabel,
        addRow,
        setRowTarget,
        addDivider,
        buildThemePreview,
        updateThemePreview,
        invoke,
        makeSwitch,
      });
      if (handled) return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Settings Error', 'Appearance section module is unavailable.');
    }
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

  const KEYBOARD_CORE_LABELS = {
    new_tab: 'New Tab',
    new_plain_shell_tab: 'New Plain Shell Tab',
    close_tab: 'Close Tab',
    rename_tab: 'Rename Tab',
    new_window: 'New Window',
    manage_tunnels: 'Manage SSH Tunnels',
    vault_open: 'Open Credential Vault',
    quit: 'Quit',
    zen_mode: 'Zen Mode',
    toggle_left_panel: 'Toggle Left Panel',
    toggle_right_panel: 'Toggle Right Panel',
    toggle_bottom_panel: 'Toggle Bottom Panel',
    split_vertical: 'Split Pane Vertically',
    split_horizontal: 'Split Pane Horizontally',
    close_pane: 'Close Pane',
    navigate_pane_up: 'Navigate Pane Up',
    navigate_pane_down: 'Navigate Pane Down',
    navigate_pane_left: 'Navigate Pane Left',
    navigate_pane_right: 'Navigate Pane Right',
  };

  const KEYBOARD_CORE_GROUPS = [
    {
      label: 'Tab & Window',
      keys: ['new_tab', 'new_plain_shell_tab', 'close_tab', 'rename_tab', 'new_window', 'quit'],
    },
    {
      label: 'Tools',
      keys: ['manage_tunnels', 'vault_open'],
    },
    {
      label: 'View',
      keys: ['zen_mode', 'toggle_left_panel', 'toggle_right_panel', 'toggle_bottom_panel'],
    },
    {
      label: 'Split Panes',
      keys: [
        'split_vertical',
        'split_horizontal',
        'close_pane',
        'navigate_pane_up',
        'navigate_pane_down',
        'navigate_pane_left',
        'navigate_pane_right',
      ],
    },
  ];

  // Currently recording shortcut state
  let recordingEl = null;
  let recordingRef = null;
  let recordingUnregister = null;

  function ensurePluginShortcutMap() {
    if (!pendingSettings?.conch?.keyboard) return {};
    if (
      !pendingSettings.conch.keyboard.plugin_shortcuts ||
      typeof pendingSettings.conch.keyboard.plugin_shortcuts !== 'object'
    ) {
      pendingSettings.conch.keyboard.plugin_shortcuts = {};
    }
    return pendingSettings.conch.keyboard.plugin_shortcuts;
  }

  function ensureToolWindowShortcutMap() {
    if (!pendingSettings?.conch?.keyboard) return {};
    if (
      !pendingSettings.conch.keyboard.tool_window_shortcuts ||
      typeof pendingSettings.conch.keyboard.tool_window_shortcuts !== 'object'
    ) {
      pendingSettings.conch.keyboard.tool_window_shortcuts = {};
    }
    return pendingSettings.conch.keyboard.tool_window_shortcuts;
  }

  function getShortcutValue(ref) {
    if (!pendingSettings?.conch?.keyboard || !ref) return '';
    if (ref.kind === 'tool-window') {
      const map = ensureToolWindowShortcutMap();
      return map[ref.key] || '';
    }
    if (ref.kind === 'plugin') {
      const map = ensurePluginShortcutMap();
      if (Object.prototype.hasOwnProperty.call(map, ref.key)) return map[ref.key] || '';
      return ref.defaultValue || '';
    }
    return pendingSettings.conch.keyboard[ref.key] || '';
  }

  function setShortcutValue(ref, value) {
    if (!pendingSettings?.conch?.keyboard || !ref) return;
    if (ref.kind === 'tool-window') {
      const map = ensureToolWindowShortcutMap();
      map[ref.key] = value;
      return;
    }
    if (ref.kind === 'plugin') {
      const map = ensurePluginShortcutMap();
      map[ref.key] = value;
      return;
    }
    pendingSettings.conch.keyboard[ref.key] = value;
  }

  function shortcutText(value) {
    const formatted = formatShortcut(value);
    return formatted || 'Unassigned';
  }

  function makeShortcutKeyBox(ref) {
    const keyBox = document.createElement('span');
    keyBox.className = 'settings-shortcut-key';
    keyBox.setAttribute('role', 'button');
    keyBox.tabIndex = 0;
    keyBox.setAttribute('aria-label', 'Record shortcut');
    keyBox.textContent = shortcutText(getShortcutValue(ref));
    keyBox.addEventListener('click', () => startRecording(keyBox, ref));
    keyBox.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      event.preventDefault();
      startRecording(keyBox, ref);
    });
    return keyBox;
  }

  function toTitleCaseWords(s) {
    return String(s || '')
      .split('_')
      .filter(Boolean)
      .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
      .join(' ');
  }

  function stopRecording() {
    if (recordingEl) {
      recordingEl.classList.remove('recording');
      recordingEl.textContent = shortcutText(getShortcutValue(recordingRef));
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

    recordingUnregister = registerGlobalKeyHandler('settings-shortcut-recorder', (e) => {
      e.preventDefault();
      e.stopPropagation();

      // Escape cancels recording
      if (e.key === 'Escape') {
        stopRecording();
        return true;
      }

      const combo = normalizeKeyEvent(e);
      if (!combo) return true; // bare modifier, keep waiting

      setShortcutValue(settingsRef, combo);
      stopRecording();
      return true;
    }, () => !!recordingEl && !!recordingRef);
  }

  function renderKeyboard(c) {
    if (settingsSectionsKeyboard && typeof settingsSectionsKeyboard.renderKeyboard === 'function') {
      const handled = settingsSectionsKeyboard.renderKeyboard(c, {
        stopRecording,
        addSearchInput,
        normalizeSearchText,
        getFuzzyMatchScore,
        getKeyboardSearchQuery: () => keyboardSearchQuery,
        setKeyboardSearchQuery: (value) => {
          keyboardSearchQuery = value;
        },
        renderCurrentSection,
        KEYBOARD_CORE_GROUPS,
        KEYBOARD_CORE_LABELS,
        getPendingKeyboardMap: () => pendingSettings?.conch?.keyboard || {},
        addSectionLabel,
        addRow,
        setRowTarget,
        applyRowSearchHighlight,
        addDivider,
        makeShortcutKeyBox,
        getToolWindowItems: () => (
          window.toolWindowManager && typeof window.toolWindowManager.listWindows === 'function'
            ? window.toolWindowManager.listWindows().slice().sort((a, b) => {
                const typeCmp = String(a.type || '').localeCompare(String(b.type || ''));
                if (typeCmp !== 0) return typeCmp;
                return String(a.title || '').localeCompare(String(b.title || ''));
              })
            : []
        ),
        getPluginMenuItems: () => cachedPluginMenuItems || [],
      });
      if (handled) return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Settings Error', 'Keyboard section module is unavailable.');
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

  // --- Terminal section ---

  function renderTerminal(c) {
    if (!settingsSectionsTerminal || typeof settingsSectionsTerminal.renderTerminal !== 'function') {
      if (window.toast && typeof window.toast.error === 'function') {
        window.toast.error('Settings Error', 'Terminal section module is unavailable.');
      }
      return;
    }
    settingsSectionsTerminal.renderTerminal(c, {
      pendingSettings,
      cachedFonts,
      addSectionLabel,
      addDivider,
      addRow,
      setRowTarget,
      makeInput,
    });
  }

  // --- Shell section ---

  function renderShell(c) {
    if (!settingsSectionsTerminal || typeof settingsSectionsTerminal.renderShell !== "function") {
      if (window.toast && typeof window.toast.error === "function") {
        window.toast.error("Settings Error", "Shell section module is unavailable.");
      }
      return;
    }
    settingsSectionsTerminal.renderShell(c, {
      pendingSettings,
      addSectionLabel,
      addDivider,
      addRow,
      setRowTarget,
      makeInput,
    });
  }

  // --- Cursor section ---

  function renderCursor(c) {
    if (!settingsSectionsTerminal || typeof settingsSectionsTerminal.renderCursor !== "function") {
      if (window.toast && typeof window.toast.error === "function") {
        window.toast.error("Settings Error", "Cursor section module is unavailable.");
      }
      return;
    }
    settingsSectionsTerminal.renderCursor(c, {
      pendingSettings,
      addSectionLabel,
      addDivider,
      addRow,
      setRowTarget,
      makeSwitch,
      makeToggleGroup,
    });
  }

  // --- Plugins section ---

  function renderPlugins(c) {
    if (settingsPluginsSection && typeof settingsPluginsSection.createRenderer === 'function') {
      const renderer = settingsPluginsSection.createRenderer({
        invoke,
        getPendingSettings: () => pendingSettings,
        getCachedPlugins: () => cachedPlugins,
        setCachedPlugins: (next) => { cachedPlugins = Array.isArray(next) ? next : []; },
        setCachedPluginMenuItems: (next) => { cachedPluginMenuItems = Array.isArray(next) ? next : []; },
        refreshPluginInventory: () => refreshPluginInventory(),
        confirmPluginPermissions: (pluginName, permissions) => confirmPluginPermissions(pluginName, permissions),
        invalidateCommandPaletteCache: (reason) => invalidateCommandPaletteCache(reason),
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

  function confirmPluginPermissions(pluginName, permissions) {
    if (window.conchDialogService && typeof window.conchDialogService.confirmPluginPermissions === 'function') {
      return window.conchDialogService.confirmPluginPermissions(pluginName, permissions);
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Plugin Permissions', 'Dialog service unavailable; denying permission request.');
    }
    return Promise.resolve(false);
  }

  // --- Advanced section ---

  function renderAdvanced(c) {
    if (!settingsSectionsBasic || typeof settingsSectionsBasic.renderAdvanced !== "function") {
      if (window.toast && typeof window.toast.error === "function") {
        window.toast.error("Settings Error", "Advanced section module is unavailable.");
      }
      return;
    }
    settingsSectionsBasic.renderAdvanced(c, {
      pendingSettings,
      addSectionLabel,
      addDivider,
      addRow,
      setRowTarget,
      makeSwitch,
      makeInput,
    });
  }

  function renderFiles(c) {
    if (!settingsSectionsBasic || typeof settingsSectionsBasic.renderFiles !== "function") {
      if (window.toast && typeof window.toast.error === "function") {
        window.toast.error("Settings Error", "Files section module is unavailable.");
      }
      return;
    }
    settingsSectionsBasic.renderFiles(c, {
      pendingSettings,
      addSectionLabel,
      addRow,
      setRowTarget,
      makeSwitch,
    });
  }

  async function applySettings() {
    try {
      const result = await invoke('save_settings', { settings: pendingSettings });
      if (standaloneMode && result && result.restart_required) {
        // Emit to the main window so the toast is visible after this window closes.
        try {
          await window.__TAURI__.event.emit('settings-restart-required');
        } catch (_) {}
      }
      close();
      if (!standaloneMode && result && result.restart_required) {
        if (window.toast) window.toast.warn('Restart Required', 'Some changes require a restart to take effect.');
      }
    } catch (e) {
      if (window.toast) window.toast.error('Settings Error', 'Failed to save settings: ' + e);
    }
  }

  exports.settings = { init, open, openInWindow, close };
})(window);
