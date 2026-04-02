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

    async function applyConfigChanged() {
      try {
        await refreshKeyboardShortcutFallbacks();
        const tc = await invoke('get_theme_colors');

        const rootStyle = document.documentElement.style;
        rootStyle.setProperty('--bg', tc.background);
        rootStyle.setProperty('--fg', tc.foreground);
        rootStyle.setProperty('--dim-fg', tc.dim_fg);
        rootStyle.setProperty('--panel-bg', tc.panel_bg);
        rootStyle.setProperty('--tab-bar-bg', tc.tab_bar_bg);
        rootStyle.setProperty('--tab-border', tc.tab_border);
        rootStyle.setProperty('--active-highlight', tc.active_highlight);
        rootStyle.setProperty('--input-bg', tc.input_bg);
        rootStyle.setProperty('--hover-bg', tc.input_bg);
        rootStyle.setProperty('--red', tc.red);
        rootStyle.setProperty('--green', tc.green);
        rootStyle.setProperty('--yellow', tc.yellow);
        rootStyle.setProperty('--blue', tc.blue);
        rootStyle.setProperty('--cyan', tc.cyan);
        rootStyle.setProperty('--magenta', tc.magenta);
        if (tc.text_secondary) rootStyle.setProperty('--text-secondary', tc.text_secondary);
        if (tc.text_muted) rootStyle.setProperty('--text-muted', tc.text_muted);
        document.body.style.background = tc.background;

        const newTheme = {
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
        setTheme(newTheme);
        for (const pane of getPanes().values()) {
          if (pane.kind === 'terminal' && pane.term) {
            pane.term.options.theme = newTheme;
          }
        }

        const appCfg = await invoke('get_app_config');
        if (appCfg.ui_font_small > 0) rootStyle.setProperty('--ui-font-small', appCfg.ui_font_small + 'px');
        if (appCfg.ui_font_list > 0) rootStyle.setProperty('--ui-font-list', appCfg.ui_font_list + 'px');
        if (appCfg.ui_font_normal > 0) rootStyle.setProperty('--ui-font-normal', appCfg.ui_font_normal + 'px');
        if (appCfg.ui_font_family) {
          document.body.style.fontFamily = appCfg.ui_font_family + ', -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
        }
        if (appCfg.ui_font_size > 0) {
          document.body.style.fontSize = appCfg.ui_font_size + 'px';
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

        if (global.toast && global.toast.configure) {
          global.toast.configure({
            position: appCfg.notification_position || 'bottom',
            nativeNotifications: appCfg.native_notifications !== false,
          });
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
