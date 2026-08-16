(function initTermLabConfigService(global) {
  'use strict';

  const DEFAULT_UI_FONT_STACK = '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
  const DEFAULT_SKIN_ID = 'default';
  const KNOWN_SKINS = Object.freeze([
    { id: 'default', label: 'Default' },
    { id: 'metal', label: 'Metal (Swing)' },
    { id: 'win95', label: 'Windows 95' },
    { id: 'win31', label: 'Windows 3.1' },
    { id: 'winxp-luna', label: 'Windows XP Luna' },
    { id: 'win2000-classic', label: 'Windows 2000 Classic' },
    { id: 'mac-os9-platinum', label: 'Mac OS 9 Platinum' },
    { id: 'mac-osx-panther', label: 'Mac OS X Panther' },
    { id: 'gnome2-clearlooks', label: 'GNOME 2 Clearlooks' },
    { id: 'kde3-keramik', label: 'KDE 3 Keramik' },
    { id: 'motif', label: 'Motif' },
    { id: 'nextstep', label: 'NeXTSTEP' },
    { id: 'amiga-workbench', label: 'Amiga Workbench' },
    { id: 'ibm-cua', label: 'IBM CUA' },
    { id: 'terminal-glass', label: 'Terminal Glass' },
    { id: 'crt-amber', label: 'CRT Amber' },
    { id: 'cyberdeck-industrial', label: 'Cyberdeck Industrial' },
    { id: 'blueprint', label: 'Blueprint' },
  ]);
  const SKIN_CSS_OVERRIDES = Object.freeze({
    metal: Object.freeze({
      '--bg': '#c3c9d2',
      '--fg': '#1e2328',
      '--dim-fg': '#5f6774',
      '--panel-bg': '#d6dde6',
      '--tab-bar-bg': '#c8d1dc',
      '--tab-border': '#97a4b6',
      '--active-highlight': '#b4bfce',
      '--input-bg': '#eef2f7',
      '--hover-bg': '#d7deea',
      '--text-secondary': '#2a313b',
      '--text-muted': '#5f6774',
      '--blue': '#3d5f9b',
      '--cyan': '#2f6686',
      '--green': '#2f7a3c',
      '--yellow': '#8c6f22',
      '--magenta': '#7f4e80',
      '--red': '#9b3f3f',
    }),
    win95: Object.freeze({
      '--bg': '#008080',
      '--fg': '#000000',
      '--dim-fg': '#3e3e3e',
      '--panel-bg': '#c0c0c0',
      '--tab-bar-bg': '#c0c0c0',
      '--tab-border': '#7f7f7f',
      '--active-highlight': '#000080',
      '--input-bg': '#ffffff',
      '--hover-bg': '#d4d0c8',
      '--text-secondary': '#000000',
      '--text-muted': '#4a4a4a',
      '--blue': '#000080',
      '--cyan': '#008080',
      '--green': '#008000',
      '--yellow': '#808000',
      '--magenta': '#800080',
      '--red': '#800000',
    }),
    win31: Object.freeze({
      '--bg': '#818181',
      '--fg': '#000000',
      '--dim-fg': '#2f2f2f',
      '--panel-bg': '#c0c0c0',
      '--tab-bar-bg': '#c0c0c0',
      '--tab-border': '#7f7f7f',
      '--active-highlight': '#000080',
      '--input-bg': '#ffffff',
      '--hover-bg': '#dcdcdc',
      '--text-secondary': '#000000',
      '--text-muted': '#3a3a3a',
      '--blue': '#000080',
      '--cyan': '#008080',
      '--green': '#007800',
      '--yellow': '#7f7f00',
      '--magenta': '#7f007f',
      '--red': '#7f0000',
    }),
    'winxp-luna': Object.freeze({
      '--bg': '#3a6ea5',
      '--fg': '#11243f',
      '--dim-fg': '#4f6786',
      '--panel-bg': '#dbe8f8',
      '--tab-bar-bg': '#c5daf4',
      '--tab-border': '#87a8d2',
      '--active-highlight': '#4b7fd1',
      '--input-bg': '#ffffff',
      '--hover-bg': '#ecf4ff',
      '--text-secondary': '#183459',
      '--text-muted': '#5a7296',
      '--blue': '#2c63bf',
      '--cyan': '#2c7cbf',
      '--green': '#2b8b45',
      '--yellow': '#b9932f',
      '--magenta': '#9654b6',
      '--red': '#b54848',
    }),
    'win2000-classic': Object.freeze({
      '--bg': '#3a6ea5',
      '--fg': '#000000',
      '--dim-fg': '#3f3f3f',
      '--panel-bg': '#d4d0c8',
      '--tab-bar-bg': '#d4d0c8',
      '--tab-border': '#808080',
      '--active-highlight': '#0a246a',
      '--input-bg': '#ffffff',
      '--hover-bg': '#ece9d8',
      '--text-secondary': '#101010',
      '--text-muted': '#555555',
      '--blue': '#0a246a',
      '--cyan': '#2077a3',
      '--green': '#2d7f42',
      '--yellow': '#8b7a20',
      '--magenta': '#7b518e',
      '--red': '#8d3c3c',
    }),
    'mac-os9-platinum': Object.freeze({
      '--bg': '#c7c7c7',
      '--fg': '#1f1f1f',
      '--dim-fg': '#595959',
      '--panel-bg': '#d9d9d9',
      '--tab-bar-bg': '#cccccc',
      '--tab-border': '#8a8a8a',
      '--active-highlight': '#3d6da8',
      '--input-bg': '#ffffff',
      '--hover-bg': '#ebebeb',
      '--text-secondary': '#222222',
      '--text-muted': '#666666',
      '--blue': '#3d6da8',
      '--cyan': '#3b8ca6',
      '--green': '#3b7b49',
      '--yellow': '#9a8536',
      '--magenta': '#8a5f9d',
      '--red': '#9f5252',
    }),
    'mac-osx-panther': Object.freeze({
      '--bg': '#7e8792',
      '--fg': '#1b1e24',
      '--dim-fg': '#4f5560',
      '--panel-bg': '#cfd5dd',
      '--tab-bar-bg': '#b8c0ca',
      '--tab-border': '#8a939f',
      '--active-highlight': '#3f7ac9',
      '--input-bg': '#f8fafc',
      '--hover-bg': '#dde3ea',
      '--text-secondary': '#252a32',
      '--text-muted': '#5b6471',
      '--blue': '#3f7ac9',
      '--cyan': '#3e8ea8',
      '--green': '#3f8a55',
      '--yellow': '#9f8b3f',
      '--magenta': '#8d63a5',
      '--red': '#ab5a5a',
    }),
    'gnome2-clearlooks': Object.freeze({
      '--bg': '#d8d9db',
      '--fg': '#1f2328',
      '--dim-fg': '#666d76',
      '--panel-bg': '#eceff2',
      '--tab-bar-bg': '#dfe4ea',
      '--tab-border': '#a8b0bb',
      '--active-highlight': '#4b79b9',
      '--input-bg': '#ffffff',
      '--hover-bg': '#f4f7fa',
      '--text-secondary': '#2a3139',
      '--text-muted': '#6a7380',
      '--blue': '#4b79b9',
      '--cyan': '#3d879f',
      '--green': '#4b8750',
      '--yellow': '#a28e43',
      '--magenta': '#9060a4',
      '--red': '#b05a5a',
    }),
    'kde3-keramik': Object.freeze({
      '--bg': '#cad4e2',
      '--fg': '#1f2834',
      '--dim-fg': '#5d6878',
      '--panel-bg': '#dbe4ef',
      '--tab-bar-bg': '#c8d6e8',
      '--tab-border': '#8da2bd',
      '--active-highlight': '#3f6fb2',
      '--input-bg': '#eef4fb',
      '--hover-bg': '#e4edf8',
      '--text-secondary': '#293648',
      '--text-muted': '#66778e',
      '--blue': '#3f6fb2',
      '--cyan': '#3c88a5',
      '--green': '#3f8755',
      '--yellow': '#9f8b40',
      '--magenta': '#875fa0',
      '--red': '#a55757',
    }),
    motif: Object.freeze({
      '--bg': '#7a7a7a',
      '--fg': '#000000',
      '--dim-fg': '#3f3f3f',
      '--panel-bg': '#b8b8b8',
      '--tab-bar-bg': '#b0b0b0',
      '--tab-border': '#6a6a6a',
      '--active-highlight': '#2f4f7f',
      '--input-bg': '#d8d8d8',
      '--hover-bg': '#c7c7c7',
      '--text-secondary': '#111111',
      '--text-muted': '#525252',
      '--blue': '#2f4f7f',
      '--cyan': '#2f6f7f',
      '--green': '#2f6f3f',
      '--yellow': '#7f6f2f',
      '--magenta': '#6f3f7f',
      '--red': '#7f3f3f',
    }),
    nextstep: Object.freeze({
      '--bg': '#2a2a2a',
      '--fg': '#e9e9e9',
      '--dim-fg': '#b7b7b7',
      '--panel-bg': '#3a3a3a',
      '--tab-bar-bg': '#343434',
      '--tab-border': '#1e1e1e',
      '--active-highlight': '#ffffff',
      '--input-bg': '#1f1f1f',
      '--hover-bg': '#454545',
      '--text-secondary': '#e0e0e0',
      '--text-muted': '#a8a8a8',
      '--blue': '#6f8fbf',
      '--cyan': '#6fb7bf',
      '--green': '#7fbf8f',
      '--yellow': '#bfbf7f',
      '--magenta': '#bf8fbf',
      '--red': '#bf7f7f',
    }),
    'amiga-workbench': Object.freeze({
      '--bg': '#0055aa',
      '--fg': '#0f1e2f',
      '--dim-fg': '#4b5d72',
      '--panel-bg': '#d3d7df',
      '--tab-bar-bg': '#b8c2cf',
      '--tab-border': '#7a889c',
      '--active-highlight': '#ff8c00',
      '--input-bg': '#f0f3f8',
      '--hover-bg': '#e4ebf5',
      '--text-secondary': '#1a2a3f',
      '--text-muted': '#5a6f89',
      '--blue': '#0055aa',
      '--cyan': '#2687aa',
      '--green': '#2d8a45',
      '--yellow': '#b08a2d',
      '--magenta': '#8b5fb5',
      '--red': '#b55353',
    }),
    'ibm-cua': Object.freeze({
      '--bg': '#0b1f43',
      '--fg': '#e9eefb',
      '--dim-fg': '#9fb2de',
      '--panel-bg': '#122d5c',
      '--tab-bar-bg': '#0f2852',
      '--tab-border': '#274980',
      '--active-highlight': '#2f65c7',
      '--input-bg': '#0d2147',
      '--hover-bg': '#19396f',
      '--text-secondary': '#dae4fb',
      '--text-muted': '#9eb1de',
      '--blue': '#2f65c7',
      '--cyan': '#2f87c7',
      '--green': '#39b36a',
      '--yellow': '#d2bc52',
      '--magenta': '#a17be2',
      '--red': '#d26e6e',
    }),
    'terminal-glass': Object.freeze({
      '--bg': '#0f141c',
      '--fg': '#d9e5f4',
      '--dim-fg': '#8ea0b8',
      '--panel-bg': 'rgba(30, 42, 56, 0.72)',
      '--tab-bar-bg': 'rgba(26, 36, 49, 0.78)',
      '--tab-border': '#42556f',
      '--active-highlight': '#4d89d9',
      '--input-bg': 'rgba(20, 30, 42, 0.76)',
      '--hover-bg': 'rgba(67, 89, 118, 0.35)',
      '--text-secondary': '#c9d7ea',
      '--text-muted': '#8ea2be',
      '--blue': '#4d89d9',
      '--cyan': '#4da9d9',
      '--green': '#59c785',
      '--yellow': '#d8c062',
      '--magenta': '#ad85e1',
      '--red': '#d97a7a',
    }),
    'crt-amber': Object.freeze({
      '--bg': '#140f07',
      '--fg': '#ffb648',
      '--dim-fg': '#a87a30',
      '--panel-bg': '#1c150a',
      '--tab-bar-bg': '#191206',
      '--tab-border': '#5f4420',
      '--active-highlight': '#7a5624',
      '--input-bg': '#1a1308',
      '--hover-bg': '#2c2010',
      '--text-secondary': '#f2ae46',
      '--text-muted': '#9f7230',
      '--blue': '#b08242',
      '--cyan': '#b08b52',
      '--green': '#c49a4f',
      '--yellow': '#ffbf52',
      '--magenta': '#a17746',
      '--red': '#b76c3d',
    }),
    'cyberdeck-industrial': Object.freeze({
      '--bg': '#1b1f24',
      '--fg': '#d7dde4',
      '--dim-fg': '#7d8794',
      '--panel-bg': '#242a31',
      '--tab-bar-bg': '#20262d',
      '--tab-border': '#4d5968',
      '--active-highlight': '#ffb300',
      '--input-bg': '#191e23',
      '--hover-bg': '#303943',
      '--text-secondary': '#d3dae3',
      '--text-muted': '#8a95a5',
      '--blue': '#5d83c6',
      '--cyan': '#4da1b0',
      '--green': '#6aa96f',
      '--yellow': '#ffb300',
      '--magenta': '#9a78b8',
      '--red': '#d07a6a',
    }),
    blueprint: Object.freeze({
      '--bg': '#1a365d',
      '--fg': '#d8ebff',
      '--dim-fg': '#94b8dd',
      '--panel-bg': '#214674',
      '--tab-bar-bg': '#1c3c67',
      '--tab-border': '#4f7fb3',
      '--active-highlight': '#8ac2ff',
      '--input-bg': '#1a3558',
      '--hover-bg': '#2b5486',
      '--text-secondary': '#d5e8ff',
      '--text-muted': '#95b7de',
      '--blue': '#8ac2ff',
      '--cyan': '#74d4ff',
      '--green': '#80d8b2',
      '--yellow': '#ffe08a',
      '--magenta': '#c5a7ff',
      '--red': '#ff9a9a',
    }),
  });
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

  function normalizeSkinId(value) {
    const raw = String(value || '').trim().toLowerCase();
    if (!raw) return DEFAULT_SKIN_ID;
    for (const skin of KNOWN_SKINS) {
      if (skin.id === raw) return skin.id;
    }
    return DEFAULT_SKIN_ID;
  }

  function applySkinCssVariables(rootStyle, skinId) {
    if (lastThemeCssVars) {
      for (const [name, value] of Object.entries(lastThemeCssVars)) {
        rootStyle.setProperty(name, value);
      }
    }

    const activeVars = SKIN_CSS_OVERRIDES[skinId];
    if (!activeVars) return;
    for (const [name, value] of Object.entries(activeVars)) {
      rootStyle.setProperty(name, value);
    }
  }

  function getAvailableSkins() {
    return KNOWN_SKINS.map((skin) => ({ id: skin.id, label: skin.label }));
  }

  function resolveNativeWindowTheme(appCfg, skinId) {
    if (!appCfg || typeof appCfg !== 'object') return undefined;
    if (String(appCfg.platform || '').toLowerCase() !== 'macos') return undefined;
    if (String(appCfg.decorations || '').toLowerCase() !== 'full') return undefined;

    if (skinId === 'metal') return 'dark';

    const appearanceMode = String(appCfg.appearance_mode || 'system').toLowerCase();
    if (appearanceMode === 'dark') return 'dark';
    if (appearanceMode === 'light') return 'light';
    return null;
  }

  function applyNativeWindowTheme(appCfg, skinId) {
    const nextTheme = resolveNativeWindowTheme(appCfg, skinId);
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

    const skinId = normalizeSkinId(appCfg.ui_skin);
    root.dataset.skin = skinId;
    applySkinCssVariables(rootStyle, skinId);
    applyNativeWindowTheme(appCfg, skinId);

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

    return { borderlessMode, skinId };
  }

  global.termlabConfigService = {
    applyThemeCss,
    toTerminalTheme,
    applyUiConfig,
    getAvailableSkins,
  };
})(window);
