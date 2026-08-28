// Routes CLI/second-instance "open this path" requests to the editor.
//
// Rust queues arrived paths per-window (`take_pending_open_paths`, drained
// exactly once per call — the second read of an already-drained queue comes
// back empty) rather than pushing an event, so a window that has not
// finished booting yet does not lose them: they sit in the queue until this
// module asks. Each path is `local_stat`-ed to tell a file from a directory
// from something that no longer exists, then routed:
//   - a regular file opens in the editor
//   - a directory is opened as a project (project_open focuses a window that
//     already holds the same canonical root, else creates one)
//   - a path that fails to stat (typically: gone before we got to it) is
//     reported by name rather than silently dropped
//
// Routing is sequential (`for...of` with `await`) rather than
// `Promise.all`-fanned so toasts land in queue order, and each path is
// wrapped in its own try/catch so one bad path can never block the rest of
// the queue from opening.
(function initTermLabOpenPathRouting(global) {
  'use strict';

  function create(deps) {
    const invoke = deps.invoke;
    const openLocalFile = deps.openLocalFile;
    const openProject = deps.openProject;
    const toastError = deps.toastError;
    const toastInfo = deps.toastInfo;

    // True only when a regular file actually reached the editor: the boot
    // path counts these to decide whether the window earned its editor-only
    // layout or must fall back to a terminal tab.
    async function routeOne(pathStr) {
      let entry;
      try {
        entry = await invoke('local_stat', { path: pathStr });
      } catch (error) {
        toastError('Cannot Open Path', pathStr + ': ' + String(error));
        return false;
      }
      if (entry && entry.is_dir) {
        // A directory is a PROJECT, and a project owns a window: this returns
        // false because no editor opened here, which is what the boot path
        // counts. Reported by name on failure rather than dropped — a folder
        // that vanished between the stat and the open is exactly the case a
        // silent return would hide.
        try {
          await openProject(pathStr);
        } catch (error) {
          toastError('Cannot Open Folder', pathStr + ': ' + String(error));
        }
        return false;
      }
      openLocalFile(pathStr);
      return true;
    }

    // Route an already-pulled list (the boot path takes the queue itself,
    // early, to decide the window layout before any tab exists). Returns the
    // number of files opened.
    async function routePaths(paths) {
      let opened = 0;
      for (const pathStr of paths || []) {
        if (await routeOne(pathStr)) opened += 1;
      }
      return opened;
    }

    async function drainPendingOpens() {
      const pending = await invoke('take_pending_open_paths');
      if (!pending || !pending.length) return;
      await routePaths(pending);
    }

    return { drainPendingOpens, routePaths };
  }

  global.termlabOpenPathRouting = { create };
})(window);
