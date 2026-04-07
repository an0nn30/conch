(function initConchConfigService(global) {
  'use strict';

  function applyThemeCss(themeColors) {
    if (!themeColors || typeof themeColors !== 'object') return;
    const rootStyle = document.documentElement.style;
    rootStyle.setProperty('--bg', themeColors.background);
    rootStyle.setProperty('--fg', themeColors.foreground);
    rootStyle.setProperty('--dim-fg', themeColors.dim_fg);
    rootStyle.setProperty('--panel-bg', themeColors.panel_bg);
    rootStyle.setProperty('--tab-bar-bg', themeColors.tab_bar_bg);
    rootStyle.setProperty('--tab-border', themeColors.tab_border);
    rootStyle.setProperty('--active-highlight', themeColors.active_highlight);
    rootStyle.setProperty('--input-bg', themeColors.input_bg);
    rootStyle.setProperty('--hover-bg', themeColors.input_bg);
    rootStyle.setProperty('--red', themeColors.red);
    rootStyle.setProperty('--green', themeColors.green);
    rootStyle.setProperty('--yellow', themeColors.yellow);
    rootStyle.setProperty('--blue', themeColors.blue);
    rootStyle.setProperty('--cyan', themeColors.cyan);
    rootStyle.setProperty('--magenta', themeColors.magenta);
    if (themeColors.text_secondary) rootStyle.setProperty('--text-secondary', themeColors.text_secondary);
    if (themeColors.text_muted) rootStyle.setProperty('--text-muted', themeColors.text_muted);
    if (themeColors.background) {
      document.body.style.background = themeColors.background;
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

    document.documentElement.classList.toggle('no-animations', appCfg.disable_animations === true);

    const rootStyle = document.documentElement.style;
    if (appCfg.ui_font_small > 0) rootStyle.setProperty('--ui-font-small', appCfg.ui_font_small + 'px');
    if (appCfg.ui_font_list > 0) rootStyle.setProperty('--ui-font-list', appCfg.ui_font_list + 'px');
    if (appCfg.ui_font_normal > 0) rootStyle.setProperty('--ui-font-normal', appCfg.ui_font_normal + 'px');

    if (appCfg.ui_font_family) {
      document.body.style.fontFamily = appCfg.ui_font_family + ', -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
    }
    if (appCfg.ui_font_size > 0) {
      document.body.style.fontSize = appCfg.ui_font_size + 'px';
    }

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

  global.conchConfigService = {
    applyThemeCss,
    toTerminalTheme,
    applyUiConfig,
  };
})(window);
