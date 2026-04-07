(function initConchStartupRuntime(global) {
  function create() {
    const configService = global.conchConfigService || {};
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
      if (global.conchKeyboardRouter && typeof global.conchKeyboardRouter.register === 'function') {
        global.conchKeyboardRouter.register({
          name: 'status-dismiss',
          priority: 30,
          isActive: () => statusEl.style.display !== 'none',
          onKeyDown: (event) => {
            if (event.key !== 'Escape') return false;
            hideStatus();
            return true;
          },
        });
      } else {
        console.warn('startup-runtime: keyboard router unavailable, status Escape handler not registered');
      }

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

    function loadTheme(invoke, fallbackTheme) {
      return invoke('get_theme_colors')
        .then((tc) => {
          const defaultTheme = {
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
          const theme = typeof configService.toTerminalTheme === 'function'
            ? configService.toTerminalTheme(tc, defaultTheme)
            : defaultTheme;
          if (typeof configService.applyThemeCss === 'function') {
            configService.applyThemeCss(tc);
          }
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
        if (typeof configService.applyUiConfig === 'function') {
          const uiResult = configService.applyUiConfig(appCfg) || {};
          borderlessMode = uiResult.borderlessMode === true;
        }
        if (window.notificationPanel) window.notificationPanel.init();

        try {
          const layoutData = await invoke('get_saved_layout');
          window.__conchInitialLayout = layoutData;
          window.__conchInitialZenMode = layoutData.zen_mode === true;
          window.__conchZenRestoreState = {
            leftVisible: layoutData.files_panel_visible !== false,
            rightVisible: layoutData.ssh_panel_visible !== false,
            bottomVisible: layoutData.bottom_panel_visible !== false,
          };
          if (layoutData.zen_mode === true) {
            document.getElementById('app').classList.add('zen-mode');
          } else {
            document.getElementById('app').classList.remove('zen-mode');
          }
          if (layoutData.zen_mode === true || layoutData.bottom_panel_visible === false) {
            document.getElementById('bottom-panel').classList.add('hidden');
          } else {
            document.getElementById('bottom-panel').classList.remove('hidden');
          }
          if (layoutData.bottom_panel_height > 0) {
            document.getElementById('bottom-panel').style.height = layoutData.bottom_panel_height + 'px';
          }
        } catch (_) {}

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
