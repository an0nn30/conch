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

    // Completion's suggestions-as-you-type flag. Absent or unreadable config
    // leaves the module's own default (on) alone rather than forcing it off:
    // manual completion works either way, so a failed read must not silently
    // disable a feature the user turned on.
    function applySuggestionsWhileTyping(appCfg) {
      const completion = global.termlabLspCompletion;
      if (!appCfg || !completion || typeof completion.setSuggestionsWhileTyping !== 'function') {
        return;
      }
      if (typeof appCfg.editor_lsp_suggestions_while_typing !== 'boolean') return;
      completion.setSuggestionsWhileTyping(appCfg.editor_lsp_suggestions_while_typing);
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
        applySuggestionsWhileTyping(appCfg);
        // Published for the post-fit window sizing in main-runtime.js, which
        // runs long after this and needs the configured columns/lines.
        window.__termlabAppConfig = appCfg;

        // Adopt a queued directory as THIS window's project before the layout
        // is read: `get_saved_layout` returns the per-project layout once the
        // window is bound, and the tool-window runtime (which registers the
        // Search window only for a project) runs later still.
        //
        // Gated on pending_open_paths_kind === 'project': a mixed queue (a
        // directory queued alongside a file for the same window) classifies
        // as "files" — the file's editor-only zen window is the stronger
        // claim, and adopting unconditionally would still drain the
        // directory (project_adopt_pending only takes directories out of the
        // queue) and bind a project root onto what should stay a zen
        // file-editor window.
        if (global.termlabProjectMode && typeof global.termlabProjectMode.adopt === 'function') {
          try {
            const pendingKindForAdopt = await invoke('pending_open_paths_kind');
            if (pendingKindForAdopt === 'project') {
              const adopted = await global.termlabProjectMode.adopt(invoke);
              // A peek that says "project" whose adopt then fails (folder
              // removed mid-boot, permission denied, backend error) must not
              // silently boot a plain terminal — the routing path (a
              // directory opened from an already-running window) toasts by
              // name for the same failure, so this seam should too. The one
              // exception is a benign "another window already has this
              // root" hand-off: project_adopt_pending destroys THIS window
              // in that case, so there is nothing to explain.
              const focusedExisting = typeof global.termlabProjectMode.adoptFocusedExisting === 'function'
                && global.termlabProjectMode.adoptFocusedExisting();
              if (!adopted && !focusedExisting && window.toast) {
                window.toast.error(
                  'Cannot Open Folder',
                  'The project could not be opened — it may have been moved, deleted, or you may not have permission to access it.',
                );
              }
            }
          } catch (_) {}
        }

        // The adopt block above only ever resolves a project that arrived
        // through PendingOpens — a directory queued by the CLI/IPC before
        // this window existed. Opening a project from an ALREADY-RUNNING
        // window (Open Folder in the menu/palette, or routing a directory
        // dropped/opened in a running window) goes a completely different
        // way: `project_open` builds a brand-new window and binds the
        // registry entry for it directly (project_open_build, before the
        // window is even shown) — nothing is ever queued into PendingOpens
        // for that new window, so the adopt block above is a no-op for it.
        // `project_info` resolves independently, by the CALLING window's own
        // label against that same registry — ask it whenever adopt did not
        // already win this window a project, so this seam covers BOTH ways a
        // window can end up with one. Gated on `!isActive()` so a successful
        // adopt is never redundantly re-queried or overwritten. Still read
        // before the layout below, so the per-project layout applies either
        // way a project was bound.
        if (global.termlabProjectMode
            && typeof global.termlabProjectMode.isActive === 'function'
            && !global.termlabProjectMode.isActive()
            && typeof global.termlabProjectMode.set === 'function') {
          try {
            global.termlabProjectMode.set(await invoke('project_info'));
          } catch (_) {}
        }

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
          // A window with queued CLI FILE paths (`termlab notes.md`) boots in
          // zen regardless of the saved layout: it is about to become an
          // editor-only window. A window opening a PROJECT (`termlab .`) does
          // the opposite — it keeps its panels, because the tree and the
          // search panel are the point. `pending_open_paths_kind` is a
          // non-destructive peek; the destructive take happens later in
          // main-runtime, after the editor service exists. Session-only, same
          // as the new-window default: this window must never teach the
          // shared layout to open in zen.
          try {
            const pendingKind = await invoke('pending_open_paths_kind');
            if (pendingKind === 'files') {
              zenOn = true;
              window.__termlabZenIsSessionDefault = true;
            }
          } catch (_) {}
          if (global.termlabProjectMode && global.termlabProjectMode.isActive()) {
            zenOn = false;
            window.__termlabZenIsSessionDefault = true;
          }
          // The EFFECTIVE decision, as opposed to __termlabInitialZenMode
          // (the raw saved value) and __termlabZenIsSessionDefault (whether
          // that decision belongs only to this window). Three other modules
          // need "is this window actually in zen right now" rather than
          // either of those: the zen-default toast (main-runtime.js) must
          // not claim zen when a project window forced it off; the
          // panel-hiding fallback (tool-window-runtime.js) must not hide
          // panels a project window deliberately kept visible just because
          // the saved layout says zen_mode; and the Zen Mode menu toggle
          // (menu-actions.js) must seed its own active/inactive state from
          // what actually happened, not from the pre-override saved value.
          // __termlabInitialZenMode itself stays untouched — the save path
          // (tool-window-runtime.js's saveLayoutNow) depends on the RAW
          // value to correctly persist an inherited-not-live zen state.
          window.__termlabEffectiveZen = zenOn;
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
