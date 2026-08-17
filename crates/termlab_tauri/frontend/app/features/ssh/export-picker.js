// Export picker — a filterable, bounded, multi-select list for the export
// dialog.
//
// The export dialog used to emit every server and tunnel as one unbounded run
// of checkboxes, which is fine for a handful of hosts and unusable at a few
// hundred: no way to find anything, no sense of how much is selected, and a
// dialog that grows past the window.
//
// This owns the list surface only. It deliberately keeps the DOM contract the
// dialog's submit handler already depends on — every row carries an
// `input[data-type="server"|"tunnel"][value=<id>]` — so selection is still read
// straight off the DOM and nothing downstream had to change.
(function (exports) {
  'use strict';

  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
    }[c]));
  }

  // Every whitespace-separated term must appear somewhere in the haystack, so
  // "prod ubuntu" matches a host labelled prod with user ubuntu regardless of
  // the order they appear in the row. Case-insensitive; an empty query matches
  // everything.
  //
  // Exported for tests: this is the whole of the filter's behaviour.
  function matchesFilter(haystack, query) {
    const q = String(query == null ? '' : query).trim().toLowerCase();
    if (!q) return true;
    const hay = String(haystack == null ? '' : haystack).toLowerCase();
    return q.split(/\s+/).every((term) => hay.indexOf(term) !== -1);
  }

  function rowHtml(entry, type) {
    const haystack = [entry.label, entry.detail].filter(Boolean).join(' ');
    return `<label class="tl-check tl-picker__row" data-search="${esc(haystack)}">`
      + `<input type="checkbox" data-type="${esc(type)}" value="${esc(entry.id)}"${entry.checked ? ' checked' : ''} />`
      + `<span class="tl-picker__row-label">${esc(entry.label)}</span>`
      + (entry.detail ? `<span class="tl-picker__row-detail" title="${esc(entry.detail)}">${esc(entry.detail)}</span>` : '')
      + `</label>`;
  }

  function groupHtml(group, type) {
    const rows = group.entries.map((e) => rowHtml(e, type)).join('');
    return `<div class="tl-picker__group">`
      + `<button type="button" class="tl-picker__group-header" aria-expanded="true">`
      + `<span class="tl-picker__chevron" aria-hidden="true"></span>`
      + `<span class="tl-picker__group-label">${esc(group.label)}</span>`
      + `<span class="tl-picker__group-count">${group.entries.length}</span>`
      + `</button>`
      + `<div class="tl-picker__group-body">${rows}</div>`
      + `</div>`;
  }

  function sectionHtml(section) {
    const total = section.groups.reduce((n, g) => n + g.entries.length, 0);
    if (!total) return '';
    return `<div class="tl-picker__section" data-section="${esc(section.id)}">`
      + `<div class="tl-picker__section-head">`
      + `<span class="tl-picker__section-title">${esc(section.label)}</span>`
      + `<span class="tl-picker__summary" data-role="summary"></span>`
      + `<button type="button" class="tl-picker__action" data-act="all">All</button>`
      + `<button type="button" class="tl-picker__action" data-act="none">None</button>`
      + `</div>`
      + `<div class="tl-picker__box tl-scroll">`
      + section.groups.map((g) => groupHtml(g, section.type)).join('')
      + `<div class="tl-picker__empty" hidden>No matches</div>`
      + `</div></div>`;
  }

  /**
   * Render the picker into `container`.
   *
   * sections: [{ id, label, type: 'server'|'tunnel', groups: [{ label, entries: [{ id, label, detail, checked }] }] }]
   *
   * Returns { refresh } — refresh() recomputes the per-section counts, which
   * the dialog calls after programmatic selection changes.
   */
  function mount(container, sections) {
    const visible = sections.filter((s) => s.groups.some((g) => g.entries.length));
    container.innerHTML =
      `<div class="tl-picker">`
      + `<input type="search" class="tl-input tl-picker__filter" data-role="filter" placeholder="Filter by name, user or host…" aria-label="Filter connections" />`
      + visible.map(sectionHtml).join('')
      + `</div>`;

    const filterEl = container.querySelector('[data-role="filter"]');

    function updateSummaries() {
      container.querySelectorAll('.tl-picker__section').forEach((section) => {
        const boxes = section.querySelectorAll('input[data-type]');
        const checked = section.querySelectorAll('input[data-type]:checked').length;
        const summary = section.querySelector('[data-role="summary"]');
        if (summary) summary.textContent = `${checked} of ${boxes.length} selected`;
      });
    }

    function applyFilter() {
      const q = filterEl ? filterEl.value : '';
      container.querySelectorAll('.tl-picker__section').forEach((section) => {
        let sectionVisible = 0;
        section.querySelectorAll('.tl-picker__group').forEach((group) => {
          let groupVisible = 0;
          group.querySelectorAll('.tl-picker__row').forEach((row) => {
            const show = matchesFilter(row.getAttribute('data-search'), q);
            row.hidden = !show;
            if (show) groupVisible++;
          });
          // A group with no surviving rows disappears entirely rather than
          // leaving a header stranded over nothing.
          group.hidden = groupVisible === 0;
          sectionVisible += groupVisible;
        });
        const empty = section.querySelector('.tl-picker__empty');
        if (empty) empty.hidden = sectionVisible !== 0;
      });
    }

    container.addEventListener('click', (e) => {
      const header = e.target.closest('.tl-picker__group-header');
      if (header) {
        const group = header.closest('.tl-picker__group');
        const collapsed = group.classList.toggle('is-collapsed');
        header.setAttribute('aria-expanded', collapsed ? 'false' : 'true');
        return;
      }
      const action = e.target.closest('.tl-picker__action');
      if (action) {
        e.preventDefault();
        const section = action.closest('.tl-picker__section');
        const wanted = action.getAttribute('data-act') === 'all';
        // Acts on what the filter currently shows, so "All" after typing a
        // filter selects the matches rather than silently selecting everything.
        section.querySelectorAll('.tl-picker__row').forEach((row) => {
          if (row.hidden) return;
          const box = row.querySelector('input[data-type]');
          if (box) box.checked = wanted;
        });
        updateSummaries();
        // The dialog gates its Export button on a change event from the body.
        container.dispatchEvent(new Event('change', { bubbles: true }));
      }
    });

    container.addEventListener('change', updateSummaries);
    if (filterEl) filterEl.addEventListener('input', applyFilter);

    applyFilter();
    updateSummaries();

    return { refresh: updateSummaries, focusFilter: () => filterEl && filterEl.focus() };
  }

  exports.termlabExportPicker = { matchesFilter, mount };
})(window);
