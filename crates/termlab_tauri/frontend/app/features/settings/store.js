// Settings feature — dialog state ownership (pending/original settings, cached
// runtime data, section defs, search index, shortcut maps, pending jump state).

(function initTermLabSettingsStore(global) {
  'use strict';

  const KEYBOARD_CORE_LABELS = {
    new_tab: 'New Tab',
    new_plain_shell_tab: 'New Plain Shell Tab',
    close_tab: 'Close Tab',
    select_tab_left: 'Select Tab to the Left',
    select_tab_right: 'Select Tab to the Right',
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
    new_file: 'New File',
    save_file: 'Save File',
    save_file_as: 'Save File As',
    open_file: 'Open File',
  };

  const KEYBOARD_CORE_GROUPS = [
    {
      label: 'Tab & Window',
      keys: [
        'new_tab', 'new_plain_shell_tab', 'close_tab', 'select_tab_left',
        'select_tab_right', 'rename_tab', 'new_window', 'quit',
      ],
    },
    {
      label: 'Editor',
      keys: ['new_file', 'open_file', 'save_file', 'save_file_as'],
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

  function toTitleCaseWords(s) {
    return String(s || '')
      .split('_')
      .filter(Boolean)
      .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
      .join(' ');
  }

  function create() {
    const constants = global.termlabSettingsFeatureConstants || {};
    const searchFeature = global.termlabSettingsFeatureSearch || {};
    const SECTION_DEFS = Array.isArray(constants.SECTION_DEFS) ? constants.SECTION_DEFS : [];
    const SETTINGS_SEARCH_INDEX = Array.isArray(constants.SETTINGS_SEARCH_INDEX)
      ? constants.SETTINGS_SEARCH_INDEX
      : [];

    let currentSection = 'appearance';
    let pendingSettings = null;
    let originalSettings = null;
    let cachedTerminalThemes = [];
    let cachedPlugins = [];
    let cachedPluginMenuItems = [];
    let cachedPluginSettingsSections = [];
    let cachedFonts = { all: [], monospace: [] };
    let settingsSidebarQuery = '';
    let keyboardSearchQuery = '';
    let settingsSidebarResults = [];
    let settingsSidebarSelectionIndex = -1;
    let pendingSettingsJump = null;
    let skipPluginDraftDiscardOnClose = false;
    // Sidebar group-header disclosure state (Task 1 "Sidebar per the
    // measurement" — the reference's tree has collapsible groups). Persists
    // across renderSidebarInto() re-renders (every keystroke/selection
    // rebuilds the sidebar's innerHTML from scratch) but not across
    // open()/openInWindow() — applyLoadedSettingsData() below resets it, same
    // lifetime as settingsSidebarQuery. Default: expanded (matches the prior
    // always-expanded behavior).
    let collapsedSidebarGroups = new Set();

    function ensureSettingsShape(settings) {
      if (!settings.termlab) settings.termlab = {};
      if (!settings.termlab.ui || typeof settings.termlab.ui !== 'object') {
        settings.termlab.ui = {};
      }
      if (typeof settings.termlab.ui.disable_animations !== 'boolean') {
        settings.termlab.ui.disable_animations = false;
      }
      if (!settings.termlab.files || typeof settings.termlab.files !== 'object') {
        settings.termlab.files = {};
      }
      if (typeof settings.termlab.files.follow_path !== 'boolean') {
        settings.termlab.files.follow_path = true;
      }
    }

    function applyLoadedSettingsData(payload) {
      const loaded = payload || {};
      originalSettings = JSON.parse(JSON.stringify(loaded.settings || {}));
      pendingSettings = JSON.parse(JSON.stringify(loaded.settings || {}));
      ensureSettingsShape(originalSettings);
      ensureSettingsShape(pendingSettings);
      cachedTerminalThemes = Array.isArray(loaded.terminalThemes) ? loaded.terminalThemes : [];
      cachedPlugins = Array.isArray(loaded.plugins) ? loaded.plugins : [];
      cachedPluginMenuItems = Array.isArray(loaded.pluginMenuItems) ? loaded.pluginMenuItems : [];
      cachedPluginSettingsSections = Array.isArray(loaded.pluginSettingsSections) ? loaded.pluginSettingsSections : [];
      cachedFonts = loaded.fonts && typeof loaded.fonts === 'object' ? loaded.fonts : { all: [], monospace: [] };
      settingsSidebarQuery = '';
      keyboardSearchQuery = '';
      currentSection = 'appearance';
      collapsedSidebarGroups = new Set();
    }

    function clearLoadedSettings() {
      pendingSettings = null;
      originalSettings = null;
    }

    // Dirty tracking for the footer's Apply button (Task 1 brief: "disabled
    // until there are pending changes"). pendingSettings/originalSettings are
    // the store's existing edit-vs-baseline pair (applyLoadedSettingsData
    // above clones the loaded settings into both; every section renderer
    // mutates pendingSettings in place) — isDirty() is just a structural
    // comparison of that existing pair, not a new source of truth. Does NOT
    // cover plugin-settings drafts, which live server-side behind their own
    // commit/discard invokes (see actions.js) rather than in pendingSettings.
    function isDirty() {
      if (!pendingSettings || !originalSettings) return false;
      try {
        return JSON.stringify(pendingSettings) !== JSON.stringify(originalSettings);
      } catch (_) {
        return false;
      }
    }

    // Called after a successful non-closing Apply so the just-saved state
    // becomes the new baseline (isDirty() goes false again until the next
    // edit), mirroring applyLoadedSettingsData's clone-into-both pattern.
    function commitPendingAsOriginal() {
      if (!pendingSettings) return;
      originalSettings = JSON.parse(JSON.stringify(pendingSettings));
    }

    function isSidebarGroupCollapsed(groupLabel) {
      return collapsedSidebarGroups.has(groupLabel);
    }

    function toggleSidebarGroupCollapsed(groupLabel) {
      if (!groupLabel) return;
      if (collapsedSidebarGroups.has(groupLabel)) collapsedSidebarGroups.delete(groupLabel);
      else collapsedSidebarGroups.add(groupLabel);
    }

    function getPluginSettingsSections() {
      return Array.isArray(cachedPluginSettingsSections)
        ? cachedPluginSettingsSections.filter((section) => section && section.section_key && section.label)
        : [];
    }

    function getPluginSettingsSectionByKey(sectionKey) {
      if (!sectionKey) return null;
      const sections = getPluginSettingsSections();
      for (const section of sections) {
        if (section.section_key === sectionKey) return section;
      }
      return null;
    }

    function isPluginSettingsSectionId(sectionId) {
      return !!getPluginSettingsSectionByKey(sectionId);
    }

    function getSectionDefs() {
      const pluginSections = getPluginSettingsSections();
      if (pluginSections.length === 0) {
        return SECTION_DEFS;
      }

      const defs = SECTION_DEFS.map((group) => ({
        group: group.group,
        items: Array.isArray(group.items) ? group.items.slice() : [],
      }));

      let extensionsGroup = defs.find((group) => group.group === 'Extensions');
      if (!extensionsGroup) {
        extensionsGroup = { group: 'Extensions', items: [] };
        defs.push(extensionsGroup);
      }

      for (const section of pluginSections) {
        extensionsGroup.items.push({
          id: section.section_key,
          label: section.label,
          description: section.description || `Plugin settings for ${section.plugin_name}`,
          keywords: `plugin ${section.plugin_name} ${section.keywords || ''}`.trim(),
        });
      }

      return defs;
    }

    function getSectionById(id) {
      for (const group of getSectionDefs()) {
        for (const item of group.items) {
          if (item.id === id) return item;
        }
      }
      return null;
    }

    function buildSettingsSearchIndex() {
      const entries = SETTINGS_SEARCH_INDEX.map((entry) => ({ ...entry }));

      for (const section of getPluginSettingsSections()) {
        const sectionPath = `${section.group || 'Extensions'} > ${section.label}`;
        entries.push({
          section: section.section_key,
          label: section.label,
          keywords: `plugin settings ${section.plugin_name} ${section.keywords || ''}`.trim(),
          path: sectionPath,
          kind: 'plugin-section',
          targetId: `plugin-section:${section.section_key}`,
        });

        const settings = Array.isArray(section.settings) ? section.settings : [];
        for (const setting of settings) {
          if (!setting || !setting.label) continue;
          entries.push({
            section: section.section_key,
            label: setting.label,
            keywords: `plugin setting ${section.plugin_name} ${setting.keywords || ''} ${setting.description || ''}`.trim(),
            path: sectionPath,
            kind: 'plugin-setting',
            targetId: `plugin-setting:${section.section_key}:${setting.id || ''}`,
          });
        }
      }

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

      const toolWindowItems = global.toolWindowManager && typeof global.toolWindowManager.listWindows === 'function'
        ? global.toolWindowManager.listWindows()
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
      const q = searchFeature.normalizeSearchText(query);
      if (!q) return [];

      const results = [];
      const seen = new Set();
      for (const entry of buildSettingsSearchIndex()) {
        const haystack = `${entry.label} ${entry.keywords || ''}`;
        const score = searchFeature.getFuzzyMatchScore(q, haystack, [entry.path, entry.section, entry.kind, entry.targetKey, entry.targetId]);
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

    function registerPendingSettingsJump(match) {
      pendingSettingsJump = match ? {
        section: match.section,
        label: match.label || '',
        targetId: match.targetId || null,
        query: settingsSidebarQuery || '',
      } : null;
    }

    function ensurePluginShortcutMap() {
      if (!pendingSettings?.termlab?.keyboard) return {};
      if (
        !pendingSettings.termlab.keyboard.plugin_shortcuts ||
        typeof pendingSettings.termlab.keyboard.plugin_shortcuts !== 'object'
      ) {
        pendingSettings.termlab.keyboard.plugin_shortcuts = {};
      }
      return pendingSettings.termlab.keyboard.plugin_shortcuts;
    }

    function ensureToolWindowShortcutMap() {
      if (!pendingSettings?.termlab?.keyboard) return {};
      if (
        !pendingSettings.termlab.keyboard.tool_window_shortcuts ||
        typeof pendingSettings.termlab.keyboard.tool_window_shortcuts !== 'object'
      ) {
        pendingSettings.termlab.keyboard.tool_window_shortcuts = {};
      }
      return pendingSettings.termlab.keyboard.tool_window_shortcuts;
    }

    function getShortcutValue(ref) {
      if (!pendingSettings?.termlab?.keyboard || !ref) return '';
      if (ref.kind === 'tool-window') {
        const map = ensureToolWindowShortcutMap();
        return map[ref.key] || '';
      }
      if (ref.kind === 'plugin') {
        const map = ensurePluginShortcutMap();
        if (Object.prototype.hasOwnProperty.call(map, ref.key)) return map[ref.key] || '';
        return ref.defaultValue || '';
      }
      return pendingSettings.termlab.keyboard[ref.key] || '';
    }

    function setShortcutValue(ref, value) {
      if (!pendingSettings?.termlab?.keyboard || !ref) return;
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
      pendingSettings.termlab.keyboard[ref.key] = value;
    }

    return {
      KEYBOARD_CORE_LABELS,
      KEYBOARD_CORE_GROUPS,

      getCurrentSection: () => currentSection,
      setCurrentSection: (id) => { currentSection = id; },

      getPendingSettings: () => pendingSettings,
      getOriginalSettings: () => originalSettings,
      getPendingKeyboardMap: () => pendingSettings?.termlab?.keyboard || {},

      getCachedTerminalThemes: () => cachedTerminalThemes,
      getCachedFonts: () => cachedFonts,
      getCachedPlugins: () => cachedPlugins,
      setCachedPlugins: (next) => { cachedPlugins = Array.isArray(next) ? next : []; },
      getCachedPluginMenuItems: () => cachedPluginMenuItems,
      setCachedPluginMenuItems: (next) => { cachedPluginMenuItems = Array.isArray(next) ? next : []; },
      getCachedPluginSettingsSections: () => cachedPluginSettingsSections,
      setCachedPluginSettingsSections: (next) => { cachedPluginSettingsSections = Array.isArray(next) ? next : []; },

      getSidebarQuery: () => settingsSidebarQuery,
      setSidebarQuery: (value) => { settingsSidebarQuery = value; },
      getKeyboardSearchQuery: () => keyboardSearchQuery,
      setKeyboardSearchQuery: (value) => { keyboardSearchQuery = value; },
      getSidebarResults: () => settingsSidebarResults,
      setSidebarResults: (results) => { settingsSidebarResults = Array.isArray(results) ? results : []; },
      getSidebarSelectionIndex: () => settingsSidebarSelectionIndex,
      setSidebarSelectionIndex: (value) => { settingsSidebarSelectionIndex = value; },

      getPendingSettingsJump: () => pendingSettingsJump,
      clearPendingSettingsJump: () => { pendingSettingsJump = null; },
      registerPendingSettingsJump,

      getSkipPluginDraftDiscardOnClose: () => skipPluginDraftDiscardOnClose,
      setSkipPluginDraftDiscardOnClose: (value) => { skipPluginDraftDiscardOnClose = !!value; },

      isDirty,
      commitPendingAsOriginal,
      isSidebarGroupCollapsed,
      toggleSidebarGroupCollapsed,

      applyLoadedSettingsData,
      clearLoadedSettings,
      getPluginSettingsSections,
      getPluginSettingsSectionByKey,
      isPluginSettingsSectionId,
      getSectionDefs,
      getSectionById,
      buildSettingsSearchIndex,
      getSidebarSearchResults,
      getShortcutValue,
      setShortcutValue,
    };
  }

  global.termlabSettingsStore = { create };
})(window);
