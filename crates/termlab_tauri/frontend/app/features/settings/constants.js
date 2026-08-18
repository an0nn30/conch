(function initTermLabSettingsFeatureConstants(global) {
  'use strict';

  const SECTION_DEFS = [
    { group: 'Workspace', items: [
      { id: 'appearance', label: 'Appearance', description: 'Theme, skin, notifications, UI fonts', keywords: 'theme colors skin interface notifications fonts typography appearance' },
      { id: 'keyboard', label: 'Keymap', description: 'Core shortcuts, tool window shortcuts, plugin shortcuts', keywords: 'keyboard shortcuts keymap bindings hotkeys commands tool windows plugins' },
      { id: 'files', label: 'Files', description: 'File explorer behavior and path following', keywords: 'files explorer path follow cwd directory' },
      { id: 'window', label: 'Window', description: 'Default size in columns and lines, title bar style, zen mode for new windows', keywords: 'window size columns lines rows dimensions title bar decorations chrome zen menu bar' },
      { id: 'editor', label: 'Editor', description: 'Editor keybindings', keywords: 'editor vim keybindings modal keys code text' },
    ]},
    { group: 'Terminal', items: [
      { id: 'terminal', label: 'Terminal', description: 'Font rendering and scrolling', keywords: 'terminal font size offset scrolling display rendering' },
      { id: 'cursor', label: 'Cursor', description: 'Cursor shape, blinking, vi mode override', keywords: 'cursor block beam underline blinking vi mode caret' },
      { id: 'shell', label: 'Shell', description: 'Shell program, arguments, environment variables', keywords: 'shell program launch arguments env environment variables login command' },
    ]},
    { group: 'Extensions', items: [
      { id: 'plugins', label: 'Plugins', description: 'Plugin system, plugin types, search paths, installed plugins', keywords: 'plugins extensions lua java search paths installed permissions' },
    ]},
    { group: 'System', items: [
      { id: 'advanced', label: 'Advanced', description: 'Startup behavior and UI density', keywords: 'advanced startup updates ui density font sizes' },
    ]},
  ];

  const SETTINGS_SEARCH_INDEX = [
    { section: 'appearance', label: 'Theme', keywords: 'color theme appearance scheme', targetId: 'appearance:theme' },
    { section: 'appearance', label: 'UI Skin', keywords: 'skin metal swing chrome styling look and feel', targetId: 'appearance:ui-skin' },
    { section: 'appearance', label: 'Appearance Mode', keywords: 'dark light system mode', targetId: 'appearance:mode' },
    { section: 'appearance', label: 'Notification Position', keywords: 'toast notifications top bottom', targetId: 'appearance:notification-position' },
    { section: 'appearance', label: 'Native Notifications', keywords: 'system notifications os notifications', targetId: 'appearance:native-notifications' },
    { section: 'appearance', label: 'Animations', keywords: 'animations motion transitions effects performance enable disable reduce motion', targetId: 'appearance:animations' },
    { section: 'window', label: 'Window Decorations', keywords: 'titlebar transparent buttonless none full window chrome', targetId: 'window:decorations' },
    { section: 'window', label: 'Default Window Size', keywords: 'columns lines rows window size dimensions 102 46 startup launch new window', targetId: 'window:columns' },
    { section: 'window', label: 'New Windows In Zen Mode', keywords: 'zen mode new window panels hidden distraction free', targetId: 'window:new-window-zen-mode' },
    { section: 'appearance', label: 'Native Menu Bar', keywords: 'menu bar macos native menu', targetId: 'appearance:native-menu-bar' },
    { section: 'editor', label: 'Vim Keybindings', keywords: 'editor vim modal keys keybindings vi normal insert mode :w :q', targetId: 'editor:vim-mode' },
    { section: 'appearance', label: 'UI Font Family', keywords: 'ui font family interface typography', targetId: 'appearance:ui-font-family' },
    { section: 'appearance', label: 'UI Font Size', keywords: 'ui font size interface typography', targetId: 'appearance:ui-font-size' },
    { section: 'keyboard', label: 'Keyboard Shortcuts', keywords: 'keyboard shortcuts keymap bindings' },
    { section: 'keyboard', label: 'Tool Window Shortcuts', keywords: 'tool window keyboard shortcuts sidebars panels' },
    { section: 'keyboard', label: 'Plugin Shortcuts', keywords: 'plugin keyboard shortcuts' },
    { section: 'files', label: 'Follow Path', keywords: 'files explorer follow path cwd directory sync', targetId: 'files:follow-path' },
    { section: 'terminal', label: 'Terminal Font Family', keywords: 'terminal font family monospace', targetId: 'terminal:font-family' },
    { section: 'terminal', label: 'Terminal Font Size', keywords: 'terminal font size', targetId: 'terminal:font-size' },
    { section: 'terminal', label: 'Font Offset X', keywords: 'font offset horizontal x rendering', targetId: 'terminal:font-offset-x' },
    { section: 'terminal', label: 'Font Offset Y', keywords: 'font offset vertical y rendering', targetId: 'terminal:font-offset-y' },
    { section: 'terminal', label: 'Scroll Sensitivity', keywords: 'scrolling trackpad mouse wheel sensitivity', targetId: 'terminal:scroll-sensitivity' },
    { section: 'shell', label: 'Shell Program', keywords: 'shell program executable login shell', targetId: 'shell:program' },
    { section: 'shell', label: 'Arguments', keywords: 'shell arguments flags startup command', targetId: 'shell:args' },
    { section: 'shell', label: 'Environment Variables', keywords: 'env environment variables terminal session', targetId: 'shell:env' },
    { section: 'cursor', label: 'Cursor Shape', keywords: 'cursor shape block beam underline', targetId: 'cursor:shape' },
    { section: 'cursor', label: 'Cursor Blinking', keywords: 'cursor blinking blink', targetId: 'cursor:blinking' },
    { section: 'cursor', label: 'Vi Mode Override', keywords: 'cursor vi mode vim modal', targetId: 'cursor:vi-mode' },
    { section: 'plugins', label: 'Enable Plugins', keywords: 'plugins enable disable master switch', targetId: 'plugins:enabled' },
    { section: 'plugins', label: 'Plugin Types', keywords: 'lua java plugins plugin types', targetId: 'plugins:types' },
    { section: 'plugins', label: 'Extra Search Paths', keywords: 'plugin paths search paths folders directories', targetId: 'plugins:search-paths' },
    { section: 'plugins', label: 'Installed Plugins', keywords: 'installed plugins rescan enable disable permissions', targetId: 'plugins:installed' },
    { section: 'advanced', label: 'Check for Updates', keywords: 'updates startup update checks', targetId: 'advanced:check-for-updates' },
    { section: 'advanced', label: 'Initial Window Size', keywords: 'window size columns lines defaults', targetId: 'advanced:window-size' },
    { section: 'advanced', label: 'UI Chrome Font Sizes', keywords: 'ui chrome font sizes density text size list normal small', targetId: 'advanced:ui-chrome-font-sizes' },
  ];

  global.termlabSettingsFeatureConstants = {
    SECTION_DEFS,
    SETTINGS_SEARCH_INDEX,
  };
})(window);
