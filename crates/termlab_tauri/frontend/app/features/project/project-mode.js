// This window's project, resolved once and read by every project surface.
//
// The adoption happens in startup-runtime.js during applyAppConfig — BEFORE
// the layout read and before the tool-window runtime registers anything — so
// the per-project layout is in effect on first paint and the Search tool
// window knows at registration time whether it belongs in this window.
//
// `isUnderRoot` compares on path boundaries rather than string prefixes: a
// sibling directory whose name merely starts with the project's ("/repository"
// beside "/repo") is a different project, and treating it as inside would
// silently attach the wrong LSP root to its files.
(function initTermLabProjectMode(global) {
  'use strict';

  let project = null;
  // Set by adopt() whenever it returns null. True only for the BENIGN case —
  // project_adopt_pending found another window already holding the same
  // root, focused it, and destroyed this one. Read by startup-runtime.js to
  // decide whether a failed adopt is worth a toast: a benign hand-off needs
  // no explanation (this window is already on its way out), but a real
  // failure (folder vanished mid-boot, permission denied, backend error)
  // must not boot a plain terminal with no user-visible reason why the
  // project didn't open.
  let lastAdoptFocusedExisting = false;

  function set(info) {
    if (!info || !info.root) {
      project = null;
      return null;
    }
    project = { root: String(info.root), name: String(info.name || '') };
    global.__termlabProject = { root: project.root, name: project.name };
    global.__termlabProjectName = project.name;
    return project;
  }

  function reset() {
    project = null;
    delete global.__termlabProject;
    delete global.__termlabProjectName;
  }

  // Never fatal: a backend that cannot answer leaves the window an ordinary
  // terminal window rather than failing its boot.
  async function adopt(invoke) {
    lastAdoptFocusedExisting = false;
    try {
      const result = await invoke('project_adopt_pending');
      if (!result || !result.adopted) {
        lastAdoptFocusedExisting = !!(result && result.focusedExisting);
        return null;
      }
      return set(result.adopted);
    } catch (error) {
      console.warn('project-mode: could not adopt a pending project', error);
      return null;
    }
  }

  // See lastAdoptFocusedExisting's comment above. Reflects only the most
  // recent adopt() call.
  function adoptFocusedExisting() {
    return lastAdoptFocusedExisting;
  }

  function root() {
    return project ? project.root : null;
  }

  function name() {
    return project ? project.name : null;
  }

  function isActive() {
    return project !== null;
  }

  function isUnderRoot(filePath) {
    if (!project || !filePath) return false;
    const target = String(filePath);
    if (target === project.root) return true;
    const prefix = project.root.endsWith('/') ? project.root : project.root + '/';
    return target.startsWith(prefix);
  }

  global.termlabProjectMode = {
    adopt, set, reset, root, name, isActive, isUnderRoot, adoptFocusedExisting,
  };
})(window);
