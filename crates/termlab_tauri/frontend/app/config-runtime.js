(function initTermLabConfigRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const refreshKeyboardShortcutFallbacks = deps.refreshKeyboardShortcutFallbacks;
    const getPanes = deps.getPanes;
    const setTheme = deps.setTheme;
    const getFontFallbacks = deps.getFontFallbacks;
    const setTermFontFamily = deps.setTermFontFamily;
    const setTermFontSize = deps.setTermFontSize;
    const setEditorVimMode = deps.setEditorVimMode;
    const configService = global.termlabConfigService || {};

    // The RESOLVED app appearance ('dark' | 'light'), which is the argument
    // `get_theme_colors` needs to answer the reserved `auto` theme name.
    // Rust cannot resolve it: `system` lives in matchMedia inside the
    // webview. appearance.js's current() never returns 'system' — it returns
    // the resolved value — and defaults to 'dark' before the first apply(),
    // which is also the Rust-side default for a missing argument.
    function resolvedAppearance() {
      if (global.termlabAppearance && typeof global.termlabAppearance.current === 'function') {
        return global.termlabAppearance.current();
      }
      return 'dark';
    }

    // The theme half of applyConfigChanged, extracted so the appearance-flip
    // path can reuse the SAME pane walk and the same guards instead of
    // growing a second, drifting copy. Both callers below go through here;
    // nothing else applies a terminal palette.
    function applyThemeColors(tc) {
      if (typeof configService.applyThemeCss === 'function') {
        configService.applyThemeCss(tc);
      }

      const fallbackTheme = {
        background: tc.background, foreground: tc.foreground,
        cursor: tc.cursor_color, cursorAccent: tc.cursor_text,
        selectionBackground: tc.selection_bg, selectionForeground: tc.selection_text,
        black: tc.black, red: tc.red, green: tc.green, yellow: tc.yellow,
        blue: tc.blue, magenta: tc.magenta, cyan: tc.cyan, white: tc.white,
        brightBlack: tc.bright_black, brightRed: tc.bright_red,
        brightGreen: tc.bright_green, brightYellow: tc.bright_yellow,
        brightBlue: tc.bright_blue, brightMagenta: tc.bright_magenta,
        brightCyan: tc.bright_cyan, brightWhite: tc.bright_white,
      };
      const newTheme = typeof configService.toTerminalTheme === 'function'
        ? configService.toTerminalTheme(tc, fallbackTheme)
        : fallbackTheme;
      setTheme(newTheme);
      for (const pane of getPanes().values()) {
        if (pane.kind === 'terminal' && pane.term) {
          pane.term.options.theme = newTheme;
        }
        // The editor's colours come from the --tl-* variables, which
        // applyThemeCss above has just rewritten, so a rebuild is all that is
        // needed. Font size is not available at this site; it is applied in
        // the terminal-config block below, which is the only place the new
        // size exists.
        if (pane.kind === 'editor' && pane.view && global.termlabEditorPane) {
          global.termlabEditorPane.refreshTheme(pane.view);
        }
      }
      return newTheme;
    }

    // F2 (branch-review.md): a monotonic token, incremented per call and
    // captured in each call's own closure — the deleted theme-preview
    // stopgap's `previewSeq` pattern (settings.js, pre-terminal-themes).
    // Two in-flight get_theme_colors fetches (possible when an OS System
    // flip lands between the appearance-listener fetch and
    // applyConfigChanged's own fetch) can resolve in EITHER order if their
    // IPC responses reorder; without a guard, whichever resolves LAST wins
    // regardless of which was issued last, so a stale palette could
    // overwrite a fresher one that already landed. The token makes it
    // last-INITIATED-wins instead: a fetch whose token has been superseded
    // by a newer call is dropped rather than applied.
    let themeColorsFetchToken = 0;

    // Fetch the palette for the CURRENT resolved appearance and apply it.
    async function refetchThemeColors() {
      const token = ++themeColorsFetchToken;
      const tc = await invoke('get_theme_colors', { resolvedAppearance: resolvedAppearance() });
      if (token !== themeColorsFetchToken) {
        // A newer refetch was issued while this one was in flight; that one
        // owns the outcome (whether it has already applied or is still
        // pending), so drop this stale result rather than overwrite it.
        return undefined;
      }
      return applyThemeColors(tc);
    }

    async function applyConfigChanged() {
      try {
        await refreshKeyboardShortcutFallbacks();

        // Fetched here (rather than at its old call site further down) so
        // the appearance re-apply lands BEFORE applyThemeCss: both the
        // token-CSS switch and the terminal accent vars it sets then land in
        // the same repaint frame instead of two. appCfg is reused below for
        // vim mode and applyUiConfig, so this is still exactly one
        // get_app_config round trip per config-changed event, same as before.
        //
        // It gets its OWN try so that moving it up cannot make an appearance
        // failure swallow the theme: before the move, a rejecting
        // get_app_config aborted the handler only AFTER applyThemeCss and the
        // pane refresh had already run. Every block that genuinely needs
        // appCfg is guarded on it below, so a failure here costs exactly what
        // it used to — the appearance re-apply, vim mode and applyUiConfig —
        // and nothing more.
        let appCfg = null;
        try {
          appCfg = await invoke('get_app_config');
          if (global.termlabAppearance && typeof global.termlabAppearance.apply === 'function') {
            global.termlabAppearance.apply(appCfg && appCfg.appearance_mode);
          }
        } catch (error) {
          console.warn('Failed to reload app config:', error);
        }

        // The appearance re-apply above has already run, so the resolved
        // appearance this fetch carries is the NEW one, not the outgoing one.
        await refetchThemeColors();

        // appCfg was already fetched above (ahead of applyThemeCss, for the
        // appearance re-apply) and is reused here rather than fetched again.
        // Vim keybindings. Applied here, in the same event that carries the
        // theme and the font, because this is the only place a settings save
        // reaches an already-open window: save_settings writes the config and
        // emits `config-changed` (settings.rs), and this listener is what turns
        // that into live UI. A reconfigure of the vim compartment keeps the
        // document, the selection and the undo history, so the toggle takes
        // effect in an open editor without reopening the file.
        //
        // The stored flag is updated too, so the next editor pane this window
        // opens is created with the new value instead of the startup one.
        //
        // Guarded on appCfg: if its fetch above failed there is no new vim
        // flag and no new UI config to apply, and forcing vim off from a
        // missing config would be a worse answer than leaving it alone.
        if (appCfg) {
          if (typeof appCfg.editor_lsp_suggestions_while_typing === 'boolean'
              && global.termlabLspCompletion
              && typeof global.termlabLspCompletion.setSuggestionsWhileTyping === 'function') {
            global.termlabLspCompletion.setSuggestionsWhileTyping(
              appCfg.editor_lsp_suggestions_while_typing,
            );
          }
          const vimMode = appCfg.editor_vim_mode === true;
          if (typeof setEditorVimMode === 'function') setEditorVimMode(vimMode);
          for (const pane of getPanes().values()) {
            if (pane.kind === 'editor' && pane.view && global.termlabEditorPane
                && typeof global.termlabEditorPane.setVimMode === 'function') {
              global.termlabEditorPane.setVimMode(pane.view, vimMode);
            }
          }
          if (typeof configService.applyUiConfig === 'function') {
            configService.applyUiConfig(appCfg);
          } else {
            document.documentElement.classList.toggle('no-animations', appCfg.disable_animations === true);
          }
        }

        try {
          const termCfg = await invoke('get_terminal_config');
          let newTermFont = '"JetBrains Mono", "Fira Code", "Cascadia Code"' + getFontFallbacks();
          if (termCfg.font_family) {
            newTermFont = '"' + termCfg.font_family + '", "Fira Code", "Cascadia Code"' + getFontFallbacks();
          }
          setTermFontFamily(newTermFont);
          const newTermSize = termCfg.font_size > 0 ? termCfg.font_size : 14;
          setTermFontSize(newTermSize);
          for (const pane of getPanes().values()) {
            if (pane.kind === 'terminal' && pane.term) {
              pane.term.options.fontFamily = newTermFont;
              pane.term.options.fontSize = newTermSize;
            }
            if (pane.kind === 'editor' && pane.view && global.termlabEditorPane) {
              global.termlabEditorPane.setFontSize(pane.view, newTermSize);
              // setTermFontFamily (above) has already updated
              // window.__termlabTermFontFamily, so rebuilding the theme here
              // picks up the new font family. setFontSize only reconfigures
              // the size compartment — it does not touch the theme
              // compartment that holds fontFamily.
              global.termlabEditorPane.refreshTheme(pane.view);
            }
          }
          // The rAF pass below is a re-fit/re-paint for xterm only: CodeMirror
          // reflows itself when the font compartment is reconfigured, and an
          // editor pane has no fitAddon and no term to refresh. Deliberately
          // left terminal-only.
          requestAnimationFrame(() => {
            for (const pane of getPanes().values()) {
              if (pane.kind !== 'terminal' || !pane.term) continue;
              if (pane.fitAddon) pane.fitAddon.fit();
              pane.term.refresh(0, pane.term.rows - 1);
            }
          });
        } catch (error) {
          console.warn('Failed to reload terminal font:', error);
        }

      } catch (error) {
        console.warn('Config reload failed:', error);
      }
    }

    // The editor-pane half of applyConfigChanged's walk, extracted so the
    // appearance-change path can reuse the same iteration and the same
    // guards. An editor's colours are baked into a CodeMirror theme at
    // buildTheme() time, so a --tl-* change only reaches an open pane through
    // a rebuild.
    function refreshEditorThemes() {
      if (!global.termlabEditorPane || typeof global.termlabEditorPane.refreshTheme !== 'function') return;
      for (const pane of getPanes().values()) {
        if (pane.kind === 'editor' && pane.view) {
          global.termlabEditorPane.refreshTheme(pane.view);
        }
      }
    }

    // What an appearance flip does: re-fetch the palette for the newly
    // resolved appearance and apply it to every pane.
    //
    // Under the default `auto` theme this is what makes terminals follow the
    // app: Rust picks TermLab Dark or TermLab Light from the appearance this
    // fetch carries. Under a concrete theme name (`gruvbox_dark`, …) the
    // fetch still happens but the payload is identical either way — the
    // resolution is decoupled by design, and that invariance is pinned in
    // Rust by theme.rs's
    // a_concrete_theme_name_yields_an_identical_payload_under_both_appearances.
    //
    // The failure path falls back to the editor-only refresh this listener
    // used to do on its own, so a rejected fetch cannot leave open editors on
    // the outgoing appearance's colours.
    function handleAppearanceChanged() {
      return refetchThemeColors().catch((error) => {
        console.warn('Failed to re-theme after an appearance change:', error);
        refreshEditorThemes();
      });
    }

    function init() {
      listenOnCurrentWindow('config-changed', () => {
        applyConfigChanged();
      });

      // Appearance flips reach open terminals and editors here. Two paths
      // emit the event: a settings save (which also runs applyConfigChanged
      // above, whose own fetch — issued after the same appearance apply — is
      // the authoritative one; both land on the same palette, so the extra
      // pass costs one fetch and changes nothing), and an OS light/dark flip
      // while in 'system' mode, which emits NO config-changed at all and is
      // the reason this listener has to exist separately.
      const appearanceEvent = (global.termlabAppearance && global.termlabAppearance.CHANGED_EVENT)
        || 'tl-appearance-changed';
      if (global.document && typeof global.document.addEventListener === 'function') {
        global.document.addEventListener(appearanceEvent, () => {
          // Kept so a test (and only a test) can await the re-theme the DOM
          // event kicked off; nothing in the app reads it.
          pendingAppearanceRetheme = handleAppearanceChanged();
        });
      }

      return {
        applyConfigChanged,
      };
    }

    let pendingAppearanceRetheme = Promise.resolve();

    return {
      init,
      applyConfigChanged,
      refreshEditorThemes,
      handleAppearanceChanged,
      appearanceSettled: () => pendingAppearanceRetheme,
    };
  }

  global.termlabConfigRuntime = {
    create,
  };
})(window);
