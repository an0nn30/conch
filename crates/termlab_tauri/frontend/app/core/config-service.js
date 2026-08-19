(function initTermLabConfigService(global) {
  'use strict';

  const DEFAULT_UI_FONT_STACK = '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
  let lastThemeCssVars = null;
  let lastNativeWindowTheme = '__unset__';

  function mapThemeCssVars(themeColors) {
    if (!themeColors || typeof themeColors !== 'object') return null;
    // UI chrome (--bg, --panel-bg, --fg, --dim-fg, borders, inputs, selection,
    // secondary/muted text) is owned by the design-system tokens and must NOT
    // be set from the terminal color scheme: that coupling painted the entire
    // window with the console background. Terminal colors reach the terminal
    // itself through toTerminalTheme(). Only the accent palette below stays
    // theme-driven, so status dots keep the active palette's flavor.
    const vars = {
      // The terminal surface (and only that) tracks the terminal palette, so
      // the container behind xterm matches its canvas. Without this the panel
      // colour shows through as a grey band while a drag resizes the pane and
      // xterm has not re-fitted yet.
      '--tl-terminal-bg': themeColors.background,
      '--red': themeColors.red,
      '--green': themeColors.green,
      '--yellow': themeColors.yellow,
      '--blue': themeColors.blue,
      '--cyan': themeColors.cyan,
      '--magenta': themeColors.magenta,
    };
    // A missing color would be written out as the literal "undefined", which
    // makes every declaration reading it invalid — for the terminal background
    // that means a transparent pane rather than the fallback.
    for (const [name, value] of Object.entries(vars)) {
      if (typeof value !== 'string' || !value.trim()) delete vars[name];
    }
    return vars;
  }

  function resolveNativeWindowTheme(appCfg) {
    if (!appCfg || typeof appCfg !== 'object') return undefined;
    if (String(appCfg.platform || '').toLowerCase() !== 'macos') return undefined;
    if (String(appCfg.decorations || '').toLowerCase() !== 'full') return undefined;

    // Same fallback convention as app/core/appearance.js: 'light' is the only
    // affirmative light answer, 'system' is the only hand-off to the OS, and
    // anything else — including a missing appearance_mode — is 'dark'. The
    // webview and the native chrome have to agree on the unresolvable case:
    // mapping falsy to 'system' here would tint the frame by OS preference
    // while appearance.js forces the content dark.
    //
    // The only consumer is applyNativeWindowTheme() below, which turns
    // 'dark'/'light' into window.setTheme(...) and null into "follow the OS";
    // undefined (returned above) means "do not touch the frame at all".
    // Unreachable in practice: the backend always emits a lowercase enum.
    const appearanceMode = String(appCfg.appearance_mode || '').toLowerCase();
    if (appearanceMode === 'light') return 'light';
    if (appearanceMode === 'system') return null;
    return 'dark';
  }

  function applyNativeWindowTheme(appCfg) {
    const nextTheme = resolveNativeWindowTheme(appCfg);
    if (typeof nextTheme === 'undefined') return;

    const cacheKey = nextTheme == null ? '__system__' : String(nextTheme);
    if (cacheKey === lastNativeWindowTheme) return;

    const tauriWindow = global.__TAURI__ && global.__TAURI__.window;
    if (!tauriWindow || typeof tauriWindow.getCurrentWindow !== 'function') return;
    const currentWindow = tauriWindow.getCurrentWindow();
    if (!currentWindow || typeof currentWindow.setTheme !== 'function') return;

    currentWindow.setTheme(nextTheme).then(() => {
      lastNativeWindowTheme = cacheKey;
    }).catch(() => {});
  }

  function applyThemeCss(themeColors) {
    if (!themeColors || typeof themeColors !== 'object') return;
    const rootStyle = document.documentElement.style;
    lastThemeCssVars = mapThemeCssVars(themeColors);
    if (!lastThemeCssVars) return;
    for (const [name, value] of Object.entries(lastThemeCssVars)) {
      rootStyle.setProperty(name, value);
    }
  }

  function toTerminalTheme(themeColors, fallbackTheme) {
    if (!themeColors || typeof themeColors !== 'object') return fallbackTheme;
    return {
      background: themeColors.background,
      foreground: themeColors.foreground,
      cursor: themeColors.cursor_color,
      cursorAccent: themeColors.cursor_text,
      selectionBackground: themeColors.selection_bg,
      selectionForeground: themeColors.selection_text,
      black: themeColors.black,
      red: themeColors.red,
      green: themeColors.green,
      yellow: themeColors.yellow,
      blue: themeColors.blue,
      magenta: themeColors.magenta,
      cyan: themeColors.cyan,
      white: themeColors.white,
      brightBlack: themeColors.bright_black,
      brightRed: themeColors.bright_red,
      brightGreen: themeColors.bright_green,
      brightYellow: themeColors.bright_yellow,
      brightBlue: themeColors.bright_blue,
      brightMagenta: themeColors.bright_magenta,
      brightCyan: themeColors.bright_cyan,
      brightWhite: themeColors.bright_white,
    };
  }

  function applyUiConfig(appCfg) {
    if (!appCfg || typeof appCfg !== 'object') return { borderlessMode: false };

    const root = document.documentElement;
    root.classList.toggle('no-animations', appCfg.disable_animations === true);

    const rootStyle = root.style;
    if (appCfg.ui_font_small > 0) rootStyle.setProperty('--ui-font-small', appCfg.ui_font_small + 'px');
    if (appCfg.ui_font_list > 0) rootStyle.setProperty('--ui-font-list', appCfg.ui_font_list + 'px');
    if (appCfg.ui_font_normal > 0) rootStyle.setProperty('--ui-font-normal', appCfg.ui_font_normal + 'px');

    applyNativeWindowTheme(appCfg);

    if (appCfg.ui_font_family) {
      document.body.style.fontFamily = appCfg.ui_font_family + ', ' + DEFAULT_UI_FONT_STACK;
    } else {
      document.body.style.removeProperty('font-family');
    }
    if (appCfg.ui_font_size > 0) {
      document.body.style.fontSize = appCfg.ui_font_size + 'px';
    } else {
      document.body.style.removeProperty('font-size');
    }
    document.body.style.removeProperty('background');

    let borderlessMode = false;
    if ((appCfg.platform === 'windows' || appCfg.platform === 'linux') && appCfg.decorations !== 'none') {
      const app = document.getElementById('app');
      if (app) app.classList.add('custom-titlebar');
      global._initTitlebarPending = true;
    } else if (appCfg.decorations === 'none' || appCfg.decorations === 'buttonless') {
      borderlessMode = true;
      const dragHandle = document.getElementById('drag-handle');
      const tabBar = document.getElementById('tabbar');
      if (dragHandle) dragHandle.classList.add('visible');
      if (tabBar) tabBar.setAttribute('data-tauri-drag-region', '');
    }

    if (global.toast && typeof global.toast.configure === 'function') {
      global.toast.configure({
        position: appCfg.notification_position || 'bottom',
        nativeNotifications: appCfg.native_notifications !== false,
      });
    }

    return { borderlessMode };
  }

  global.termlabConfigService = {
    applyThemeCss,
    toTerminalTheme,
    applyUiConfig,
  };
})(window);
