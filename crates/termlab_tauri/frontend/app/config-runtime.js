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

    async function applyConfigChanged() {
      try {
        await refreshKeyboardShortcutFallbacks();
        const tc = await invoke('get_theme_colors');
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

        const appCfg = await invoke('get_app_config');
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
        const vimMode = appCfg && appCfg.editor_vim_mode === true;
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

    function init() {
      listenOnCurrentWindow('config-changed', () => {
        applyConfigChanged();
      });
      return {
        applyConfigChanged,
      };
    }

    return {
      init,
      applyConfigChanged,
    };
  }

  global.termlabConfigRuntime = {
    create,
  };
})(window);
