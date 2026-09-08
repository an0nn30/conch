// Pure grouping, filtering, counting and ordering for the Problems window.
//
// No DOM, no IPC, no state: `build()` takes Rust's authoritative snapshot plus
// the current session statuses and returns everything the view and F8 need.
// Both consumers read the SAME `flat` array, which is why "what F8 walks" and
// "what the list shows" cannot drift apart.
//
// Ordering is total and independent of input order. That matters more than it
// looks: Rust hands the snapshot over sorted by URI and range, but the window
// groups by project first, and a HashMap-shaped tie anywhere in between would
// make F8 land somewhere different each time the same diagnostics arrived.
(function initTermLabProblemsModel(global) {
  'use strict';

  const SEVERITY_ORDER = ['error', 'warning', 'information', 'hint'];
  const SEVERITY_RANK = { error: 0, warning: 1, information: 2, hint: 3 };
  const SEVERITY_LABEL = {
    error: 'Error',
    warning: 'Warning',
    information: 'Information',
    hint: 'Hint',
  };
  const UNGROUPED_LABEL = 'Other files';

  function uriToPath(uri) {
    const uris = global.termlabLspUri;
    return uris && typeof uris.uriToPath === 'function' ? uris.uriToPath(uri) : String(uri || '');
  }

  function severityLabel(severity) {
    return SEVERITY_LABEL[severity] || SEVERITY_LABEL.error;
  }

  function basename(filePath) {
    const text = String(filePath || '');
    const at = text.lastIndexOf('/');
    return at >= 0 ? text.slice(at + 1) || text : text;
  }

  // --- roots ------------------------------------------------------------------
  //
  // Component-wise, never string-prefix: `/repo` is not an ancestor of
  // `/repository`, and a prefix test says it is. The Rust store makes the same
  // distinction; disagreeing here would file a diagnostic under a project it
  // does not belong to.

  function isDescendant(filePath, root) {
    if (!root) return false;
    if (filePath === root) return true;
    const prefix = root.charAt(root.length - 1) === '/' ? root : `${root}/`;
    return filePath.slice(0, prefix.length) === prefix;
  }

  function rootForPath(filePath, roots) {
    const target = String(filePath || '');
    let best = null;
    for (const root of roots || []) {
      const candidate = String(root || '');
      if (!candidate || !isDescendant(target, candidate)) continue;
      if (best === null || candidate.length > best.length) best = candidate;
    }
    return best;
  }

  function relativePath(filePath, root) {
    const target = String(filePath || '');
    if (!root || !isDescendant(target, root)) return target;
    if (target === root) return basename(target);
    const prefix = root.charAt(root.length - 1) === '/' ? root : `${root}/`;
    return target.slice(prefix.length);
  }

  // --- normalization ----------------------------------------------------------

  function normalize(item) {
    const start = (item && item.range && item.range.start) || { line: 0, character: 0 };
    const filePath = uriToPath(item && item.uri);
    return {
      id: String((item && item.id) || ''),
      uri: String((item && item.uri) || ''),
      path: filePath,
      fileName: basename(filePath),
      severity: SEVERITY_RANK[item && item.severity] === undefined ? 'error' : item.severity,
      code: item && item.code !== null && item.code !== undefined ? String(item.code) : '',
      source: item && item.source ? String(item.source) : '',
      message: String((item && item.message) || ''),
      range: (item && item.range) || null,
      // LSP counts from zero; every editor a user has ever seen counts from one.
      line: Number(start.line || 0) + 1,
      column: Number(start.character || 0) + 1,
    };
  }

  // --- counts -----------------------------------------------------------------

  function emptyCounts() {
    return {
      error: 0, warning: 0, information: 0, hint: 0, total: 0,
    };
  }

  function countsFor(items) {
    const out = emptyCounts();
    for (const item of items || []) {
      if (out[item.severity] === undefined) continue;
      out[item.severity] += 1;
      out.total += 1;
    }
    return out;
  }

  // --- filters ----------------------------------------------------------------

  function normalizeFilters(filters) {
    const given = (filters && filters.severities) || {};
    const severities = {};
    for (const severity of SEVERITY_ORDER) {
      severities[severity] = given[severity] !== false;
    }
    return {
      severities,
      text: String((filters && filters.text) || '').trim().toLowerCase(),
    };
  }

  function matchesFilters(item, filters) {
    const active = normalizeFilters(filters);
    if (!active.severities[item.severity]) return false;
    if (!active.text) return true;
    const haystack = `${item.message} ${item.path} ${item.source} ${item.code}`.toLowerCase();
    return haystack.indexOf(active.text) >= 0;
  }

  // --- ordering ---------------------------------------------------------------

  function compareStrings(left, right) {
    if (left < right) return -1;
    if (left > right) return 1;
    return 0;
  }

  // Severity first, then where it is, then a total tie-break so two
  // diagnostics reported at the same spot never swap places between renders.
  function compareItems(left, right) {
    return (SEVERITY_RANK[left.severity] - SEVERITY_RANK[right.severity])
      || (left.line - right.line)
      || (left.column - right.column)
      || compareStrings(left.source, right.source)
      || compareStrings(left.code, right.code)
      || compareStrings(left.message, right.message)
      || compareStrings(left.id, right.id);
  }

  // --- sessions ---------------------------------------------------------------

  // One entry per project root: a root may be served by several adapters, and
  // the group speaks for the root, not for any one server.
  function projectsFrom(sessions) {
    const byRoot = new Map();
    for (const status of sessions || []) {
      if (!status || !status.projectRootUri) continue;
      const root = uriToPath(status.projectRootUri);
      if (!root) continue;
      const entry = byRoot.get(root) || {
        root, adapters: [], state: 'ready', message: null,
      };
      if (status.adapterId && entry.adapters.indexOf(status.adapterId) < 0) {
        entry.adapters.push(String(status.adapterId));
      }
      // The worst state a root's servers are in is the one worth surfacing:
      // a project whose only Rust server died is not "ready" because its
      // TypeScript server is.
      if (worseState(status.state, entry.state)) {
        entry.state = status.state;
        entry.message = status.message || null;
      }
      byRoot.set(root, entry);
    }
    for (const entry of byRoot.values()) entry.adapters.sort(compareStrings);
    return Array.from(byRoot.values()).sort((a, b) => compareStrings(a.root, b.root));
  }

  const STATE_SEVERITY = {
    ready: 0,
    disabled: 1,
    choosingProject: 2,
    starting: 3,
    indexing: 3,
    untrusted: 4,
    unavailable: 5,
    failed: 6,
  };

  function worseState(candidate, current) {
    const a = STATE_SEVERITY[candidate];
    const b = STATE_SEVERITY[current];
    if (a === undefined) return false;
    return b === undefined || a > b;
  }

  // --- build ------------------------------------------------------------------

  function build(input) {
    const options = input || {};
    const filters = normalizeFilters(options.filters);
    const all = (options.items || []).map(normalize);
    const totals = countsFor(all);
    const visible = all.filter((item) => matchesFilters(item, filters));

    const projects = projectsFrom(options.sessions);
    const roots = projects.map((project) => project.root);

    const byRoot = new Map();
    for (const project of projects) {
      byRoot.set(project.root, {
        key: project.root,
        root: project.root,
        label: basename(project.root) || project.root,
        adapters: project.adapters,
        state: project.state,
        message: project.message,
        files: new Map(),
      });
    }

    for (const item of visible) {
      const root = rootForPath(item.path, roots);
      const key = root === null ? '' : root;
      let group = byRoot.get(key);
      if (!group) {
        group = {
          key: '',
          root: null,
          label: UNGROUPED_LABEL,
          adapters: [],
          state: null,
          message: null,
          files: new Map(),
        };
        byRoot.set('', group);
      }
      const relative = relativePath(item.path, root);
      const file = group.files.get(item.path) || {
        path: item.path,
        relativePath: relative,
        fileName: item.fileName,
        items: [],
      };
      file.items.push(item);
      group.files.set(item.path, file);
    }

    const groups = [];
    for (const group of byRoot.values()) {
      const files = Array.from(group.files.values())
        .sort((a, b) => compareStrings(a.relativePath, b.relativePath));
      for (const file of files) {
        file.items.sort(compareItems);
        file.counts = countsFor(file.items);
      }
      const items = files.reduce((acc, file) => acc.concat(file.items), []);
      const entry = {
        key: group.key,
        root: group.root,
        label: group.label,
        adapters: group.adapters,
        state: group.state,
        message: group.message,
        files,
        counts: countsFor(items),
      };
      // A healthy project with nothing to say is not worth a row. A project
      // whose server is starting, indexing, failed or untrusted is: that row
      // is the only place the absence gets explained.
      if (entry.counts.total > 0 || (entry.state && entry.state !== 'ready')) {
        groups.push(entry);
      }
    }
    // Rooted projects by path; the catch-all group last, whatever its name.
    groups.sort((a, b) => {
      if (a.root === null) return b.root === null ? 0 : 1;
      if (b.root === null) return -1;
      return compareStrings(a.root, b.root);
    });

    const flat = [];
    for (const group of groups) {
      for (const file of group.files) {
        for (const item of file.items) flat.push(item);
      }
    }

    return {
      groups, flat, totals, counts: countsFor(flat), filters,
    };
  }

  // --- states -----------------------------------------------------------------

  function panelState(input) {
    const options = input || {};
    if (!options.hydrated) return 'loading';
    const sessions = (options.sessions || []).filter((status) => status && status.state);
    if (!sessions.length) return 'disconnected';
    const broken = (status) => status.state === 'failed' || status.state === 'unavailable';
    if (sessions.every(broken)) return 'failed';
    if (Number(options.itemCount) > 0) return 'ready';
    const busy = (status) => status.state === 'indexing' || status.state === 'starting';
    if (sessions.some(busy)) return 'indexing';
    return 'empty';
  }

  // Wrapping traversal. `current < 0` means "nothing selected yet", where Next
  // starts at the top and Previous starts at the bottom.
  function stepIndex(length, current, delta) {
    if (!length) return -1;
    if (current === null || current === undefined || current < 0) {
      return delta > 0 ? 0 : length - 1;
    }
    return ((current + delta) % length + length) % length;
  }

  global.termlabProblemsModel = {
    SEVERITY_ORDER,
    UNGROUPED_LABEL,
    severityLabel,
    basename,
    normalize,
    rootForPath,
    relativePath,
    emptyCounts,
    countsFor,
    matchesFilters,
    normalizeFilters,
    compareItems,
    build,
    panelState,
    stepIndex,
  };
})(window);
