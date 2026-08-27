(function initTermLabStartupRuntime(global) {
  function create() {
    const configService = global.termlabConfigService || {};
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
      if (global.termlabKeyboardRouter && typeof global.termlabKeyboardRouter.register === 'function') {
        global.termlabKeyboardRouter.register({
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

    // Called AFTER applyAppConfig has resolved the appearance (main-runtime
    // chains the two), because the palette `auto` resolves to depends on it:
    // fetching in parallel would paint the dark built-in on a light install
    // and only correct itself on the next flip.
    function loadTheme(invoke, fallbackTheme) {
      const resolvedAppearance = global.termlabAppearance
        && typeof global.termlabAppearance.current === 'function'
        ? global.termlabAppearance.current()
        : 'dark';
      return invoke('get_theme_colors', { resolvedAppearance })
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
      // The editor's vim keymap, returned so main-runtime can seed the flag it
      // hands to newly created editor panes. Off on the failure path: a config
      // that could not be read must not turn modal editing on.
      let vimMode = false;
      try {
        const appCfg = await invoke('get_app_config');
        vimMode = appCfg && appCfg.editor_vim_mode === true;
        // Published for the post-fit window sizing in main-runtime.js, which
        // runs long after this and needs the configured columns/lines.
        window.__termlabAppConfig = appCfg;
        if (global.termlabAppearance && typeof global.termlabAppearance.apply === 'function') {
          global.termlabAppearance.apply(appCfg && appCfg.appearance_mode);
        }
        if (typeof configService.applyUiConfig === 'function') {
          const uiResult = configService.applyUiConfig(appCfg) || {};
          borderlessMode = uiResult.borderlessMode === true;
        }
        if (window.notificationPanel) window.notificationPanel.init();

        try {
          const layoutData = await invoke('get_saved_layout');
          window.__termlabInitialLayout = layoutData;
          window.__termlabInitialZenMode = layoutData.zen_mode === true;
          window.__termlabZenRestoreState = {
            leftVisible: layoutData.files_panel_visible !== false,
            rightVisible: layoutData.ssh_panel_visible !== false,
            bottomVisible: layoutData.bottom_panel_visible !== false,
          };
          // A window opened with Cmd+Shift+N starts in zen mode when the
          // setting is on, whatever the saved layout says: the usual shape is
          // one main window with the panels showing and extra windows used as
          // bare terminals. create_new_window labels these "window-N"; the
          // first window is "main".
          //
          // __termlabZenIsSessionDefault marks zen as belonging to this window
          // only. tool-window-runtime.js's save path reads it and persists the
          // INHERITED zen state instead of the live one, so a throwaway window
          // never teaches the shared layout to open the main window in zen.
          // Toggling zen by hand clears the flag (menu-actions.js), making the
          // choice persist as usual.
          let zenOn = layoutData.zen_mode === true;
          try {
            const label = await invoke('current_window_label');
            const isSecondaryWindow = typeof label === 'string' && label.startsWith('window-');
            if (isSecondaryWindow && appCfg && appCfg.new_window_zen_mode !== false) {
              zenOn = true;
              window.__termlabZenIsSessionDefault = true;
            }
          } catch (_) {
            // No window label: treat it as the main window and honour the
            // saved layout.
          }
          // A window with queued CLI open paths (`termlab notes.md`) boots in
          // zen regardless of the saved layout or window kind: it is about to
          // become an editor-only window (main-runtime skips the terminal
          // tab), and should read as a small editor app, not a terminal with
          // chrome. `has_pending_open_paths` is a non-destructive peek — the
          // destructive take happens later in main-runtime, after the editor
          // service exists. Session-only, same as the new-window default:
          // this window must never teach the shared layout to open in zen.
          try {
            if (await invoke('has_pending_open_paths')) {
              zenOn = true;
              window.__termlabZenIsSessionDefault = true;
            }
          } catch (_) {}
          if (zenOn) {
            document.getElementById('app').classList.add('zen-mode');
          } else {
            document.getElementById('app').classList.remove('zen-mode');
          }
          // Bottom-zone visibility/height restore is handled by
          // tool-window-runtime.js's own restore block instead of here: the
          // tool window manager (and its #bottom-zone-wrap DOM refs) isn't
          // initialized yet at this point in startup, and — unlike the old
          // tabbed bottom panel — visibility now requires an active tool
          // window, which only exists once plugin/bottom windows register.
        } catch (_) {}

      } catch (_) {}
      return { borderlessMode, vimMode };
    }

    return {
      initStatusController,
      ensureRuntimeDependencies,
      loadTerminalConfig,
      loadTheme,
      applyAppConfig,
    };
  }

  global.termlabStartupRuntime = {
    create,
  };
})(window);
