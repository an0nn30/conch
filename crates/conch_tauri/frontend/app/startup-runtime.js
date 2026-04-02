(function initConchStartupRuntime(global) {
  function create() {
    function initStatusController() {
      const statusEl = document.getElementById('status');
      const statusMessageEl = document.getElementById('status-message');
      const statusDismissBtn = document.getElementById('status-dismiss');

      function hideStatus() {
        statusEl.style.display = 'none';
        statusMessageEl.textContent = '';
      }

      function showStatus(message) {
        statusEl.style.display = 'flex';
        statusMessageEl.textContent = message;
        console.error(message);
      }

      statusDismissBtn.addEventListener('click', hideStatus);
      document.addEventListener('keydown', (event) => {
        if (event.key !== 'Escape') return;
        if (statusEl.style.display === 'none') return;
        hideStatus();
      }, true);

      window.addEventListener('error', (event) => {
        showStatus('Frontend error: ' + event.message);
      });
      window.addEventListener('unhandledrejection', (event) => {
        showStatus('Unhandled promise rejection: ' + String(event.reason));
      });

      return { showStatus, hideStatus };
    }

    function ensureRuntimeDependencies(tauri, showStatus) {
      if (!tauri || !tauri.core || !tauri.event) {
        showStatus(
          'Tauri API is unavailable in this webview.\n' +
          'The app likely loaded from the wrong URL/context.'
        );
        return false;
      }
      if (typeof global.Terminal === 'undefined' || typeof global.FitAddon === 'undefined' || typeof global.WebLinksAddon === 'undefined') {
        showStatus(
          'xterm.js assets failed to load.\n' +
          'Check internet access or replace CDN assets with local files.'
        );
        return false;
      }
      return true;
    }

    function loadTerminalConfig(invoke, fontFallbacks) {
      const config = {
        fontFamily: '"JetBrains Mono", "Fira Code", "Cascadia Code"' + fontFallbacks,
        fontSize: 14,
        cursorStyle: 'block',
        cursorBlink: true,
        scrollSensitivity: 1,
      };
      return invoke('get_terminal_config')
        .then((tc) => {
          if (tc.font_family) config.fontFamily = '"' + tc.font_family + '", "Fira Code", "Cascadia Code"' + fontFallbacks;
          if (tc.font_size > 0) config.fontSize = tc.font_size;
          if (tc.cursor_style) config.cursorStyle = tc.cursor_style;
          config.cursorBlink = tc.cursor_blink;
          if (tc.scroll_sensitivity > 0) config.scrollSensitivity = tc.scroll_sensitivity;
          return config;
        })
        .catch((event) => {
          console.warn('Failed to load terminal config:', event);
          return config;
        });
    }

    function applyThemeCss(tc) {
      const r = document.documentElement.style;
      r.setProperty('--bg', tc.background);
      r.setProperty('--fg', tc.foreground);
      r.setProperty('--dim-fg', tc.dim_fg);
      r.setProperty('--panel-bg', tc.panel_bg);
      r.setProperty('--tab-bar-bg', tc.tab_bar_bg);
      r.setProperty('--tab-border', tc.tab_border);
      r.setProperty('--active-highlight', tc.active_highlight);
      r.setProperty('--red', tc.red);
      r.setProperty('--green', tc.green);
      r.setProperty('--yellow', tc.yellow);
      r.setProperty('--blue', tc.blue);
      r.setProperty('--cyan', tc.cyan);
      r.setProperty('--magenta', tc.magenta);
      r.setProperty('--input-bg', tc.input_bg);
      r.setProperty('--hover-bg', tc.input_bg);
      if (tc.text_secondary) r.setProperty('--text-secondary', tc.text_secondary);
      if (tc.text_muted) r.setProperty('--text-muted', tc.text_muted);
      document.body.style.background = tc.background;
    }

    function loadTheme(invoke, fallbackTheme) {
      return invoke('get_theme_colors')
        .then((tc) => {
          const theme = {
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
          applyThemeCss(tc);
          return theme;
        })
        .catch((event) => {
          console.warn('Failed to load theme colors:', event);
          return fallbackTheme;
        });
    }

    async function applyAppConfig(invoke) {
      let borderlessMode = false;
      try {
        const appCfg = await invoke('get_app_config');
        if ((appCfg.platform === 'windows' || appCfg.platform === 'linux') && appCfg.decorations !== 'none') {
          document.getElementById('app').classList.add('custom-titlebar');
          window._initTitlebarPending = true;
        } else if (appCfg.decorations === 'none' || appCfg.decorations === 'buttonless') {
          borderlessMode = true;
          document.getElementById('drag-handle').classList.add('visible');
          document.getElementById('tabbar').setAttribute('data-tauri-drag-region', '');
        }
        if (window.toast && window.toast.configure) {
          window.toast.configure({
            position: appCfg.notification_position || 'bottom',
            nativeNotifications: appCfg.native_notifications !== false,
          });
        }
        if (window.notificationPanel) window.notificationPanel.init();

        try {
          const layoutData = await invoke('get_saved_layout');
          if (layoutData.bottom_panel_visible === false) {
            document.getElementById('bottom-panel').classList.add('hidden');
          } else {
            document.getElementById('bottom-panel').classList.remove('hidden');
          }
          if (layoutData.bottom_panel_height > 0) {
            document.getElementById('bottom-panel').style.height = layoutData.bottom_panel_height + 'px';
          }
        } catch (_) {}

        const r = document.documentElement.style;
        if (appCfg.ui_font_small > 0) r.setProperty('--ui-font-small', appCfg.ui_font_small + 'px');
        if (appCfg.ui_font_list > 0) r.setProperty('--ui-font-list', appCfg.ui_font_list + 'px');
        if (appCfg.ui_font_normal > 0) r.setProperty('--ui-font-normal', appCfg.ui_font_normal + 'px');
        if (appCfg.ui_font_family) {
          document.body.style.fontFamily = appCfg.ui_font_family + ', -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
        }
        if (appCfg.ui_font_size > 0) {
          document.body.style.fontSize = appCfg.ui_font_size + 'px';
        }
      } catch (_) {}
      return { borderlessMode };
    }

    return {
      initStatusController,
      ensureRuntimeDependencies,
      loadTerminalConfig,
      loadTheme,
      applyAppConfig,
    };
  }

  global.conchStartupRuntime = {
    create,
  };
})(window);
