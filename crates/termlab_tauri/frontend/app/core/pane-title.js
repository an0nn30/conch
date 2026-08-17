// Tab and window title composition.
//
// Produces the shape iTerm and macOS Terminal use — "dustin@mbp: ~/projects/conch
// — 120×30" — from a snapshot of a pane's state. Pure string work with no DOM and
// no I/O, so the rules live here and are covered by scripts/tests/test_pane_title.mjs
// rather than being spread through tab-manager.js.
//
// Precedence for the body: an explicit OSC title from the shell or a program
// wins (that is a deliberate statement about what the pane is doing), then the
// foreground program, then the working directory.
(function (exports) {
  'use strict';

  const NBSP_DASH = ' — ';

  /**
   * Replace a leading home directory with `~`.
   *
   * Matches on a path-segment boundary so `/Users/dustinson` is not reported as
   * `~son` for user `dustin`.
   */
  function collapseHome(cwd, home) {
    const p = String(cwd == null ? '' : cwd);
    const h = String(home == null ? '' : home).replace(/\/+$/, '');
    if (!p || !h) return p;
    if (p === h) return '~';
    if (p.startsWith(h + '/')) return '~' + p.slice(h.length);
    return p;
  }

  /**
   * Compose a pane title.
   *
   * state: { user, host, home, cwd, program, oscTitle, cols, rows }
   * Every field is optional; the title degrades rather than showing empty
   * separators, and falls back to "Terminal" when nothing is known.
   */
  function composeTitle(state) {
    const s = state || {};
    const user = String(s.user || '').trim();
    const host = String(s.host || '').trim();
    const osc = String(s.oscTitle || '').trim();
    const program = String(s.program || '').trim();
    const cols = Number(s.cols) || 0;
    const rows = Number(s.rows) || 0;

    // A prefix needs both halves: "dustin@" or "@mbp" alone is noise.
    const prefix = user && host ? user + '@' + host : '';

    let body;
    if (osc) body = osc;
    else if (program) body = program;
    else body = collapseHome(s.cwd, s.home);

    // Shells commonly emit "user@host: dir" themselves; prefixing that again
    // gives "dustin@mbp: dustin@mbp: ~". Drop our prefix when the title already
    // opens with it.
    const bodyRepeatsPrefix = !!(prefix && body
      && body.toLowerCase().startsWith(prefix.toLowerCase()));

    let head;
    if (bodyRepeatsPrefix) head = body;
    else if (prefix && body) head = prefix + ': ' + body;
    else head = prefix || body;

    if (!head) head = 'Terminal';

    // Size only once the terminal has actually been measured — a pane that has
    // not been fitted yet reports 0×0, which is worse than saying nothing.
    const size = cols > 0 && rows > 0 ? cols + '×' + rows : '';
    return size ? head + NBSP_DASH + size : head;
  }

  exports.termlabPaneTitle = { composeTitle, collapseHome };
})(window);
