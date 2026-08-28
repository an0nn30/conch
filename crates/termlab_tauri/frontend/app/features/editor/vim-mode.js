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

  // The vim keys for the IDE features this app actually ships.
  //
  // These have to be vim COMMANDS, not DOM handlers. In normal mode vim's
  // ViewPlugin owns the keystroke, and the Prec.highest handlers the LSP
  // surfaces install deliberately fall through when nothing of theirs is open,
  // so `g` then `d` never reach them as a pair. The engine's own command table
  // is the only correct hook, and the only one that understands multi-key
  // sequences.
  //
  //   gd, gD, <C-]>  go to definition   (nothing in the package binds these)
  //   <C-o>, <C-i>   back / forward     (remapped, see absorbJumpList below)
  //   K              hover              (the package binds no K)
  //   ]d, [d         next/prev problem  (mapCommand unshifts, so these beat
  //                                      the package's ]<character> motion)
  //
  // Everything is normal-mode only, so insert-mode Tab, Ctrl-Space and
  // Ctrl-O ("one normal command") keep their meanings. A key is mapped only
  // when its feature was actually wired, so a window without one gets no dead
  // key.
  //
  // Each action defers to a microtask for the same reason the ex commands do:
  // it runs inside vim's own operation, and the work dispatches transactions
  // into that very view — and may build a whole new tab.
  let registeredNavigation = false;
  function registerNavigationCommands(deps) {
    const CM = global.CM6;
    const Vim = CM && CM.Vim;
    if (
      !Vim || typeof Vim.defineAction !== 'function' || typeof Vim.mapCommand !== 'function'
    ) return false;
    const d = deps || {};
    if (typeof d.goToDefinition !== 'function') return false;
    if (registeredNavigation) return true;
    registeredNavigation = true;

    // The adapter hands an action its CodeMirror 6 view as `cm6` (the CM5
    // adapter calls it `cm`); the caret is read off the view by the feature
    // itself, which is what keeps this seam to one line per key.
    const viewOf = (cm) => (cm && (cm.cm6 || cm.cm)) || null;

    function map(keys, name, run, needsView) {
      Vim.defineAction(name, (cm) => {
        const view = viewOf(cm);
        if (needsView && !view) return;
        defer(() => run(view));
      });
      for (const spelling of keys) {
        Vim.mapCommand(spelling, 'action', name, {}, { context: 'normal' });
      }
    }

    // `gD` (declaration) and `<C-]>` (tag jump) are the same action here: the
    // definition payload folds declaration into definition, and the package
    // binds neither.
    map(['gd', 'gD', '<C-]>'], 'termlabGoToDefinition', d.goToDefinition, true);
    if (typeof d.navigateBack === 'function') {
      map(['<C-o>'], 'termlabJumpBack', () => d.navigateBack(), false);
    }
    if (typeof d.navigateForward === 'function') {
      map(['<C-i>'], 'termlabJumpForward', () => d.navigateForward(), false);
    }
    if (typeof d.showHover === 'function') {
      map(['K'], 'termlabShowHover', d.showHover, true);
    }
    if (typeof d.nextDiagnostic === 'function') {
      map([']d'], 'termlabNextDiagnostic', () => d.nextDiagnostic(), false);
    }
    if (typeof d.previousDiagnostic === 'function') {
      map(['[d'], 'termlabPreviousDiagnostic', () => d.previousDiagnostic(), false);
    }
    if (typeof d.recordJump === 'function') absorbJumpList(Vim, d.recordJump);
    return true;
  }

  // One history, not two.
  //
  // vim keeps its own jumplist, and it cannot serve this app: its entries are
  // CodeMirror BOOKMARKS belonging to one document, so a jump that changed
  // file has nothing to come back to, and walking an entry recorded in another
  // view would put the caret at a line number borrowed from a different file.
  // That is exactly why Ctrl-O did nothing after a cross-file `gd`.
  //
  // So <C-o>/<C-i> consult the window's own cross-file history and ONLY that —
  // no fall-through to vim's native walk, which could never be right here —
  // and this wrapper feeds that history from the same event vim feeds its own
  // list from: `jumpList.add(cm, oldCur, newCur)`, called by every motion the
  // keymap marks `toJumplist` (G, gg, {, }, /search, n/N, marks, %, H/M/L).
  // The engine keeps its own list underneath, untouched and now unread.
  //
  // `getVimGlobalState_` is the package's own accessor for that state. It is
  // labelled a testing hook, so this is guarded end to end: if a future
  // version moves it, the in-file half of the history is lost and the LSP
  // half still works, rather than the editor breaking.
  function absorbJumpList(Vim, recordJump) {
    if (typeof Vim.getVimGlobalState_ !== 'function') return false;
    let jumpList = null;
    try {
      const state = Vim.getVimGlobalState_();
      jumpList = state && state.jumpList;
    } catch (_) {
      return false;
    }
    if (!jumpList || typeof jumpList.add !== 'function') return false;
    // The recorder lives ON the list, and the wrapper reads it at call time.
    // The engine's state object outlives any one registration (it is created
    // when the package is first imported), so a second window — or a reload —
    // must be able to take the recording over rather than wrap a wrapper and
    // leave the first, now-dead recorder in the chain.
    jumpList.termlabRecordJump = recordJump;
    if (jumpList.termlabAbsorbed) return true;
    const original = jumpList.add;
    jumpList.termlabAbsorbed = true;
    jumpList.add = function absorbedAdd(cm, oldCur) {
      try {
        const view = cm && (cm.cm6 || cm.cm);
        const record = jumpList.termlabRecordJump;
        // vim counts columns in `ch`; the history speaks LSP characters, which
        // is the same unit under a different name.
        if (view && oldCur && typeof record === 'function') {
          record(view, { line: oldCur.line, character: oldCur.ch });
        }
      } catch (error) {
        console.error('vim jump was not recorded in the navigation history', error);
      }
      return original.apply(this, arguments);
    };
    return true;
  }

  global.termlabVimMode = {
    vimExtensions,
    registerExCommands,
    registerNavigationCommands,
  };
})(window);
