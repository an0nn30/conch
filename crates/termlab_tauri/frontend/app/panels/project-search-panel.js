// Project-wide text search — bottom zone, project windows only.
//
// The backend streams batches on `project-search-results`; this renders them
// grouped by file as they arrive, so a first hit in a large tree is on screen
// long before the walk finishes. Every emission carries the search id it
// belongs to, and anything from a superseded id is dropped on the floor — a
// cancelled search is silent by design.
//
// Row activation goes through editor-service's openLocalFileAt, which owns the
// app-wide ownership protocol and puts the jump on the Ctrl-O trail. The one
// conversion this file performs is 1-based search coordinates to the 0-based
// LSP positions that the editor's range API speaks; it happens exactly once,
// here, so no caller downstream has to know.
//
// Every string that came from the filesystem is written with textContent.
(function initTermLabProjectSearchPanel(global) {
  'use strict';

  const RESULTS_EVENT = 'project-search-results';
  const MAX_MATCHES = 1000;

  function el(tag, className) {
    const node = global.document.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  // Stable file order (first appearance wins) so a streaming batch never
  // reshuffles rows the user is reading.
  function groupByFile(matches) {
    const order = [];
    const byPath = new Map();
    for (const match of matches || []) {
      const key = match.relativePath;
      if (!byPath.has(key)) {
        const group = { relativePath: key, path: match.path, matches: [] };
        byPath.set(key, group);
        order.push(group);
      }
      byPath.get(key).matches.push(match);
    }
    return order;
  }

  function init(options) {
    const opts = options || {};
    const panelEl = opts.panelEl;
    if (!panelEl) throw new Error('Search panel requires panelEl');
    const invoke = opts.invoke;
    const listen = opts.listen;

    let caseSensitive = false;
    let activeSearchId = null;
    let state = 'idle';
    let capped = false;
    let matches = [];

    const root = el('div', 'tl-project-search');
    const toolbar = el('div', 'tl-project-search__toolbar');

    const input = el('input', 'tl-project-search__input');
    input.type = 'search';
    input.setAttribute('placeholder', 'Search in project');
    input.setAttribute('aria-label', 'Search in project');
    input.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter') return;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      runSearch();
    });

    const caseToggle = el('button', 'tl-project-search__toggle');
    caseToggle.type = 'button';
    caseToggle.textContent = 'Aa';
    caseToggle.title = 'Match case';
    caseToggle.setAttribute('data-search-case', 'toggle');
    caseToggle.setAttribute('aria-pressed', 'false');
    caseToggle.setAttribute('aria-label', 'Match case');
    caseToggle.addEventListener('click', () => {
      caseSensitive = !caseSensitive;
      caseToggle.setAttribute('aria-pressed', caseSensitive ? 'true' : 'false');
    });

    toolbar.appendChild(input);
    toolbar.appendChild(caseToggle);

    const statusHost = el('div', 'tl-project-search__status-host');
    const list = el('div', 'tl-project-search__list tl-scroll');
    list.setAttribute('role', 'tree');
    list.setAttribute('aria-label', 'Search results');

    root.appendChild(toolbar);
    root.appendChild(statusHost);
    root.appendChild(list);
    panelEl.appendChild(root);

    function statusText() {
      if (state === 'idle') return 'Search this project for text.';
      if (state === 'searching') return 'Searching…';
      if (!matches.length) return 'No results.';
      if (capped) {
        return 'Showing the first ' + MAX_MATCHES + ' matches — narrow the query for the rest.';
      }
      return matches.length + ' matches in ' + groupByFile(matches).length + ' files.';
    }

    function renderStatus() {
      const node = el('div', 'tl-project-search__status');
      node.setAttribute('role', 'status');
      // 'capped' is its own visual state, distinct from a plain 'done' — the
      // wording alone ("first 1000 matches") is not enough of a signal on a
      // status line a user may only glance at.
      node.setAttribute('data-state', state === 'done' && capped ? 'capped' : state);
      node.textContent = statusText();
      statusHost.replaceChildren(node);
    }

    function makeRow(kind, level) {
      const row = el('div', 'tl-project-search__row');
      row.setAttribute('role', 'treeitem');
      row.setAttribute('data-search-row', kind);
      row.setAttribute('aria-level', String(level));
      row.setAttribute('tabindex', '-1');
      return row;
    }

    function renderList() {
      const built = [];
      for (const group of groupByFile(matches)) {
        const fileRow = makeRow('file', 1);
        fileRow.classList.add('tl-project-search__row--file');
        const where = el('span', 'tl-project-search__where');
        where.textContent = group.relativePath;
        const count = el('span', 'tl-project-search__count');
        count.textContent = String(group.matches.length);
        fileRow.appendChild(where);
        fileRow.appendChild(count);
        fileRow.title = group.path;
        built.push(fileRow);

        for (const match of group.matches) {
          const row = makeRow('match', 2);
          row.classList.add('tl-project-search__row--match');
          const line = el('span', 'tl-project-search__line');
          line.textContent = String(match.line);
          const preview = el('span', 'tl-project-search__preview');
          preview.textContent = match.preview;
          row.appendChild(line);
          row.appendChild(preview);
          row.title = match.path + ':' + match.line;
          row.setAttribute('aria-label', group.relativePath + ' line ' + match.line + ': ' + match.preview);
          row._match = match;
          built.push(row);
        }
      }
      list.replaceChildren(...built);
    }

    function render() {
      renderStatus();
      renderList();
    }

    function runSearch() {
      const query = String(input.value || '').trim();
      matches = [];
      capped = false;
      if (!query) {
        activeSearchId = null;
        state = 'idle';
        Promise.resolve(invoke('project_search_cancel')).catch(() => {});
        render();
        return;
      }
      state = 'searching';
      render();
      Promise.resolve(invoke('project_search', { query, caseSensitive }))
        .then((searchId) => { activeSearchId = searchId; })
        .catch((error) => {
          activeSearchId = null;
          state = 'idle';
          render();
          if (global.toast) global.toast.error('Search Failed', String(error));
        });
    }

    function onResults(event) {
      const payload = (event && event.payload) || null;
      if (!payload || payload.searchId !== activeSearchId) return;
      matches = matches.concat(payload.matches || []);
      if (payload.done) {
        state = 'done';
        capped = payload.capped === true;
      }
      render();
    }

    const unlisten = typeof listen === 'function' ? listen(RESULTS_EVENT, onResults) : null;

    function activate(row) {
      const match = row && row._match;
      if (!match) return;
      const service = global.termlabEditorService;
      if (!service || typeof service.openLocalFileAt !== 'function') return;
      // Search lines are 1-based; LSP ranges are 0-based.
      const zeroBased = Math.max(0, Number(match.line) - 1);
      const position = { line: zeroBased, character: 0 };
      Promise.resolve(
        service.openLocalFileAt(match.path, { start: position, end: { line: zeroBased, character: 0 } }, { focus: true }),
      ).catch((error) => {
        console.warn('project search: could not open the match', error);
      });
    }

    list.addEventListener('click', (event) => {
      const row = event.target && typeof event.target.closest === 'function'
        ? event.target.closest('[data-search-row]')
        : event.target;
      if (!row) return;
      activate(row);
    });

    list.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter') return;
      const row = event.target;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      activate(row);
    });

    render();

    return {
      element: root,
      focusInput() {
        if (typeof input.focus === 'function') input.focus();
      },
      destroy() {
        Promise.resolve(invoke('project_search_cancel')).catch(() => {});
        if (typeof unlisten === 'function') unlisten();
        else if (unlisten && typeof unlisten.then === 'function') unlisten.then((fn) => { if (typeof fn === 'function') fn(); }).catch(() => {});
      },
    };
  }

  global.projectSearchPanel = { init, groupByFile };
})(window);
