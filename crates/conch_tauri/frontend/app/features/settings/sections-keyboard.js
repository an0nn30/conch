(function initConchSettingsSectionsKeyboard(global) {
  'use strict';

  function toTitleCaseWords(value) {
    return String(value || '')
      .split('_')
      .filter(Boolean)
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
      .join(' ');
  }

  function renderKeyboard(container, deps) {
    if (!container) return false;
    const d = deps || {};

    if (typeof d.stopRecording === 'function') d.stopRecording();

    const addSearchInput = typeof d.addSearchInput === 'function' ? d.addSearchInput : null;
    const normalizeSearchText = typeof d.normalizeSearchText === 'function'
      ? d.normalizeSearchText
      : (input) => String(input || '').trim().toLowerCase();
    const getFuzzyMatchScore = typeof d.getFuzzyMatchScore === 'function'
      ? d.getFuzzyMatchScore
      : (() => Number.POSITIVE_INFINITY);
    const getKeyboardSearchQuery = typeof d.getKeyboardSearchQuery === 'function'
      ? d.getKeyboardSearchQuery
      : (() => '');
    const setKeyboardSearchQuery = typeof d.setKeyboardSearchQuery === 'function'
      ? d.setKeyboardSearchQuery
      : () => {};
    const renderCurrentSection = typeof d.renderCurrentSection === 'function'
      ? d.renderCurrentSection
      : () => {};

    const addSectionLabel = typeof d.addSectionLabel === 'function' ? d.addSectionLabel : null;
    const addRow = typeof d.addRow === 'function' ? d.addRow : null;
    const setRowTarget = typeof d.setRowTarget === 'function' ? d.setRowTarget : null;
    const applyRowSearchHighlight = typeof d.applyRowSearchHighlight === 'function'
      ? d.applyRowSearchHighlight
      : null;
    const addDivider = typeof d.addDivider === 'function' ? d.addDivider : null;
    const makeShortcutKeyBox = typeof d.makeShortcutKeyBox === 'function' ? d.makeShortcutKeyBox : null;

    if (!addSearchInput || !addSectionLabel || !addRow || !setRowTarget || !applyRowSearchHighlight || !addDivider || !makeShortcutKeyBox) {
      return false;
    }

    const heading = document.createElement('h3');
    heading.textContent = 'Keyboard Shortcuts';
    container.appendChild(heading);

    addSearchInput(container, 'Search shortcuts', getKeyboardSearchQuery(), (value) => {
      setKeyboardSearchQuery(value);
      renderCurrentSection();
    });

    const query = normalizeSearchText(getKeyboardSearchQuery());
    const matchesShortcut = (label, desc, extra) => {
      if (!query) return true;
      return Number.isFinite(getFuzzyMatchScore(query, `${label} ${desc || ''} ${extra || ''}`));
    };

    let totalRendered = 0;
    const knownKeys = new Set();

    const coreGroups = Array.isArray(d.KEYBOARD_CORE_GROUPS) ? d.KEYBOARD_CORE_GROUPS : [];
    const coreLabels = d.KEYBOARD_CORE_LABELS && typeof d.KEYBOARD_CORE_LABELS === 'object'
      ? d.KEYBOARD_CORE_LABELS
      : {};

    for (let groupIndex = 0; groupIndex < coreGroups.length; groupIndex++) {
      const group = coreGroups[groupIndex];
      const rows = [];

      for (const key of group.keys || []) {
        knownKeys.add(key);
        const label = coreLabels[key] || toTitleCaseWords(key);
        if (!matchesShortcut(label, group.label, key)) continue;
        rows.push({ label, key });
      }

      if (rows.length === 0) continue;
      addSectionLabel(container, group.label);
      for (const row of rows) {
        const rowEl = addRow(container, row.label, null, makeShortcutKeyBox({ kind: 'core', key: row.key }));
        setRowTarget(rowEl, `keyboard:core:${row.key}`);
        applyRowSearchHighlight(rowEl, row.label, null, query);
        totalRendered++;
      }
      addDivider(container);
    }

    const keyboard = typeof d.getPendingKeyboardMap === 'function' ? d.getPendingKeyboardMap() : {};
    const extraKeys = Object.keys(keyboard)
      .filter((key) => key !== 'plugin_shortcuts' && key !== 'tool_window_shortcuts' && typeof keyboard[key] === 'string' && !knownKeys.has(key))
      .sort();

    if (extraKeys.length > 0) {
      const rows = [];
      for (const key of extraKeys) {
        const label = toTitleCaseWords(key);
        if (!matchesShortcut(label, 'Other', key)) continue;
        rows.push({ label, key });
      }

      if (rows.length > 0) {
        addSectionLabel(container, 'Other');
        for (const row of rows) {
          const rowEl = addRow(container, row.label, null, makeShortcutKeyBox({ kind: 'core', key: row.key }));
          setRowTarget(rowEl, `keyboard:core:${row.key}`);
          applyRowSearchHighlight(rowEl, row.label, null, query);
          totalRendered++;
        }
        addDivider(container);
      }
    }

    const toolWindowItems = typeof d.getToolWindowItems === 'function' ? d.getToolWindowItems() : [];
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
        addSectionLabel(container, 'Tool Windows');
        for (const row of rows) {
          const rowEl = addRow(container, row.label, row.desc, makeShortcutKeyBox({ kind: 'tool-window', key: row.id }));
          setRowTarget(rowEl, `keyboard:tool-window:${row.id}`);
          applyRowSearchHighlight(rowEl, row.label, row.desc, query);
          totalRendered++;
        }
        addDivider(container);
      }
    }

    const pluginMenuItems = typeof d.getPluginMenuItems === 'function' ? d.getPluginMenuItems() : [];
    const byPluginAction = new Map();
    for (const item of pluginMenuItems) {
      if (!item || !item.plugin || !item.action) continue;
      const uniqueKey = `${item.plugin}:${item.action}`;
      if (byPluginAction.has(uniqueKey)) continue;
      byPluginAction.set(uniqueKey, item);
    }

    const plugins = Array.from(byPluginAction.values()).sort((a, b) => {
      const pluginCmp = String(a.plugin || '').localeCompare(String(b.plugin || ''));
      if (pluginCmp !== 0) return pluginCmp;
      return String(a.label || '').localeCompare(String(b.label || ''));
    });

    if (plugins.length > 0) {
      const rows = [];
      for (const item of plugins) {
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
        addSectionLabel(container, 'Plugin Shortcuts');
        for (const row of rows) {
          const rowEl = addRow(container, row.label, row.desc, makeShortcutKeyBox({ kind: 'plugin', key: row.key, defaultValue: row.defaultValue }));
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
      container.appendChild(empty);
    }

    return true;
  }

  global.conchSettingsSectionsKeyboard = {
    renderKeyboard,
  };
})(window);
