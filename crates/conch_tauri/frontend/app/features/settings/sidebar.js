(function initConchSettingsSidebar(global) {
  'use strict';

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

    sidebar.innerHTML = '';
    setSidebarResults([]);

    const searchWrap = document.createElement('div');
    searchWrap.className = 'settings-sidebar-search-wrap';
    const searchInput = document.createElement('input');
    searchInput.type = 'search';
    searchInput.className = 'settings-sidebar-search';
    searchInput.placeholder = 'Search settings';
    searchInput.value = getSidebarQuery();
    searchInput.addEventListener('input', () => {
      setSidebarQuery(searchInput.value);
      setSidebarSelectionIndex(-1);
      const active = document.activeElement === searchInput;
      renderSidebarInto(sidebar, d);
      if (active) {
        const nextInput = sidebar.querySelector('.settings-sidebar-search');
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
        header.className = 'settings-sidebar-group';
        header.textContent = 'Sections';
        sidebar.appendChild(header);

        for (let idx = 0; idx < sectionMatches.length; idx++) {
          const item = sectionMatches[idx];
          const resultIndex = settingMatches.length + idx;
          const row = document.createElement('div');
          row.className = 'settings-sidebar-item settings-sidebar-item-search'
            + (item.id === getCurrentSection() ? ' active' : '')
            + (getSidebarSelectionIndex() === resultIndex ? ' selected' : '');
          row.dataset.section = item.id;
          row.setAttribute('aria-current', item.id === getCurrentSection() ? 'page' : 'false');

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
        header.className = 'settings-sidebar-group';
        header.textContent = 'Settings';
        sidebar.appendChild(header);

        for (let idx = 0; idx < settingMatches.length; idx++) {
          const match = settingMatches[idx];
          const row = document.createElement('div');
          row.className = 'settings-sidebar-item settings-sidebar-item-search'
            + (getSidebarSelectionIndex() === idx ? ' selected' : '');
          row.dataset.section = match.section;
          row.setAttribute('aria-current', match.section === getCurrentSection() ? 'page' : 'false');

          const title = document.createElement('div');
          title.className = 'settings-sidebar-item-title';
          appendHighlightedText(title, match.label, q);
          row.appendChild(title);

          const desc = document.createElement('div');
          desc.className = 'settings-sidebar-item-desc';
          appendHighlightedText(desc, match.path || match.sectionLabel, q);
          row.appendChild(desc);

          attachActivatableItem(row, () => onSidebarSearchResultSelected(match));
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

    setSidebarResults([]);
    setSidebarSelectionIndex(-1);

    for (const group of sectionDefs) {
      const groupEl = document.createElement('div');
      groupEl.className = 'settings-sidebar-group';
      groupEl.textContent = group.group;
      sidebar.appendChild(groupEl);

      for (const item of group.items) {
        const itemEl = document.createElement('div');
        itemEl.className = 'settings-sidebar-item' + (item.id === getCurrentSection() ? ' active' : '');
        itemEl.dataset.section = item.id;
        itemEl.setAttribute('aria-current', item.id === getCurrentSection() ? 'page' : 'false');

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

        attachActivatableItem(itemEl, () => selectSection(item.id));
        sidebar.appendChild(itemEl);
      }
    }
  }

  global.conchSettingsSidebar = {
    renderSidebarInto,
  };
})(window);
