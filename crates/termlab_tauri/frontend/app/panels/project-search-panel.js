// Project-wide text search — bottom zone, project windows only.
//
// The backend streams batches on `project-search-results`; this renders them
// grouped by file as they arrive, so a first hit in a large tree is on screen
// long before the walk finishes. Every emission carries the search id it
// belongs to, and anything from a superseded id is dropped on the floor — a
// cancelled search is silent by design.
//
// EARLY EVENTS: `project_search`'s reply (the search id) and the
// `project-search-results` events it triggers travel independent channels
// with no ordering guarantee — the walker thread is spawned before the
// command returns. A fast walk can flush a batch, even the terminal one,
// before the invoke that started it has resolved in this window. Payloads
// that arrive before the id is known are held in `earlyBatches` and replayed
// against the confirmed id the moment it arrives — the identical hazard (and
// fix shape) documented on editor-service.js's `early` map for
// transfer-progress events.
//
// Row activation goes through editor-service's openLocalFileAt, which owns the
// app-wide ownership protocol and puts the jump on the Ctrl-O trail. The one
// conversion this file performs is 1-based search coordinates to the 0-based
// LSP positions that the editor's range API speaks; it happens exactly once,
// here, so no caller downstream has to know.
//
// Keyboard navigation follows the Problems panel's roving-tabindex
// convention (arrows/Home/End move it, Enter activates the row under it,
// click uses .closest() to find the row), except the active row's identity
// is tracked by a stable key rather than an index — a streaming batch can
// insert new rows in the middle of the flat list (a hit for an
// already-listed file pushes every row after it down), and an index-based
// "active row" would silently point at the wrong row after that, the same
// class of bug project-tree's F2 fix addressed for its own re-renders.
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

  // Whether `node` is `ancestor` or nested inside it — used to tell whether
  // keyboard focus currently lives inside the results list, since replacing
  // the list's children would otherwise silently drop focus to <body> and
  // further arrow presses would stop reaching the container.
  function isWithin(node, ancestor) {
    let current = node;
    while (current) {
      if (current === ancestor) return true;
      current = current.parentNode;
    }
    return false;
  }

  function matchKey(match) {
    return 'match:' + match.path + ':' + match.line + ':' + match.column;
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

    // Guards the window between calling `project_search` and its promise
    // resolving with the real id: while true, incoming payloads cannot be
    // judged (their `searchId` cannot yet be compared to anything) and are
    // held in `earlyBatches` instead of being discarded outright.
    let awaitingConfirmation = false;
    let earlyBatches = [];
    // Bumped on every runSearch()/cancelSearch() call. A `.then()` callback
    // from an invoke started by an EARLIER call closes over the generation it
    // was issued in, so if the user starts (or cancels) another search before
    // that older reply lands, it recognizes itself as superseded and does
    // nothing — the confirmation, and any buffered replay, belong only to the
    // most recent call.
    let searchGeneration = 0;

    // Roving-tabindex bookkeeping — see the file header for why this is a
    // key, not an index.
    let rows = [];
    let activeKey = null;

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
    // Clearing the box by hand (backspace/cut/delete) — not just an empty
    // Enter — must not leave a walk running forever in the background with
    // nothing on screen to cancel it from.
    input.addEventListener('input', () => {
      if (String(input.value || '').trim()) return;
      cancelSearch();
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

    // Flat, in-render-order descriptors — one pass to build, one pass to
    // render, and the same list `moveTo`/`activateAt` index against.
    function buildDescriptors() {
      const out = [];
      for (const group of groupByFile(matches)) {
        out.push({ kind: 'file', key: 'file:' + group.relativePath, group });
        for (const match of group.matches) {
          out.push({
            kind: 'match', key: matchKey(match), group, match,
          });
        }
      }
      return out;
    }

    function renderFileRow(descriptor) {
      const group = descriptor.group;
      const row = makeRow('file', 1);
      row.classList.add('tl-project-search__row--file');
      const where = el('span', 'tl-project-search__where');
      where.textContent = group.relativePath;
      const count = el('span', 'tl-project-search__count');
      count.textContent = String(group.matches.length);
      row.appendChild(where);
      row.appendChild(count);
      row.title = group.path;
      return row;
    }

    function renderMatchRow(descriptor) {
      const group = descriptor.group;
      const match = descriptor.match;
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
      return row;
    }

    function renderRow(descriptor) {
      const row = descriptor.kind === 'file' ? renderFileRow(descriptor) : renderMatchRow(descriptor);
      row._key = descriptor.key;
      return row;
    }

    function applyActiveTabIndex() {
      rows.forEach((row) => {
        const isActive = row._key === activeKey;
        row.setAttribute('tabindex', isActive ? '0' : '-1');
        row.classList.toggle('is-active', isActive);
      });
    }

    function renderList() {
      // Both checked and restored around the replace below, the same shape
      // as project-tree.js's render(): a real DOM drops focus to <body> the
      // moment the focused node is removed from it, and list.replaceChildren
      // removes every row on every batch.
      const hadFocus = isWithin(global.document.activeElement, list);

      const descriptors = buildDescriptors();
      const built = descriptors.map(renderRow);
      list.replaceChildren(...built);
      rows = built;

      // Keep pointing at the same row it named before, if it is still here;
      // otherwise fall back to the first row so the list is never left with
      // nothing in the tab order.
      if (activeKey && !rows.some((row) => row._key === activeKey)) activeKey = null;
      if (!activeKey && rows.length) activeKey = rows[0]._key;
      applyActiveTabIndex();

      if (hadFocus) {
        const target = rows.find((row) => row._key === activeKey) || list;
        if (target && typeof target.focus === 'function') target.focus();
      }
    }

    function render() {
      renderStatus();
      renderList();
    }

    // The one place a payload actually becomes visible state — called either
    // immediately (the ordinary case: the id was already confirmed) or later,
    // when a buffered early payload is replayed against the id it turns out
    // to belong to.
    function applyPayload(payload) {
      matches = matches.concat(payload.matches || []);
      if (payload.done) {
        state = 'done';
        capped = payload.capped === true;
      }
      render();
    }

    function resetSearchState() {
      searchGeneration += 1;
      matches = [];
      capped = false;
      earlyBatches = [];
      activeSearchId = null;
      awaitingConfirmation = false;
      activeKey = null;
    }

    function cancelSearch() {
      const wasRunning = awaitingConfirmation || state === 'searching' || state === 'done';
      resetSearchState();
      state = 'idle';
      if (wasRunning) Promise.resolve(invoke('project_search_cancel')).catch(() => {});
      render();
    }

    function runSearch() {
      const query = String(input.value || '').trim();
      if (!query) {
        cancelSearch();
        return;
      }
      resetSearchState();
      const myGeneration = searchGeneration;
      awaitingConfirmation = true;
      state = 'searching';
      render();
      Promise.resolve(invoke('project_search', { query, caseSensitive }))
        .then((searchId) => {
          if (myGeneration !== searchGeneration) return; // superseded while in flight
          activeSearchId = searchId;
          awaitingConfirmation = false;
          const buffered = earlyBatches;
          earlyBatches = [];
          for (const payload of buffered) {
            if (payload.searchId === activeSearchId) applyPayload(payload);
          }
        })
        .catch((error) => {
          if (myGeneration !== searchGeneration) return;
          activeSearchId = null;
          awaitingConfirmation = false;
          state = 'idle';
          render();
          if (global.toast) global.toast.error('Search Failed', String(error));
        });
    }

    function onResults(event) {
      const payload = (event && event.payload) || null;
      if (!payload) return;
      if (awaitingConfirmation) {
        // The id is not confirmed yet, so there is no way to judge whose
        // batch this is. Hold it — see the file header.
        earlyBatches.push(payload);
        return;
      }
      if (payload.searchId !== activeSearchId) return; // superseded — drop it
      applyPayload(payload);
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

    function currentActiveIndex() {
      const at = rows.findIndex((row) => row._key === activeKey);
      return at >= 0 ? at : 0;
    }

    function moveTo(index) {
      if (!rows.length) return;
      const clamped = Math.min(Math.max(index, 0), rows.length - 1);
      const row = rows[clamped];
      activeKey = row._key;
      applyActiveTabIndex();
      if (typeof row.focus === 'function') row.focus();
    }

    // `refocus` mirrors the Problems panel: true for a keyboard activation
    // (which already holds focus and should keep it), unset for a click
    // (which the browser already focused on its own).
    function activateAt(index, refocus) {
      const row = rows[index];
      if (!row) return;
      activeKey = row._key;
      applyActiveTabIndex();
      if (refocus && typeof row.focus === 'function') row.focus();
      activate(row);
    }

    list.addEventListener('click', (event) => {
      const row = event.target && typeof event.target.closest === 'function'
        ? event.target.closest('[data-search-row]')
        : null;
      if (!row) return;
      const index = rows.indexOf(row);
      if (index < 0) return;
      activateAt(index);
    });

    list.addEventListener('keydown', (event) => {
      const key = event.key;
      if (key === 'ArrowDown') { moveTo(currentActiveIndex() + 1); event.preventDefault(); return; }
      if (key === 'ArrowUp') { moveTo(currentActiveIndex() - 1); event.preventDefault(); return; }
      if (key === 'Home') { moveTo(0); event.preventDefault(); return; }
      if (key === 'End') { moveTo(rows.length - 1); event.preventDefault(); return; }
      if (key === 'Enter' || key === ' ' || key === 'Spacebar') {
        activateAt(currentActiveIndex(), true);
        event.preventDefault();
      }
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
