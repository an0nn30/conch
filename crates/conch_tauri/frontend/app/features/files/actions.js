(function initConchFilesActions(global) {
  'use strict';

  function navigate(pane, path, deps) {
    if (!pane) return;
    const d = deps || {};
    pane.backStack.push(pane.currentPath);
    pane.forwardStack = [];
    pane.currentPath = path;
    pane.pathInput = path;
    if (typeof d.loadEntries === 'function') d.loadEntries(pane);
  }

  function goBack(pane, deps) {
    if (!pane || pane.backStack.length === 0) return;
    const d = deps || {};
    pane.forwardStack.push(pane.currentPath);
    pane.currentPath = pane.backStack.pop();
    pane.pathInput = pane.currentPath;
    if (typeof d.loadEntries === 'function') d.loadEntries(pane);
  }

  function goForward(pane, deps) {
    if (!pane || pane.forwardStack.length === 0) return;
    const d = deps || {};
    pane.backStack.push(pane.currentPath);
    pane.currentPath = pane.forwardStack.pop();
    pane.pathInput = pane.currentPath;
    if (typeof d.loadEntries === 'function') d.loadEntries(pane);
  }

  async function goHome(pane, deps) {
    if (!pane) return;
    const d = deps || {};
    if (pane.isLocal) {
      try {
        const home = typeof d.getHomeDir === 'function' ? await d.getHomeDir() : '/';
        navigate(pane, home, d);
      } catch (_) {
        navigate(pane, '/', d);
      }
      return;
    }
    navigate(pane, '.', d);
  }

  function activateEntry(pane, entry, deps) {
    if (!pane || !entry || !entry.is_dir) return;
    const d = deps || {};
    const sep = '/';
    const base = String(pane.currentPath || '');
    const next = base.endsWith(sep) ? (base + entry.name) : (base + sep + entry.name);
    navigate(pane, next, d);
  }

  global.conchFilesActions = {
    navigate,
    goBack,
    goForward,
    goHome,
    activateEntry,
  };
})(window);
