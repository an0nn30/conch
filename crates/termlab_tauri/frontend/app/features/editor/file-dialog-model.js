// File dialog model: sorting, filtering, breadcrumbs, and path arithmetic
// for the unified local/remote file-open and save-as dialog. Pure — no DOM,
// no invokes — so it is testable without a running app. `file-dialog.js`
// (a later task) is the only thing that renders this.
//
// Entries are the Rust `FileEntry` shape, verbatim:
//   { name: string, is_dir: bool, size: number, modified: number|null,
//     permissions: string|null }
// The backend never emits `.`/`..` entries, so filtering/sorting does not
// special-case them.
(function initTermLabFileDialogModel(global) {
  'use strict';

  // Same semantics as `matchesFilter` in app/features/ssh/export-picker.js:
  // every whitespace-separated term must appear in the haystack, case
  // insensitive. Kept in lockstep with that copy rather than shared because
  // this is a plain-script (non-module) codebase with no import graph.
  function matchesFilter(haystack, query) {
    const q = String(query == null ? '' : query).trim().toLowerCase();
    if (!q) return true;
    const hay = String(haystack == null ? '' : haystack).toLowerCase();
    return q.split(/\s+/).every((term) => hay.indexOf(term) !== -1);
  }

  // Directories first, then case-insensitive name order. Returns a new
  // array; never mutates the input or its entries.
  function sortEntries(entries) {
    const list = Array.isArray(entries) ? entries.slice() : [];
    list.sort((a, b) => {
      const aDir = !!(a && a.is_dir);
      const bDir = !!(b && b.is_dir);
      if (aDir !== bDir) return aDir ? -1 : 1;
      const aName = String((a && a.name) || '').toLowerCase();
      const bName = String((b && b.name) || '').toLowerCase();
      if (aName < bName) return -1;
      if (aName > bName) return 1;
      return 0;
    });
    return list;
  }

  // `query`: every whitespace-separated term must appear in the entry name
  // (see matchesFilter above). `showHidden`: when false, entries whose name
  // starts with a dot are excluded regardless of query.
  function filterEntries(entries, query, showHidden) {
    const list = Array.isArray(entries) ? entries : [];
    return list.filter((e) => {
      const name = String((e && e.name) || '');
      if (!showHidden && name.charAt(0) === '.') return false;
      return matchesFilter(name, query);
    });
  }

  // '/a/b/c' -> [{label:'/', path:'/'}, {label:'a', path:'/a'},
  //              {label:'b', path:'/a/b'}, {label:'c', path:'/a/b/c'}]
  // Empty/null/root all yield just the root crumb. A trailing slash (or any
  // run of slashes) never produces an extra empty crumb.
  function splitBreadcrumbs(p) {
    const raw = typeof p === 'string' ? p : '';
    const segments = raw.split('/').filter(Boolean);
    const crumbs = [{ label: '/', path: '/' }];
    let acc = '';
    segments.forEach((seg) => {
      acc += '/' + seg;
      crumbs.push({ label: seg, path: acc });
    });
    return crumbs;
  }

  // Joins a directory and a name with exactly one slash between them,
  // regardless of whether `dir` already ends in one.
  function joinPath(dir, name) {
    const d = typeof dir === 'string' ? dir : '';
    const n = (typeof name === 'string' ? name : '').replace(/^\/+/, '');
    const trimmedDir = d.replace(/\/+$/, '');
    if (!trimmedDir) return '/' + n;
    return n ? trimmedDir + '/' + n : trimmedDir;
  }

  // '/a/b' -> '/a'; '/a' -> '/'; '/' -> '/'; empty/null -> '/'.
  function parentPath(p) {
    const raw = typeof p === 'string' ? p : '';
    if (!raw || raw === '/') return '/';
    const trimmed = raw.length > 1 ? raw.replace(/\/+$/, '') : raw;
    if (!trimmed || trimmed === '/') return '/';
    const idx = trimmed.lastIndexOf('/');
    if (idx <= 0) return '/';
    return trimmed.slice(0, idx);
  }

  global.termlabFileDialogModel = {
    sortEntries,
    filterEntries,
    splitBreadcrumbs,
    joinPath,
    parentPath,
  };
})(window);
