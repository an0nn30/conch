(function initConchConfigRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const refreshKeyboardShortcutFallbacks = deps.refreshKeyboardShortcutFallbacks;
    const getPanes = deps.getPanes;
    const setTheme = deps.setTheme;
    const getFontFallbacks = deps.getFontFallbacks;
    const setTermFontFamily = deps.setTermFontFamily;
    const setTermFontSize = deps.setTermFontSize;
    const configService = global.conchConfigService || {};

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
        }

        const appCfg = await invoke('get_app_config');
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
          }
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

  global.conchConfigRuntime = {
    create,
  };
})(window);
