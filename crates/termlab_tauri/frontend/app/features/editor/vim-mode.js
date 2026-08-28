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
  // answer for "vim mode off" — but enabled-with-no-engine is a FAILURE and
  // must say so. The vendor bundle is generated and git-ignored, so a
  // checkout can carry a stale bundle whose CM6 predates the vim export;
  // returning [] silently there made the Settings toggle look broken with
  // nothing anywhere saying why (it shipped that way and cost a bug report).
  // Same class as the "editor bundle missing" toast in editor-service.js.
  let warnedMissingEngine = false;
  function vimExtensions(enabled) {
    const CM = global.CM6;
    if (!enabled) return [];
    if (!CM || typeof CM.vim !== 'function') {
      if (!warnedMissingEngine) {
        warnedMissingEngine = true;
        console.error('vim mode is enabled but the vendor bundle has no vim engine — rebuild it: npm run build:vendor in crates/termlab_tauri/frontend');
        if (global.toast) {
          global.toast.error(
            'Vim mode unavailable',
            'The editor bundle is stale. Run "npm run build:vendor" in crates/termlab_tauri/frontend and relaunch.',
          );
        }
      }
      return [];
    }
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
        // `:w` on an untitled buffer opens the Save As chooser (savePane's
        // diversion). Cancelling it rejects with this sentinel: still false,
        // so `:wq` keeps the tab rather than closing over unsaved text, but
        // silent — the user cancelled, they do not need to be told.
        if (error && error.name === 'SaveCancelled') return false;
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

  // `gd` / `gD` — Go to Definition, the way a vim user actually asks for it.
  //
  // This has to be a vim COMMAND, not a DOM handler. In normal mode vim's
  // ViewPlugin owns the keystroke, and the Prec.highest handlers the LSP
  // surfaces install deliberately fall through when nothing of theirs is open,
  // so `g` and `d` never reach them as a pair. The engine's own command table
  // is the only correct hook, and it is also the only one that understands
  // that `gd` is two keys.
  //
  // Nothing is taken away: @replit/codemirror-vim binds gg/gj/gk/ge/gE/gi/gI/
  // gv/gu/gU/gn/gN/gq/gw/gc/gJ/g~/g?/g*/g# and neither `gd` nor `gD`. `gD`
  // (declaration in vim) maps to the same action because the LSP payload folds
  // declaration into definition.
  //
  // The action runs inside vim's own operation, and the jump dispatches
  // transactions into this very view — and may build a whole new tab — so it
  // is deferred to a microtask exactly as the ex commands are.
  let registeredNavigation = false;
  function registerNavigationCommands(deps) {
    const CM = global.CM6;
    const Vim = CM && CM.Vim;
    if (
      !Vim || typeof Vim.defineAction !== 'function' || typeof Vim.mapCommand !== 'function'
    ) return false;
    const goToDefinition = deps && typeof deps.goToDefinition === 'function'
      ? deps.goToDefinition
      : null;
    if (!goToDefinition) return false;
    if (registeredNavigation) return true;
    registeredNavigation = true;
    // The adapter hands the action its CodeMirror 6 view as `cm6` (the CM5
    // adapter calls it `cm`); the caret is read off the view by the navigator
    // itself, which is what keeps this seam to one line.
    Vim.defineAction('termlabGoToDefinition', (cm) => {
      const view = cm && (cm.cm6 || cm.cm || null);
      if (!view) return;
      defer(() => goToDefinition(view));
    });
    // `<C-]>` is vim's tag-jump idiom and the package binds only `<C-t>`, so
    // it comes along for free through the same mechanism.
    for (const keys of ['gd', 'gD', '<C-]>']) {
      Vim.mapCommand(keys, 'action', 'termlabGoToDefinition', {}, { context: 'normal' });
    }
    return true;
  }

  global.termlabVimMode = {
    vimExtensions,
    registerExCommands,
    registerNavigationCommands,
  };
})(window);
