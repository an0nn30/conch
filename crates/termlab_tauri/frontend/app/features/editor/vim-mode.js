// Optional vim keybindings for the editor ([editor] vim_mode).
//
// Two jobs, and deliberately no more. It hands editor-pane.js the extension
// array for the compartment that holds vim's keymap, and it rebinds the ex
// commands that mean something in an app rather than in a file: `:w` is this
// app's save (which for a remote file means an upload, with its toast), and
// `:q` is this app's tab close (which for a dirty editor means the
// Save/Don't Save/Cancel prompt).
//
// Nothing here reaches for a view, a pane map or a tab. Everything arrives
// through `deps`, because the accessors this needs — savePane, closeTab,
// currentPane — live in three different places, two of them inside
// main-runtime's closure. manager-compose-runtime.js is where all three are in
// scope at once, so that is where registerExCommands is called from.
(function initTermLabVimMode(global) {
  'use strict';

  function toastError(title, body) {
    if (global.toast && typeof global.toast.error === 'function') {
      global.toast.error(title, body);
      return;
    }
    console.error(`${title}: ${body}`);
  }

  // The contents of editor-pane's vim compartment. An empty array is a real
  // answer, not a failure: it is what "vim mode off" looks like, and what a
  // reconfigure back to plain editing dispatches.
  function vimExtensions(enabled) {
    const CM = global.CM6;
    if (!enabled || !CM || typeof CM.vim !== 'function') return [];
    return [CM.vim()];
  }

  // vim runs an ex command inside `cm.operation(...)` (see vim.js's
  // exCommandDispatcher.processCommand), and `:q` ends in a closeTab that
  // destroys the very CodeMirror view the operation belongs to. Deferring to a
  // microtask lets the operation finish first; it also means the ex prompt is
  // gone before the unsaved-changes dialog opens on top of where it was.
  function defer(fn) {
    Promise.resolve().then(fn).catch((error) => {
      console.error('vim ex command failed:', error);
    });
  }

  // Bind `:w`, `:q` and `:wq` to this app's own paths.
  //
  // deps = { savePane(pane), closeTab(tabId), currentPane() }
  //   savePane   — termlabEditorService.savePane; rejects on failure
  //   closeTab   — managerDelegates.closeTab; the GUARDED close. Never swap
  //                this for closePane or a raw view.destroy(): those do not
  //                ask about unsaved work, and there is no second guard behind
  //                them.
  //   currentPane — __termlabPaneAccess.currentPane; the focused pane
  //
  // Returns false when the bundle carries no vim engine, so the caller can
  // tell "not registered" from "registered and did nothing".
  function registerExCommands(deps) {
    const CM = global.CM6;
    const Vim = CM && CM.Vim;
    if (!Vim || typeof Vim.defineEx !== 'function') return false;

    const d = deps || {};

    // Resolved when the command is typed, not when the deferred work runs: by
    // then the close may already have moved focus elsewhere.
    function focusedEditorPane() {
      if (typeof d.currentPane !== 'function') return null;
      const pane = d.currentPane();
      if (!pane || pane.kind !== 'editor' || !pane.view) return null;
      return pane;
    }

    // Returns whether the save succeeded, because `:wq` must not close over a
    // failed one — the same rule editor-service's confirmDirtyPanes applies.
    async function save(pane) {
      if (typeof d.savePane !== 'function') return false;
      try {
        await d.savePane(pane);
        return true;
      } catch (error) {
        toastError('Save Failed', String(error));
        return false;
      }
    }

    async function close(pane) {
      if (typeof d.closeTab !== 'function') return;
      await d.closeTab(pane.tabId);
    }

    // `defineEx(name, prefix, fn)` fills exCommands[name] and
    // commandMap_[prefix], so each pair below makes both the long and the
    // short spelling work, and `write` displaces vim's own stub `:w`.
    Vim.defineEx('write', 'w', () => {
      const pane = focusedEditorPane();
      if (!pane) return;
      defer(() => save(pane));
    });

    Vim.defineEx('quit', 'q', () => {
      const pane = focusedEditorPane();
      if (!pane) return;
      defer(() => close(pane));
    });

    Vim.defineEx('wq', 'wq', () => {
      const pane = focusedEditorPane();
      if (!pane) return;
      defer(async () => {
        if (await save(pane)) await close(pane);
      });
    });

    return true;
  }

  global.termlabVimMode = {
    vimExtensions,
    registerExCommands,
  };
})(window);
