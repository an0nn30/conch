(function initTermLabSettingsSidebar(global) {
  'use strict';

  // Real <button> elements (Task 1 brief: "Rows keep role=button/tabindex=0
  // but should become real <button> elements unless that breaks the
  // existing keyboard handling"). It doesn't: attachActivatableItem's
  // Enter/Space keydown handler calls event.preventDefault() before invoking
  // onActivate(), which suppresses the button's own native
  // Enter/Space-triggers-click activation, so onActivate() still runs
  // exactly once per press — same contract as the old <div role="button">
  // rows, now with real focus/keyboard semantics (and no need to set
  // role="button" — <button>'s implicit role already is button — but it's
  // harmless to set redundantly and keeps this helper correct for any
  // future non-button caller too).
  function attachActivatableItem(el, onActivate) {
    if (!el || typeof onActivate !== 'function') return;
    el.setAttribute('role', 'button');
    el.tabIndex = 0;
    el.addEventListener('click', onActivate);
    el.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      event.preventDefault();
      onActivate();
    });
  }

  function makeItemButton(extraClassName) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'tl-settings__item' + (extraClassName ? ' ' + extraClassName : '');
    return btn;
  }

  function renderSidebarInto(sidebar, deps) {
    if (!sidebar) return;
    const d = deps || {};

    const sectionDefs = Array.isArray(d.sectionDefs) ? d.sectionDefs : [];
    const normalizeSearchText = typeof d.normalizeSearchText === 'function'
      ? d.normalizeSearchText
      : (value) => String(value || '').trim().toLowerCase();
    const getFuzzyMatchScore = typeof d.getFuzzyMatchScore === 'function'
      ? d.getFuzzyMatchScore
      : (() => Number.POSITIVE_INFINITY);
    const getSidebarSearchResults = typeof d.getSidebarSearchResults === 'function'
      ? d.getSidebarSearchResults
      : (() => []);
    const appendHighlightedText = typeof d.appendHighlightedText === 'function'
      ? d.appendHighlightedText
      : ((el, text) => { el.textContent = String(text || ''); });

    const getSidebarQuery = typeof d.getSidebarQuery === 'function' ? d.getSidebarQuery : () => '';
    const setSidebarQuery = typeof d.setSidebarQuery === 'function' ? d.setSidebarQuery : () => {};
    const getSidebarSelectionIndex = typeof d.getSidebarSelectionIndex === 'function' ? d.getSidebarSelectionIndex : () => -1;
    const setSidebarSelectionIndex = typeof d.setSidebarSelectionIndex === 'function' ? d.setSidebarSelectionIndex : () => {};
    const getSidebarResults = typeof d.getSidebarResults === 'function' ? d.getSidebarResults : () => [];
    const setSidebarResults = typeof d.setSidebarResults === 'function' ? d.setSidebarResults : () => {};
    const getCurrentSection = typeof d.getCurrentSection === 'function' ? d.getCurrentSection : () => '';
    const moveSidebarSearchSelection = typeof d.moveSidebarSearchSelection === 'function' ? d.moveSidebarSearchSelection : () => {};
    const onSidebarSearchResultSelected = typeof d.onSidebarSearchResultSelected === 'function' ? d.onSidebarSearchResultSelected : () => {};
    const selectSection = typeof d.selectSection === 'function' ? d.selectSection : () => {};
    // Group-header disclosure state (METRICS.md: "a disclosure tree
    // (Appearance & Behavior > children)"). Lives in the settings store —
    // see store.js's isSidebarGroupCollapsed/toggleSidebarGroupCollapsed —
    // so it survives the re-render this function does on every keystroke and
    // every section selection.
    const isGroupCollapsed = typeof d.isGroupCollapsed === 'function' ? d.isGroupCollapsed : () => false;
    const toggleGroupCollapsed = typeof d.toggleGroupCollapsed === 'function' ? d.toggleGroupCollapsed : () => {};

    sidebar.innerHTML = '';
    setSidebarResults([]);

    const searchWrap = document.createElement('div');
    searchWrap.className = 'tl-settings__search-wrap';
    const searchInput = document.createElement('input');
    searchInput.type = 'search';
    searchInput.className = 'tl-input tl-settings__search';
    searchInput.placeholder = 'Search settings';
    searchInput.value = getSidebarQuery();
    searchInput.addEventListener('input', () => {
      setSidebarQuery(searchInput.value);
      setSidebarSelectionIndex(-1);
      const active = document.activeElement === searchInput;
      renderSidebarInto(sidebar, d);
      if (active) {
        const nextInput = sidebar.querySelector('.tl-settings__search');
        if (nextInput) {
          nextInput.focus();
          nextInput.setSelectionRange(nextInput.value.length, nextInput.value.length);
        }
      }
    });
    searchInput.addEventListener('keydown', (event) => {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        event.stopPropagation();
        moveSidebarSearchSelection(1);
        return;
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        event.stopPropagation();
        moveSidebarSearchSelection(-1);
        return;
      }
      if (event.key === 'Enter') {
        const results = getSidebarResults();
        if (!Array.isArray(results) || results.length === 0) return;
        event.preventDefault();
        event.stopPropagation();
        const idx = getSidebarSelectionIndex() >= 0 ? getSidebarSelectionIndex() : 0;
        const match = results[idx];
        if (match) onSidebarSearchResultSelected(match);
      }
    });
    searchWrap.appendChild(searchInput);
    sidebar.appendChild(searchWrap);

    const q = normalizeSearchText(getSidebarQuery());
    if (q) {
      const sectionMatches = [];
      for (const group of sectionDefs) {
        for (const item of group.items) {
          const haystack = `${item.label} ${item.description || ''} ${item.keywords || ''}`;
          if (!Number.isFinite(getFuzzyMatchScore(q, haystack, [group.group, item.id]))) continue;
          sectionMatches.push(item);
        }
      }

      const settingMatches = getSidebarSearchResults(q);
      const combinedResults = [
        ...settingMatches,
        ...sectionMatches.map((item) => ({
          section: item.id,
          label: item.label,
          path: item.description || item.label,
          kind: 'section',
          targetId: null,
        })),
      ];
      setSidebarResults(combinedResults);
      if (getSidebarSelectionIndex() >= combinedResults.length) {
        setSidebarSelectionIndex(combinedResults.length - 1);
      }

      if (sectionMatches.length > 0) {
        const header = document.createElement('div');
        header.className = 'tl-settings__group';
        header.textContent = 'Sections';
        sidebar.appendChild(header);

        for (let idx = 0; idx < sectionMatches.length; idx++) {
          const item = sectionMatches[idx];
          const resultIndex = settingMatches.length + idx;
          const row = makeItemButton('tl-settings__item--search'
            + (item.id === getCurrentSection() ? ' is-active' : '')
            + (getSidebarSelectionIndex() === resultIndex ? ' is-selected' : ''));
          row.dataset.section = item.id;
          row.setAttribute('aria-current', item.id === getCurrentSection() ? 'page' : 'false');

          const title = document.createElement('div');
          title.className = 'tl-settings__item-title';
          appendHighlightedText(title, item.label, q);
          row.appendChild(title);

          if (item.description) {
            const desc = document.createElement('div');
            desc.className = 'tl-settings__item-desc';
            appendHighlightedText(desc, item.description, q);
            row.appendChild(desc);
          }

          attachActivatableItem(row, () => {
            const results = getSidebarResults();
            const match = Array.isArray(results) ? results[resultIndex] : null;
            if (match) onSidebarSearchResultSelected(match);
          });
          sidebar.appendChild(row);
        }
      }

      if (settingMatches.length > 0) {
        const header = document.createElement('div');
        header.className = 'tl-settings__group';
        header.textContent = 'Settings';
        sidebar.appendChild(header);

        for (let idx = 0; idx < settingMatches.length; idx++) {
          const match = settingMatches[idx];
          const row = makeItemButton('tl-settings__item--search'
            + (getSidebarSelectionIndex() === idx ? ' is-selected' : ''));
          row.dataset.section = match.section;
          row.setAttribute('aria-current', match.section === getCurrentSection() ? 'page' : 'false');

          const title = document.createElement('div');
          title.className = 'tl-settings__item-title';
          appendHighlightedText(title, match.label, q);
          row.appendChild(title);

          const desc = document.createElement('div');
          desc.className = 'tl-settings__item-desc';
          appendHighlightedText(desc, match.path || match.sectionLabel, q);
          row.appendChild(desc);

          attachActivatableItem(row, () => onSidebarSearchResultSelected(match));
          sidebar.appendChild(row);
        }
      }

      if (sectionMatches.length === 0 && settingMatches.length === 0) {
        const empty = document.createElement('div');
        empty.className = 'tl-settings__empty';
        empty.textContent = 'No settings match your search.';
        sidebar.appendChild(empty);
      }
      return;
    }

    setSidebarResults([]);
    setSidebarSelectionIndex(-1);

    for (const group of sectionDefs) {
      const collapsed = isGroupCollapsed(group.group);

      const groupEl = document.createElement('button');
      groupEl.type = 'button';
      groupEl.className = 'tl-settings__group';
      groupEl.setAttribute('aria-expanded', collapsed ? 'false' : 'true');
      const chevron = global.tlIcon && typeof global.tlIcon.create === 'function'
        ? global.tlIcon.create(collapsed ? 'chevronRight' : 'chevronDown', { size: 12 })
        : null;
      if (chevron) {
        chevron.className += ' tl-settings__group-chevron';
        groupEl.appendChild(chevron);
      }
      const groupLabel = document.createElement('span');
      groupLabel.textContent = group.group;
      groupEl.appendChild(groupLabel);
      groupEl.addEventListener('click', () => {
        toggleGroupCollapsed(group.group);
        renderSidebarInto(sidebar, d);
      });
      sidebar.appendChild(groupEl);

      if (collapsed) continue;

      for (const item of group.items) {
        const itemEl = makeItemButton(item.id === getCurrentSection() ? 'is-active' : '');
        itemEl.dataset.section = item.id;
        itemEl.setAttribute('aria-current', item.id === getCurrentSection() ? 'page' : 'false');
        itemEl.textContent = item.label;

        attachActivatableItem(itemEl, () => selectSection(item.id));
        sidebar.appendChild(itemEl);
      }
    }
  }

  global.termlabSettingsSidebar = {
    renderSidebarInto,
  };
})(window);
