// Breadcrumb path model for the file panes. Pure logic: segments() maps an
// absolute path (POSIX or Windows-drive) to clickable ancestors, collapse()
// folds deep paths into head + hidden-middle + tail so the crumb bar never
// overflows its row. Rendering and click wiring live in pane-view.js.
(function initTermLabFilesBreadcrumbs(global) {
  'use strict';

  function segments(rawPath) {
    const path = typeof rawPath === 'string' ? rawPath : '';
    if (!path) return [];

    const windowsDrive = path.match(/^([A-Za-z]:)(\\|$)/);
    if (windowsDrive) {
      const drive = windowsDrive[1];
      const out = [{ label: drive, path: `${drive}\\` }];
      let current = drive;
      for (const part of path.slice(drive.length).split('\\')) {
        if (!part) continue;
        current = `${current}\\${part}`;
        out.push({ label: part, path: current });
      }
      return out;
    }

    if (path.startsWith('/')) {
      const out = [{ label: '/', path: '/' }];
      let current = '';
      for (const part of path.split('/')) {
        if (!part) continue;
        current = `${current}/${part}`;
        out.push({ label: part, path: current });
      }
      return out;
    }

    // Relative or otherwise unrecognized: one inert segment so the bar still
    // shows where the pane is.
    return [{ label: path, path }];
  }

  function collapse(segs, maxVisible) {
    const list = Array.isArray(segs) ? segs : [];
    if (list.length === 0) return { head: null, hidden: [], tail: [] };
    const visible = Math.max(2, maxVisible || 4);
    const head = list[0];
    const rest = list.slice(1);
    if (list.length <= visible) return { head, hidden: [], tail: rest };
    const tail = rest.slice(rest.length - (visible - 1));
    const hidden = rest.slice(0, rest.length - (visible - 1));
    return { head, hidden, tail };
  }

  global.termlabFilesBreadcrumbs = { segments, collapse };
})(window);
