// Routes CLI/second-instance "open this path" requests to the editor.
//
// Rust queues arrived paths per-window (`take_pending_open_paths`, drained
// exactly once per call — the second read of an already-drained queue comes
// back empty) rather than pushing an event, so a window that has not
// finished booting yet does not lose them: they sit in the queue until this
// module asks. Each path is `local_stat`-ed to tell a file from a directory
// from something that no longer exists, then routed:
//   - a regular file opens in the editor
//   - a directory is not supported yet — DIRECTORY_COMING_SOON is the one
//     place that copy lives, so the later LSP/workspace-open branch has a
//     single seam to replace
//   - a path that fails to stat (typically: gone before we got to it) is
//     reported by name rather than silently dropped
//
// Routing is sequential (`for...of` with `await`) rather than
// `Promise.all`-fanned so toasts land in queue order, and each path is
// wrapped in its own try/catch so one bad path can never block the rest of
// the queue from opening.
(function initTermLabOpenPathRouting(global) {
  'use strict';

  const DIRECTORY_COMING_SOON = 'Opening a folder from the command line is not supported yet — open it from the Files panel instead.';

  function create(deps) {
    const invoke = deps.invoke;
    const openLocalFile = deps.openLocalFile;
    const toastError = deps.toastError;
    const toastInfo = deps.toastInfo;

    async function routeOne(pathStr) {
      let entry;
      try {
        entry = await invoke('local_stat', { path: pathStr });
      } catch (error) {
        toastError('Cannot Open Path', pathStr + ': ' + String(error));
        return;
      }
      if (entry && entry.is_dir) {
        toastInfo('Folder', DIRECTORY_COMING_SOON);
        return;
      }
      openLocalFile(pathStr);
    }

    async function drainPendingOpens() {
      const pending = await invoke('take_pending_open_paths');
      if (!pending || !pending.length) return;
      for (const pathStr of pending) {
        await routeOne(pathStr);
      }
    }

    return { drainPendingOpens };
  }

  global.termlabOpenPathRouting = { create, DIRECTORY_COMING_SOON };
})(window);
