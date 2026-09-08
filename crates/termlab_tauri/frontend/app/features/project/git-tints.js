// Git status for the project tree: the flat snapshot Rust produces, plus the
// directory rollup the tree needs.
//
// The rollup is computed here rather than in Rust because it is a pure
// function of a snapshot the tree already has, and because "which directories
// are currently on screen" is a frontend question. It matches on path
// SEGMENTS, never string prefixes: "srcfoo/a.rs" is not inside "src", and a
// prefix test would tint half the tree the first time someone names two
// directories that way.
//
// Snapshots are replace-only — never merged — so a file that stopped being
// modified simply stops appearing, with no stale tint left behind.
(function initTermLabProjectGit(global) {
  'use strict';

  const DEFAULT_INTERVAL_MS = 10000;

  // The repo-relative path, or null when the file is not inside the root.
  function relativeTo(root, absolutePath) {
    if (!root || !absolutePath) return null;
    const rootStr = String(root);
    const target = String(absolutePath);
    if (target === rootStr) return '';
    const prefix = rootStr.endsWith('/') ? rootStr : rootStr + '/';
    if (!target.startsWith(prefix)) return null;
    return target.slice(prefix.length);
  }

  function stateForPath(snapshot, root, absolutePath, isDir) {
    if (!snapshot || snapshot.available !== true || !snapshot.files) return null;
    const relative = relativeTo(root, absolutePath);
    if (relative === null) return null;
    const files = snapshot.files;
    if (!isDir) {
      return Object.prototype.hasOwnProperty.call(files, relative) ? files[relative] : null;
    }
    // A folder shows the modified tint when anything beneath it has a state —
    // one tint, not six, because a folder has no single state of its own.
    const prefix = relative === '' ? '' : relative + '/';
    for (const key of Object.keys(files)) {
      if (prefix === '' || key.startsWith(prefix)) return 'modified';
    }
    return null;
  }

  // Refresh triggers: window focus, an editor save in this window, and a
  // timer while the panel is visible in project mode. There is no filesystem
  // watcher in v1, so these three are the whole freshness story.
  //
  // `isVisible` gates the TIMER only. Focus and save are user acts and always
  // refresh; a ticking clock against a hidden panel is pure waste — a git
  // process every ten seconds for a status nobody is looking at.
  function startPolling(options) {
    const opts = options || {};
    const invoke = opts.invoke;
    const getTree = opts.getTree;
    const target = opts.target || global;
    const isVisible = typeof opts.isVisible === 'function' ? opts.isVisible : () => true;
    const intervalMs = Number(opts.intervalMs) > 0 ? Number(opts.intervalMs) : DEFAULT_INTERVAL_MS;
    let stopped = false;

    function refresh() {
      if (stopped) return;
      Promise.resolve(invoke('project_git_status'))
        .then((snapshot) => {
          if (stopped) return;
          const tree = typeof getTree === 'function' ? getTree() : null;
          if (tree && typeof tree.setGitStatus === 'function') tree.setGitStatus(snapshot);
        })
        // Silently off: git being absent, or the project not being a
        // repository, is completely ordinary and must never toast.
        .catch(() => {});
    }

    const onFocus = () => refresh();
    const onSaved = () => refresh();
    target.addEventListener('focus', onFocus);
    target.addEventListener('termlab:editor-saved', onSaved);
    const timer = setInterval(() => {
      if (isVisible()) refresh();
    }, intervalMs);
    refresh();

    return function stop() {
      stopped = true;
      clearInterval(timer);
      target.removeEventListener('focus', onFocus);
      target.removeEventListener('termlab:editor-saved', onSaved);
    };
  }

  global.termlabProjectGit = { relativeTo, stateForPath, startPolling };
})(window);
