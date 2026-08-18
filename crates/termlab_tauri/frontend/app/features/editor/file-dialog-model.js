// File dialog model: sorting, filtering, breadcrumbs, and path arithmetic
// for the unified local/remote file-open and save-as dialog. Pure — no DOM,
// no invokes — so it is testable without a running app. `file-dialog.js`
// is the only thing that renders this.
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

  // Field comparator for two entries already known to be in the same
  // dir/file group. `direction` only affects this comparison — never the
  // dir-first partition or null placement, both of which are absolute.
  function compareByKey(key, direction, a, b) {
    const dir = direction === 'desc' ? -1 : 1;
    if (key === 'size') {
      const aSize = Number(a && a.size);
      const bSize = Number(b && b.size);
      const aVal = Number.isFinite(aSize) ? aSize : 0;
      const bVal = Number.isFinite(bSize) ? bSize : 0;
      if (aVal < bVal) return -1 * dir;
      if (aVal > bVal) return 1 * dir;
      return 0;
    }
    if (key === 'modified') {
      const aMod = a && a.modified;
      const bMod = b && b.modified;
      const aNull = aMod == null;
      const bNull = bMod == null;
      // Null modified times sort last regardless of direction — this is
      // NOT multiplied by `dir`.
      if (aNull && bNull) return 0;
      if (aNull) return 1;
      if (bNull) return -1;
      const aVal = Number(aMod);
      const bVal = Number(bMod);
      if (aVal < bVal) return -1 * dir;
      if (aVal > bVal) return 1 * dir;
      return 0;
    }
    // 'name' (also the fallback for any unrecognized key).
    const aName = String((a && a.name) || '').toLowerCase();
    const bName = String((b && b.name) || '').toLowerCase();
    if (aName < bName) return -1 * dir;
    if (aName > bName) return 1 * dir;
    return 0;
  }

  // Directories always precede files, regardless of `key`/`direction` — that
  // partition is absolute, not just another sort key. Within each group,
  // entries order by `key` ('name' | 'size' | 'modified'; unrecognized
  // values fall back to 'name') and `direction` ('asc' | 'desc'). Modified
  // is numeric with null entries always last, in both directions. Stable:
  // equal-key entries keep their original relative order (enforced via an
  // explicit index tiebreaker rather than relying on engine sort stability).
  // Returns a new array; never mutates the input or its entries. A no-arg
  // call (`sortEntries(entries)`) is identical to `sortEntries(entries,
  // 'name', 'asc')` — today's behavior, preserved for existing callers.
  function sortEntries(entries, key = 'name', direction = 'asc') {
    const list = Array.isArray(entries) ? entries.slice() : [];
    const indexed = list.map((item, index) => ({ item, index }));
    indexed.sort((a, b) => {
      const aDir = !!(a.item && a.item.is_dir);
      const bDir = !!(b.item && b.item.is_dir);
      if (aDir !== bDir) return aDir ? -1 : 1;
      const cmp = compareByKey(key, direction, a.item, b.item);
      if (cmp !== 0) return cmp;
      return a.index - b.index;
    });
    return indexed.map((entry) => entry.item);
  }

  // `B`, `KB`, `MB`, `GB`; one decimal below 10 units of the chosen unit,
  // none at 10 units and above. NEVER returns '—' — the dir case ('—' for
  // size) is the caller's job, not this function's. Never throws.
  function formatSize(bytes) {
    const n = Number(bytes);
    const start = Number.isFinite(n) && n > 0 ? n : 0;
    const units = ['B', 'KB', 'MB', 'GB'];
    let unitIndex = 0;
    let value = start;
    while (value >= 1024 && unitIndex < units.length - 1) {
      value /= 1024;
      unitIndex += 1;
    }
    if (unitIndex === 0) {
      return Math.round(value) + ' B';
    }
    let formatted = value < 10 ? value.toFixed(1) : String(Math.round(value));
    // `toFixed` can round a value like 9.96 up to "10.0" — re-render
    // without a decimal so "one decimal below 10, none at 10+" still holds
    // after rounding, not just before it.
    if (formatted.indexOf('.') !== -1 && parseFloat(formatted) >= 10) {
      formatted = String(Math.round(value));
    }
    return formatted + ' ' + units[unitIndex];
  }

  const MONTH_ABBR = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];

  function pad2(n) {
    return n < 10 ? '0' + n : String(n);
  }

  // Local-time midnight (as epoch milliseconds) for the calendar day
  // containing `epochSeconds`. Used only to diff calendar days, never
  // exposed.
  function localDayStartMs(epochSeconds) {
    const d = new Date(epochSeconds * 1000);
    return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  }

  // 'Today' / 'Yesterday' / 'Mon D' (within `now`'s calendar year) /
  // 'YYYY-MM-DD' (otherwise). `now` is INJECTED (`nowEpochSeconds`) — this
  // function never reads the clock itself, so it is deterministic and
  // testable. Comparisons are calendar-day boundaries in LOCAL time, not
  // elapsed-hours windows: 23:59 yesterday is "Yesterday" even one minute
  // before "now", and 00:01 today is "Today" even though nearly a full day
  // separates them. null/undefined `epochSeconds` (or any non-finite value)
  // renders as '—'. Never throws.
  function formatModified(epochSeconds, nowEpochSeconds) {
    const n = Number(epochSeconds);
    if (epochSeconds == null || !Number.isFinite(n)) return '—';
    const now = Number(nowEpochSeconds);
    const nowSafe = Number.isFinite(now) ? now : 0;

    const dayStartMs = localDayStartMs(n);
    const nowDayStartMs = localDayStartMs(nowSafe);
    const diffDays = Math.round((nowDayStartMs - dayStartMs) / 86400000);

    if (diffDays === 0) return 'Today';
    if (diffDays === 1) return 'Yesterday';

    const d = new Date(n * 1000);
    const nowDate = new Date(nowSafe * 1000);
    if (d.getFullYear() === nowDate.getFullYear()) {
      return MONTH_ABBR[d.getMonth()] + ' ' + d.getDate();
    }
    return d.getFullYear() + '-' + pad2(d.getMonth() + 1) + '-' + pad2(d.getDate());
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
    formatSize,
    formatModified,
  };
})(window);
