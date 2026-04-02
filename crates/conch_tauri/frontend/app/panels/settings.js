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

  const SECTION_DEFS = [
    { group: 'Workspace', items: [
      { id: 'appearance', label: 'Appearance', description: 'Theme, notifications, window chrome, UI fonts', keywords: 'theme colors interface notifications window menu bar fonts typography appearance' },
      { id: 'keyboard', label: 'Keymap', description: 'Core shortcuts, tool window shortcuts, plugin shortcuts', keywords: 'keyboard shortcuts keymap bindings hotkeys commands tool windows plugins' },
    ]},
    { group: 'Terminal', items: [
      { id: 'terminal', label: 'Terminal', description: 'Font rendering and scrolling', keywords: 'terminal font size offset scrolling display rendering' },
      { id: 'cursor', label: 'Cursor', description: 'Cursor shape, blinking, vi mode override', keywords: 'cursor block beam underline blinking vi mode caret' },
      { id: 'shell', label: 'Shell', description: 'Shell program, arguments, environment variables', keywords: 'shell program launch arguments env environment variables login command' },
    ]},
    { group: 'Extensions', items: [
      { id: 'plugins', label: 'Plugins', description: 'Plugin system, plugin types, search paths, installed plugins', keywords: 'plugins extensions lua java search paths installed permissions' },
    ]},
    { group: 'System', items: [
      { id: 'advanced', label: 'Advanced', description: 'Startup behavior, window defaults, UI density', keywords: 'advanced startup updates default window size ui density font sizes' },
    ]},
  ];

  const SETTINGS_SEARCH_INDEX = [
    { section: 'appearance', label: 'Theme', keywords: 'color theme appearance scheme', targetId: 'appearance:theme' },
    { section: 'appearance', label: 'Appearance Mode', keywords: 'dark light system mode', targetId: 'appearance:mode' },
    { section: 'appearance', label: 'Notification Position', keywords: 'toast notifications top bottom', targetId: 'appearance:notification-position' },
    { section: 'appearance', label: 'Native Notifications', keywords: 'system notifications os notifications', targetId: 'appearance:native-notifications' },
    { section: 'appearance', label: 'Window Decorations', keywords: 'titlebar transparent buttonless none full window chrome', targetId: 'appearance:window-decorations' },
    { section: 'appearance', label: 'Native Menu Bar', keywords: 'menu bar macos native menu', targetId: 'appearance:native-menu-bar' },
    { section: 'appearance', label: 'UI Font Family', keywords: 'ui font family interface typography', targetId: 'appearance:ui-font-family' },
    { section: 'appearance', label: 'UI Font Size', keywords: 'ui font size interface typography', targetId: 'appearance:ui-font-size' },
    { section: 'keyboard', label: 'Keyboard Shortcuts', keywords: 'keyboard shortcuts keymap bindings' },
    { section: 'keyboard', label: 'Tool Window Shortcuts', keywords: 'tool window keyboard shortcuts sidebars panels' },
    { section: 'keyboard', label: 'Plugin Shortcuts', keywords: 'plugin keyboard shortcuts' },
    { section: 'terminal', label: 'Terminal Font Family', keywords: 'terminal font family monospace', targetId: 'terminal:font-family' },
    { section: 'terminal', label: 'Terminal Font Size', keywords: 'terminal font size', targetId: 'terminal:font-size' },
    { section: 'terminal', label: 'Font Offset X', keywords: 'font offset horizontal x rendering', targetId: 'terminal:font-offset-x' },
    { section: 'terminal', label: 'Font Offset Y', keywords: 'font offset vertical y rendering', targetId: 'terminal:font-offset-y' },
    { section: 'terminal', label: 'Scroll Sensitivity', keywords: 'scrolling trackpad mouse wheel sensitivity', targetId: 'terminal:scroll-sensitivity' },
    { section: 'shell', label: 'Shell Program', keywords: 'shell program executable login shell', targetId: 'shell:program' },
    { section: 'shell', label: 'Arguments', keywords: 'shell arguments flags startup command', targetId: 'shell:args' },
    { section: 'shell', label: 'Environment Variables', keywords: 'env environment variables terminal session', targetId: 'shell:env' },
    { section: 'cursor', label: 'Cursor Shape', keywords: 'cursor shape block beam underline', targetId: 'cursor:shape' },
    { section: 'cursor', label: 'Cursor Blinking', keywords: 'cursor blinking blink', targetId: 'cursor:blinking' },
    { section: 'cursor', label: 'Vi Mode Override', keywords: 'cursor vi mode vim modal', targetId: 'cursor:vi-mode' },
    { section: 'plugins', label: 'Enable Plugins', keywords: 'plugins enable disable master switch', targetId: 'plugins:enabled' },
    { section: 'plugins', label: 'Plugin Types', keywords: 'lua java plugins plugin types', targetId: 'plugins:types' },
    { section: 'plugins', label: 'Extra Search Paths', keywords: 'plugin paths search paths folders directories', targetId: 'plugins:search-paths' },
    { section: 'plugins', label: 'Installed Plugins', keywords: 'installed plugins rescan enable disable permissions', targetId: 'plugins:installed' },
    { section: 'advanced', label: 'Check for Updates', keywords: 'updates startup update checks', targetId: 'advanced:check-for-updates' },
    { section: 'advanced', label: 'Initial Window Size', keywords: 'window size columns lines defaults', targetId: 'advanced:window-size' },
    { section: 'advanced', label: 'UI Chrome Font Sizes', keywords: 'ui chrome font sizes density text size list normal small', targetId: 'advanced:ui-chrome-font-sizes' },
  ];

  function clearSettingsAutofocusTimer() {
    if (settingsSearchAutofocusTimer) {
      clearTimeout(settingsSearchAutofocusTimer);
      settingsSearchAutofocusTimer = null;
    }
  }

  function init(opts) {
    invoke = opts.invoke;
    listenFn = opts.listen;
  }

  function normalizeSearchText(value) {
    return String(value || '').trim().toLowerCase();
  }

  function tokenizeSearchText(value) {
    return normalizeSearchText(value).split(/[\s:_-]+/).filter(Boolean);
  }

  function levenshteinDistance(a, b) {
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
    return String(value || '').replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }

  function appendHighlightedText(container, text, query) {
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
    sidebar.innerHTML = '';
    settingsSidebarResults = [];

    const searchWrap = document.createElement('div');
    searchWrap.className = 'settings-sidebar-search-wrap';
    const searchInput = document.createElement('input');
    searchInput.type = 'search';
    searchInput.className = 'settings-sidebar-search';
    searchInput.placeholder = 'Search settings';
    searchInput.value = settingsSidebarQuery;
    searchInput.addEventListener('input', () => {
      settingsSidebarQuery = searchInput.value;
      settingsSidebarSelectionIndex = -1;
      const active = document.activeElement === searchInput;
      renderSidebarInto(sidebar);
      if (active) {
        const nextInput = sidebar.querySelector('.settings-sidebar-search');
        if (nextInput) {
          nextInput.focus();
          nextInput.setSelectionRange(nextInput.value.length, nextInput.value.length);
        }
      }
    });
    searchInput.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        e.stopPropagation();
        moveSidebarSearchSelection(1);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        e.stopPropagation();
        moveSidebarSearchSelection(-1);
        return;
      }
      if (e.key === 'Enter') {
        if (settingsSidebarResults.length === 0) return;
        e.preventDefault();
        e.stopPropagation();
        const idx = settingsSidebarSelectionIndex >= 0 ? settingsSidebarSelectionIndex : 0;
        const match = settingsSidebarResults[idx];
        if (match) onSidebarSearchResultSelected(match);
      }
    });
    searchWrap.appendChild(searchInput);
    sidebar.appendChild(searchWrap);

    const q = normalizeSearchText(settingsSidebarQuery);
    if (q) {
      const sectionMatches = [];
      for (const group of SECTION_DEFS) {
        for (const item of group.items) {
          const haystack = `${item.label} ${item.description || ''} ${item.keywords || ''}`;
          if (!Number.isFinite(getFuzzyMatchScore(q, haystack, [group.group, item.id]))) continue;
          sectionMatches.push(item);
        }
      }
      const settingMatches = getSidebarSearchResults(q);
      settingsSidebarResults = [...settingMatches];
      for (const item of sectionMatches) {
        settingsSidebarResults.push({
          section: item.id,
          label: item.label,
          path: item.description || item.label,
          kind: 'section',
          targetId: null,
        });
      }
      if (settingsSidebarSelectionIndex >= settingsSidebarResults.length) {
        settingsSidebarSelectionIndex = settingsSidebarResults.length - 1;
      }

      if (sectionMatches.length > 0) {
        const header = document.createElement('div');
        header.className = 'settings-sidebar-group';
        header.textContent = 'Sections';
        sidebar.appendChild(header);
        for (let idx = 0; idx < sectionMatches.length; idx++) {
          const item = sectionMatches[idx];
          const row = document.createElement('div');
          const resultIndex = settingMatches.length + idx;
          row.className = 'settings-sidebar-item settings-sidebar-item-search' + (item.id === currentSection ? ' active' : '') + (settingsSidebarSelectionIndex === resultIndex ? ' selected' : '');
          row.dataset.section = item.id;
          const title = document.createElement('div');
          title.className = 'settings-sidebar-item-title';
          appendHighlightedText(title, item.label, q);
          row.appendChild(title);
          if (item.description) {
            const desc = document.createElement('div');
            desc.className = 'settings-sidebar-item-desc';
            appendHighlightedText(desc, item.description, q);
            row.appendChild(desc);
          }
          row.addEventListener('click', () => onSidebarSearchResultSelected(settingsSidebarResults[resultIndex]));
          sidebar.appendChild(row);
        }
      }

      if (settingMatches.length > 0) {
        const header = document.createElement('div');
        header.className = 'settings-sidebar-group';
        header.textContent = 'Settings';
        sidebar.appendChild(header);
        for (let idx = 0; idx < settingMatches.length; idx++) {
          const match = settingMatches[idx];
          const row = document.createElement('div');
          row.className = 'settings-sidebar-item settings-sidebar-item-search' + (settingsSidebarSelectionIndex === idx ? ' selected' : '');
          row.dataset.section = match.section;
          const title = document.createElement('div');
          title.className = 'settings-sidebar-item-title';
          appendHighlightedText(title, match.label, q);
          row.appendChild(title);
          const desc = document.createElement('div');
          desc.className = 'settings-sidebar-item-desc';
          appendHighlightedText(desc, match.path || match.sectionLabel, q);
          row.appendChild(desc);
          row.addEventListener('click', () => onSidebarSearchResultSelected(match));
          sidebar.appendChild(row);
        }
      }

      if (sectionMatches.length === 0 && settingMatches.length === 0) {
        const empty = document.createElement('div');
        empty.className = 'settings-sidebar-empty';
        empty.textContent = 'No settings match your search.';
        sidebar.appendChild(empty);
      }
      return;
    }

    settingsSidebarResults = [];
    settingsSidebarSelectionIndex = -1;
    for (const group of SECTION_DEFS) {
      const groupEl = document.createElement('div');
      groupEl.className = 'settings-sidebar-group';
      groupEl.textContent = group.group;
      sidebar.appendChild(groupEl);
      for (const item of group.items) {
        const itemEl = document.createElement('div');
        itemEl.className = 'settings-sidebar-item' + (item.id === currentSection ? ' active' : '');
        itemEl.dataset.section = item.id;
        const title = document.createElement('div');
        title.className = 'settings-sidebar-item-title';
        title.textContent = item.label;
        itemEl.appendChild(title);
        if (item.description) {
          const desc = document.createElement('div');
          desc.className = 'settings-sidebar-item-desc';
          desc.textContent = item.description;
          itemEl.appendChild(desc);
        }
        itemEl.addEventListener('click', () => selectSection(item.id));
        sidebar.appendChild(itemEl);
      }
    }
  }

  async function open() {
    if (document.getElementById('settings-overlay')) { close(); return; }

    try {
      const [settings, themes, plugins, pluginMenuItems, fonts] = await Promise.all([
        invoke('get_all_settings'),
        invoke('list_themes'),
        invoke('scan_plugins'),
        invoke('get_plugin_menu_items').catch(() => []),
        invoke('list_system_fonts'),
      ]);
      originalSettings = JSON.parse(JSON.stringify(settings));
      pendingSettings = JSON.parse(JSON.stringify(settings));
      cachedThemes = themes;
      cachedPlugins = plugins;
      cachedPluginMenuItems = Array.isArray(pluginMenuItems) ? pluginMenuItems : [];
      cachedFonts = fonts;
      settingsSidebarQuery = '';
      keyboardSearchQuery = '';
      currentSection = 'appearance';
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
      const [settings, themes, plugins, pluginMenuItems, fonts] = await Promise.all([
        invoke('get_all_settings'),
        invoke('list_themes'),
        invoke('scan_plugins'),
        invoke('get_plugin_menu_items').catch(() => []),
        invoke('list_system_fonts'),
      ]);
      originalSettings = JSON.parse(JSON.stringify(settings));
      pendingSettings = JSON.parse(JSON.stringify(settings));
      cachedThemes = themes;
      cachedPlugins = plugins;
      cachedPluginMenuItems = Array.isArray(pluginMenuItems) ? pluginMenuItems : [];
      cachedFonts = fonts;
      settingsSidebarQuery = '';
      keyboardSearchQuery = '';
      currentSection = 'appearance';
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

    // Escape to close the settings window.
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') {
        if (recordingEl) return;  // let recording handler handle it
        close();
      }
    });

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

  function escHtml(value) {
    return String(value ?? '')
      .replaceAll('&', '&amp;')
      .replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;')
      .replaceAll('"', '&quot;')
      .replaceAll("'", '&#39;');
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

    addSectionLabel(c, 'Theme & Color');

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
    setRowTarget(addRow(c, 'Theme', 'Color theme for the terminal and UI', themeSelect), 'appearance:theme');

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
    setRowTarget(addRow(c, 'Appearance Mode', null, toggleGroup), 'appearance:mode');

    addDivider(c);

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
    setRowTarget(addRow(c, 'Notification Position', 'Where toast notifications appear on screen', posGroup), 'appearance:notification-position');

    // Native notifications toggle
    const nativeSwitch = makeSwitch(
      pendingSettings.conch.ui.native_notifications !== false,
      (val) => { pendingSettings.conch.ui.native_notifications = val; }
    );
    setRowTarget(addRow(c, 'Native Notifications', 'Use system notifications when the app is not focused', nativeSwitch), 'appearance:native-notifications');

    addDivider(c);

    addSectionLabel(c, 'Window Chrome');

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
    setRowTarget(addRow(c, 'Window Decorations', 'Window title bar style', decoSelect), 'appearance:window-decorations');

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
      setRowTarget(addRow(c, 'Native Menu Bar', 'Use the system menu bar instead of in-app menu', sw), 'appearance:native-menu-bar');
    }

    addDivider(c);

    addSectionLabel(c, 'Interface Typography');

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
    setRowTarget(addRow(c, 'UI Font Family', null, fontSelect), 'appearance:ui-font-family');

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
    setRowTarget(addRow(c, 'UI Font Size', null, sizeInput), 'appearance:ui-font-size');
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
    close_tab: 'Close Tab',
    rename_tab: 'Rename Tab',
    new_window: 'New Window',
    manage_tunnels: 'Manage SSH Tunnels',
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
      keys: ['new_tab', 'close_tab', 'rename_tab', 'new_window', 'quit'],
    },
    {
      label: 'Tools',
      keys: ['manage_tunnels'],
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
  let recordingHandler = null;

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
    keyBox.textContent = shortcutText(getShortcutValue(ref));
    keyBox.addEventListener('click', () => startRecording(keyBox, ref));
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
    if (recordingHandler) {
      document.removeEventListener('keydown', recordingHandler, true);
      recordingHandler = null;
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

      setShortcutValue(settingsRef, combo);
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

    addSearchInput(c, 'Search shortcuts', keyboardSearchQuery, (value) => {
      keyboardSearchQuery = value;
      renderCurrentSection();
    });

    const query = normalizeSearchText(keyboardSearchQuery);
    const matchesShortcut = (label, desc, extra) => {
      if (!query) return true;
      return Number.isFinite(getFuzzyMatchScore(query, `${label} ${desc || ''} ${extra || ''}`));
    };
    let totalRendered = 0;

    const knownKeys = new Set();
    for (let gi = 0; gi < KEYBOARD_CORE_GROUPS.length; gi++) {
      const group = KEYBOARD_CORE_GROUPS[gi];
      const rows = [];

      for (const key of group.keys) {
        knownKeys.add(key);
        const label = KEYBOARD_CORE_LABELS[key] || toTitleCaseWords(key);
        if (!matchesShortcut(label, group.label, key)) continue;
        rows.push({ label, key });
      }

      if (rows.length === 0) continue;
      addSectionLabel(c, group.label);
      for (const row of rows) {
        const rowEl = addRow(c, row.label, null, makeShortcutKeyBox({ kind: 'core', key: row.key }));
        setRowTarget(rowEl, `keyboard:core:${row.key}`);
        applyRowSearchHighlight(rowEl, row.label, null, query);
        totalRendered++;
      }
      addDivider(c);
    }

    const keyboard = pendingSettings?.conch?.keyboard || {};
    const extraKeys = Object.keys(keyboard)
      .filter((k) => k !== 'plugin_shortcuts' && k !== 'tool_window_shortcuts' && typeof keyboard[k] === 'string' && !knownKeys.has(k))
      .sort();
    if (extraKeys.length > 0) {
      const rows = [];
      for (const key of extraKeys) {
        const label = toTitleCaseWords(key);
        if (!matchesShortcut(label, 'Other', key)) continue;
        rows.push({ label, key });
      }
      if (rows.length > 0) {
        addSectionLabel(c, 'Other');
        for (const row of rows) {
          const rowEl = addRow(c, row.label, null, makeShortcutKeyBox({ kind: 'core', key: row.key }));
          setRowTarget(rowEl, `keyboard:core:${row.key}`);
          applyRowSearchHighlight(rowEl, row.label, null, query);
          totalRendered++;
        }
        addDivider(c);
      }
    }

    const toolWindowItems = window.toolWindowManager && typeof window.toolWindowManager.listWindows === 'function'
      ? window.toolWindowManager.listWindows().slice().sort((a, b) => {
          const typeCmp = String(a.type || '').localeCompare(String(b.type || ''));
          if (typeCmp !== 0) return typeCmp;
          return String(a.title || '').localeCompare(String(b.title || ''));
        })
      : [];
    if (toolWindowItems.length > 0) {
      const rows = [];
      for (const item of toolWindowItems) {
        const side = String(item.zone || '').replace('-', ' \u2022 ');
        const desc = item.type === 'built-in'
          ? `Built-in \u2022 ${side}`
          : `Plugin tool window \u2022 ${side}`;
        if (!matchesShortcut(item.title || item.id, desc, item.id)) continue;
        rows.push({ label: item.title || item.id, desc, id: item.id });
      }
      if (rows.length > 0) {
        addSectionLabel(c, 'Tool Windows');
        for (const row of rows) {
          const rowEl = addRow(c, row.label, row.desc, makeShortcutKeyBox({ kind: 'tool-window', key: row.id }));
          setRowTarget(rowEl, `keyboard:tool-window:${row.id}`);
          applyRowSearchHighlight(rowEl, row.label, row.desc, query);
          totalRendered++;
        }
        addDivider(c);
      }
    }

    const byPluginAction = new Map();
    for (const item of cachedPluginMenuItems || []) {
      if (!item || !item.plugin || !item.action) continue;
      const uniqueKey = `${item.plugin}:${item.action}`;
      if (byPluginAction.has(uniqueKey)) continue;
      byPluginAction.set(uniqueKey, item);
    }
    const pluginItems = Array.from(byPluginAction.values()).sort((a, b) => {
      const pluginCmp = String(a.plugin || '').localeCompare(String(b.plugin || ''));
      if (pluginCmp !== 0) return pluginCmp;
      return String(a.label || '').localeCompare(String(b.label || ''));
    });
    if (pluginItems.length > 0) {
      const rows = [];
      for (const item of pluginItems) {
        const pluginKey = `${item.plugin}:${item.action}`;
        const desc = item.menu ? `${item.plugin} \u2022 ${item.menu}` : item.plugin;
        if (!matchesShortcut(item.label || toTitleCaseWords(item.action), desc, pluginKey)) continue;
        rows.push({
          label: item.label || toTitleCaseWords(item.action),
          desc,
          key: pluginKey,
          defaultValue: item.keybind || '',
        });
      }
      if (rows.length > 0) {
        addSectionLabel(c, 'Plugin Shortcuts');
        for (const row of rows) {
          const rowEl = addRow(c, row.label, row.desc, makeShortcutKeyBox({ kind: 'plugin', key: row.key, defaultValue: row.defaultValue }));
          setRowTarget(rowEl, `keyboard:plugin:${row.key}`);
          applyRowSearchHighlight(rowEl, row.label, row.desc, query);
          totalRendered++;
        }
      }
    }
    if (query && totalRendered === 0) {
      const empty = document.createElement('div');
      empty.className = 'settings-search-empty';
      empty.textContent = 'No shortcuts match your search.';
      c.appendChild(empty);
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

    addSectionLabel(c, 'Typography');

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
    setRowTarget(addRow(c, 'Terminal Font Family', null, fontFamilySelect), 'terminal:font-family');

    const fontSizeInput = makeInput('number', pendingSettings.terminal.font.size);
    fontSizeInput.addEventListener('input', () => {
      const v = parseFloat(fontSizeInput.value);
      if (!isNaN(v)) pendingSettings.terminal.font.size = v;
    });
    setRowTarget(addRow(c, 'Terminal Font Size', null, fontSizeInput), 'terminal:font-size');

    const offsetXInput = makeInput('number', pendingSettings.terminal.font.offset.x, { step: '0.5' });
    offsetXInput.addEventListener('input', () => {
      const v = parseFloat(offsetXInput.value);
      if (!isNaN(v)) pendingSettings.terminal.font.offset.x = v;
    });
    setRowTarget(addRow(c, 'Font Offset X', null, offsetXInput), 'terminal:font-offset-x');

    const offsetYInput = makeInput('number', pendingSettings.terminal.font.offset.y, { step: '0.5' });
    offsetYInput.addEventListener('input', () => {
      const v = parseFloat(offsetYInput.value);
      if (!isNaN(v)) pendingSettings.terminal.font.offset.y = v;
    });
    setRowTarget(addRow(c, 'Font Offset Y', null, offsetYInput), 'terminal:font-offset-y');

    addDivider(c);

    addSectionLabel(c, 'Scrolling');

    const scrollInput = makeInput('number', pendingSettings.terminal.scroll_sensitivity, {
      step: '0.05', min: 0, max: 1,
    });
    scrollInput.addEventListener('input', () => {
      const v = parseFloat(scrollInput.value);
      if (!isNaN(v)) pendingSettings.terminal.scroll_sensitivity = v;
    });
    setRowTarget(addRow(c, 'Scroll Sensitivity', '0.0 to 1.0 (tuned for macOS trackpads)', scrollInput), 'terminal:scroll-sensitivity');
  }

  // --- Shell section ---

  function renderShell(c) {
    const h = document.createElement('h3');
    h.textContent = 'Shell & Environment';
    c.appendChild(h);

    addSectionLabel(c, 'Launch');

    const shellInput = makeInput('text', pendingSettings.terminal.shell.program, {
      placeholder: 'Uses $SHELL login shell',
    });
    shellInput.addEventListener('input', () => {
      pendingSettings.terminal.shell.program = shellInput.value;
    });
    setRowTarget(addRow(c, 'Shell Program', null, shellInput), 'shell:program');

    const argsInput = makeInput('text', (pendingSettings.terminal.shell.args || []).join(', '));
    argsInput.addEventListener('input', () => {
      pendingSettings.terminal.shell.args = argsInput.value
        .split(',')
        .map(s => s.trim())
        .filter(s => s.length > 0);
    });
    setRowTarget(addRow(c, 'Arguments', 'Comma-separated (e.g. -l, -c, echo ok)', argsInput), 'shell:args');

    addDivider(c);

    addSectionLabel(c, 'Environment Variables');

    const envContainer = document.createElement('div');
    envContainer.dataset.settingId = 'shell:env';
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

    addSectionLabel(c, 'Primary Cursor');

    const shapeToggle = makeToggleGroup(
      [
        { label: 'Block', value: 'Block' },
        { label: 'Underline', value: 'Underline' },
        { label: 'Beam', value: 'Beam' },
      ],
      pendingSettings.terminal.cursor.style.shape,
      (val) => { pendingSettings.terminal.cursor.style.shape = val; }
    );
    setRowTarget(addRow(c, 'Cursor Shape', null, shapeToggle), 'cursor:shape');

    const blinkSwitch = makeSwitch(
      pendingSettings.terminal.cursor.style.blinking,
      (val) => { pendingSettings.terminal.cursor.style.blinking = val; }
    );
    setRowTarget(addRow(c, 'Cursor Blinking', null, blinkSwitch), 'cursor:blinking');

    addDivider(c);

    addSectionLabel(c, 'Vi Mode Override');

    const viNote = document.createElement('div');
    viNote.dataset.settingId = 'cursor:vi-mode';
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
    setRowTarget(addRow(c, 'Vi Mode Override', null, viShapeToggle), 'cursor:vi-mode');

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

    addSectionLabel(c, 'Plugin System');

    const enablePluginsSwitch = makeSwitch(
      pendingSettings.conch.plugins.enabled,
      (val) => { pendingSettings.conch.plugins.enabled = val; }
    );
    setRowTarget(addRow(c, 'Enable Plugins', 'Master switch \u2014 disable to run as pure terminal', enablePluginsSwitch), 'plugins:enabled');

    addDivider(c);

    addSectionLabel(c, 'Plugin Types');
    const pluginTypesAnchor = document.createElement('div');
    pluginTypesAnchor.dataset.settingId = 'plugins:types';
    c.appendChild(pluginTypesAnchor);

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

    addSectionLabel(c, 'Extra Search Paths');
    const searchPathsHint = document.createElement('div');
    searchPathsHint.dataset.settingId = 'plugins:search-paths';
    searchPathsHint.className = 'settings-row-desc';
    searchPathsHint.style.marginBottom = '8px';
    searchPathsHint.textContent = 'Built-in defaults always include ~/.config/conch/plugins. Add extra directories here.';
    c.appendChild(searchPathsHint);

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
    installedHeader.dataset.settingId = 'plugins:installed';
    installedHeader.style.cssText = 'display:flex; justify-content:space-between; align-items:center; padding-right:10px;';
    const installedLabel = document.createElement('div');
    installedLabel.className = 'settings-section-label';
    installedLabel.textContent = 'Installed Plugins';
    installedHeader.appendChild(installedLabel);

    const rescanLabel = document.createElement('span');
    rescanLabel.textContent = 'Rescan';
    rescanLabel.setAttribute('role', 'button');
    rescanLabel.setAttribute('tabindex', '0');
    rescanLabel.style.cssText = 'font-size:12px; color:var(--blue, #7aa2f7); cursor:pointer; user-select:none;';
    const handleRescan = async () => {
      rescanLabel.style.pointerEvents = 'none';
      rescanLabel.style.opacity = '0.6';
      try {
        cachedPlugins = await invoke('scan_plugins');
        cachedPluginMenuItems = await invoke('get_plugin_menu_items').catch(() => []);
        if (window.titlebar && typeof window.titlebar.refresh === 'function') {
          window.titlebar.refresh().catch(() => {});
        }
      } catch (e) {
        if (window.toast) window.toast.error('Plugin Scan Failed', String(e));
      }
      rescanLabel.style.pointerEvents = 'auto';
      rescanLabel.style.opacity = '1';
      renderPluginList();
    };
    rescanLabel.addEventListener('click', handleRescan);
    rescanLabel.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        handleRescan();
      }
    });
    installedHeader.appendChild(rescanLabel);
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
        left.style.cssText = 'display:flex; align-items:center; gap:8px; min-width:0; flex:1; overflow:hidden;';

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

        // Enable/disable checkbox
        const toggle = document.createElement('input');
        toggle.type = 'checkbox';
        toggle.checked = !!plugin.loaded;
        toggle.style.cssText = 'flex-shrink:0; margin-left:8px;';
        toggle.setAttribute('aria-label', `${plugin.loaded ? 'Disable' : 'Enable'} ${plugin.name}`);
        toggle.addEventListener('change', async () => {
          const nextLoaded = toggle.checked;
          toggle.disabled = true;
          try {
            if (!nextLoaded) {
              await invoke('disable_plugin', { name: plugin.name, source: plugin.source });
              await invoke('rebuild_menu').catch(() => {});
              if (window.toast) window.toast.info('Plugin Disabled', plugin.name);
            } else {
              const perms = Array.isArray(plugin.permissions) ? plugin.permissions.filter(Boolean) : [];
              if (perms.length > 0) {
                const accepted = await confirmPluginPermissions(plugin.name, perms);
                if (!accepted) {
                  toggle.checked = false;
                  return;
                }
              }
              await invoke('enable_plugin', { name: plugin.name, source: plugin.source, path: plugin.path });
              await invoke('rebuild_menu').catch(() => {});
              if (window.toast) window.toast.success('Plugin Enabled', plugin.name);
            }
            cachedPlugins = await invoke('scan_plugins');
            cachedPluginMenuItems = await invoke('get_plugin_menu_items').catch(() => []);
            if (window.titlebar && typeof window.titlebar.refresh === 'function') {
              window.titlebar.refresh().catch(() => {});
            }
          } catch (e) {
            toggle.checked = !!plugin.loaded;
            if (window.toast) window.toast.error('Plugin Action Failed', String(e));
          }
          toggle.disabled = false;
          renderPluginList();
        });
        row.appendChild(toggle);

        pluginListContainer.appendChild(row);
      }
    }

    renderPluginList();
  }

  function confirmPluginPermissions(pluginName, permissions) {
    return new Promise((resolve) => {
      const overlay = document.createElement('div');
      overlay.className = 'ssh-overlay';

      const items = permissions
        .map((p) => `<div style="font-size:12px; color:var(--fg); line-height:1.5;">• ${escHtml(p)}</div>`)
        .join('');

      overlay.innerHTML = `
        <div class="ssh-form" style="min-width:420px; max-width:620px;">
          <div class="ssh-form-title">Plugin Permissions</div>
          <div class="ssh-form-body">
            <div style="margin-bottom:10px; font-size:12px; color:var(--fg);">
              Plugin "${escHtml(pluginName)}" requests:
            </div>
            <div style="display:flex; flex-direction:column; gap:4px; margin-bottom:12px;">
              ${items}
            </div>
            <div style="font-size:12px; color:var(--dim-fg);">
              Allow and enable this plugin?
            </div>
          </div>
          <div class="ssh-form-buttons">
            <button class="ssh-form-btn" id="pp-deny">Deny</button>
            <button class="ssh-form-btn primary" id="pp-allow">Allow</button>
          </div>
        </div>`;

      const finish = (accepted) => {
        document.removeEventListener('keydown', onKey, true);
        overlay.remove();
        resolve(accepted);
      };

      const onKey = (e) => {
        if (e.key !== 'Escape') return;
        e.preventDefault();
        e.stopPropagation();
        finish(false);
      };

      overlay.addEventListener('mousedown', (e) => {
        if (e.target === overlay) finish(false);
      });
      overlay.querySelector('#pp-deny').addEventListener('click', () => finish(false));
      overlay.querySelector('#pp-allow').addEventListener('click', () => finish(true));
      document.addEventListener('keydown', onKey, true);
      document.body.appendChild(overlay);
    });
  }

  // --- Advanced section ---

  function renderAdvanced(c) {
    const h = document.createElement('h3');
    h.textContent = 'Advanced';
    c.appendChild(h);

    addSectionLabel(c, 'Startup & Updates');

    const updateSwitch = makeSwitch(
      pendingSettings.conch.check_for_updates !== false,
      (val) => { pendingSettings.conch.check_for_updates = val; }
    );
    setRowTarget(addRow(c, 'Check for Updates', 'Automatically check for new versions when the app starts (macOS and Windows)', updateSwitch), 'advanced:check-for-updates');

    addDivider(c);

    addSectionLabel(c, 'Window Defaults');
    const windowDefaultsAnchor = document.createElement('div');
    windowDefaultsAnchor.dataset.settingId = 'advanced:window-size';
    c.appendChild(windowDefaultsAnchor);

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

    addSectionLabel(c, 'Interface Density');
    const densityAnchor = document.createElement('div');
    densityAnchor.dataset.settingId = 'advanced:ui-chrome-font-sizes';
    c.appendChild(densityAnchor);

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
