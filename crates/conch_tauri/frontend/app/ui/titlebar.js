// Custom VS Code-style titlebar with inline menus and window controls.
// Only active on Windows where native decorations are disabled.

(function (exports) {
  'use strict';

  const isMacPlatform = typeof navigator !== 'undefined' && navigator.platform.includes('Mac');

  let titlebarEl = null;
  let activeMenu = null; // currently open dropdown
  let hoverNavEnabled = false; // when a menu is open, hovering others opens them
  let menuActionHandler = null; // callback for menu item clicks
  let menuAreaEl = null;
  let menuDefState = [];
  let shortcutsState = {};
  let debugBuildState = false;
  let acceleratorHandler = null;

  // -----------------------------------------------------------------------
  // Menu definition — mirrors the native Tauri menu built in Rust.
  // Shortcuts are display-only; the native menu handles actual accelerators.
  // -----------------------------------------------------------------------
  function buildPluginSubmenu(pluginItems) {
    if (!Array.isArray(pluginItems) || pluginItems.length === 0) return null;
    const byPlugin = new Map();
    for (const item of pluginItems) {
      if (!item || !item.plugin || !item.action) continue;
      if (!byPlugin.has(item.plugin)) byPlugin.set(item.plugin, []);
      byPlugin.get(item.plugin).push(item);
    }
    const pluginNames = Array.from(byPlugin.keys()).sort((a, b) =>
      a.toLowerCase().localeCompare(b.toLowerCase()),
    );
    const pluginEntries = [];
    for (const pluginName of pluginNames) {
      const items = byPlugin.get(pluginName) || [];
      items.sort((a, b) => String(a.label || '').toLowerCase().localeCompare(String(b.label || '').toLowerCase()));
      pluginEntries.push({
        label: pluginName,
        submenu: items.map((it) => ({
          id: `plugin.${it.plugin}.${it.action}`,
          label: it.label || it.action,
          shortcut: it.keybind || '',
        })),
      });
    }
    if (pluginEntries.length === 0) return null;
    return { label: 'Plugins', submenu: pluginEntries };
  }

  function buildMenuDef(shortcuts, debugBuild, pluginItems) {
    const ctrl = isMacPlatform ? 'Cmd' : 'Ctrl';
    const menus = [
      {
        label: 'File', items: [
          { id: 'new-tab', label: 'New Tab', shortcut: `${ctrl}+T` },
          { id: 'new-window', label: 'New Window', shortcut: `${ctrl}+Shift+N` },
          { type: 'separator' },
          { label: 'SSH Manager', submenu: [
            { id: 'ssh-export', label: 'Export' },
            { id: 'ssh-import', label: 'Import' },
          ]},
          { type: 'separator' },
          { id: 'settings', label: 'Settings\u2026', shortcut: `${ctrl}+,` },
          { type: 'separator' },
          { id: 'rename-tab', label: 'Rename Tab', shortcut: shortcuts.rename_tab || 'F2' },
          { id: 'close-tab', label: 'Close Tab', shortcut: `${ctrl}+W` },
          { id: 'close-window', label: 'Close Window' },
        ]
      },
      {
        label: 'Edit', items: [
          { id: 'cut', label: 'Cut', shortcut: `${ctrl}+X`, noAccel: true },
          { id: 'copy', label: 'Copy', shortcut: `${ctrl}+C`, noAccel: true },
          { id: 'paste', label: 'Paste', shortcut: `${ctrl}+V`, noAccel: true },
          { id: 'select-all', label: 'Select All', shortcut: `${ctrl}+A`, noAccel: true },
        ]
      },
      {
        label: 'View', items: [
          { id: 'toggle-left-panel', label: 'Toggle Left Panel', shortcut: shortcuts.toggle_left_panel || '' },
          { id: 'toggle-right-panel', label: 'Toggle Right Panel', shortcut: shortcuts.toggle_right_panel || '' },
          { id: 'toggle-bottom-panel', label: 'Toggle Bottom Panel', shortcut: shortcuts.toggle_bottom_panel || '' },
          { type: 'separator' },
          { id: 'split-vertical', label: 'Split Pane Vertically', shortcut: shortcuts.split_vertical || `${ctrl}+D` },
          { id: 'split-horizontal', label: 'Split Pane Horizontally', shortcut: shortcuts.split_horizontal || `${ctrl}+Shift+D` },
          { id: 'close-pane', label: 'Close Pane', shortcut: shortcuts.close_pane || `${ctrl}+Shift+W` },
          { type: 'separator' },
          { id: 'focus-sessions', label: 'Toggle & Focus Sessions', shortcut: `${ctrl}+/` },
          { id: 'zen-mode', label: 'Zen Mode', shortcut: shortcuts.zen_mode || '' },
          { type: 'separator' },
          { id: 'zoom-in', label: 'Zoom In', shortcut: `${ctrl}+=` },
          { id: 'zoom-out', label: 'Zoom Out', shortcut: `${ctrl}+-` },
          { id: 'zoom-reset', label: 'Reset Zoom', shortcut: `${ctrl}+0` },
        ]
      },
      {
        label: 'Tools', items: [
          { id: 'open-command-palette', label: 'Plugin Commands\u2026', shortcut: `${ctrl}+Shift+P` },
          { type: 'separator' },
          { id: 'manage-tunnels', label: 'Manage SSH Tunnels\u2026', shortcut: shortcuts.manage_tunnels || `${ctrl}+Shift+T` },
        ]
      },
      {
        label: 'Window', items: [
          { id: 'win-minimize', label: 'Minimize' },
          { id: 'win-maximize', label: 'Maximize' },
          { type: 'separator' },
          { id: 'win-fullscreen', label: 'Fullscreen' },
        ]
      },
      {
        label: 'Help', items: [
          { id: 'check-for-updates', label: 'Check for Updates\u2026' },
          { type: 'separator' },
          { id: 'about', label: 'About Conch' },
        ]
      },
    ];
    if (debugBuild) {
      menus.splice(5, 0, {
        label: 'Debug', items: [
          { id: 'open-devtools', label: 'Open Developer Console', shortcut: 'F12' },
        ]
      });
    }
    const toolsMenu = menus.find((m) => m.label === 'Tools');
    const pluginSubmenu = buildPluginSubmenu(pluginItems);
    if (toolsMenu && pluginSubmenu) {
      toolsMenu.items.push({ type: 'separator' });
      toolsMenu.items.push(pluginSubmenu);
    }
    return menus;
  }

  // Format a shortcut string for display: "cmd+shift+t" -> "Cmd+Shift+T"
  function formatShortcut(s) {
    if (!s) return '';
    return s
      .split('+')
      .map(part => {
        const p = part.trim().toLowerCase();
        if (p === 'cmd') return isMacPlatform ? 'Cmd' : 'Ctrl';
        if (p === 'cmdorctrl') return isMacPlatform ? 'Cmd' : 'Ctrl';
        if (p === 'ctrl') return 'Ctrl';
        if (p === 'shift') return 'Shift';
        if (p === 'alt') return 'Alt';
        return p.charAt(0).toUpperCase() + p.slice(1);
      })
      .join('+');
  }

  // -----------------------------------------------------------------------
  // DOM construction
  // -----------------------------------------------------------------------
  function createTitlebar(onAction) {
    menuActionHandler = onAction;

    titlebarEl = document.createElement('div');
    titlebarEl.id = 'custom-titlebar';
    titlebarEl.innerHTML = `
      <div class="titlebar-menu-area"></div>
      <div class="titlebar-drag" data-tauri-drag-region>
        <span class="titlebar-title" data-tauri-drag-region>Conch</span>
      </div>
      <div class="titlebar-controls">
        <button class="titlebar-btn titlebar-btn-minimize" aria-label="Minimize">
          <svg width="10" height="1" viewBox="0 0 10 1"><rect width="10" height="1" fill="currentColor"/></svg>
        </button>
        <button class="titlebar-btn titlebar-btn-maximize" aria-label="Maximize">
          <svg width="10" height="10" viewBox="0 0 10 10"><rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1"/></svg>
        </button>
        <button class="titlebar-btn titlebar-btn-close" aria-label="Close">
          <svg width="10" height="10" viewBox="0 0 10 10">
            <line x1="0" y1="0" x2="10" y2="10" stroke="currentColor" stroke-width="1.2"/>
            <line x1="10" y1="0" x2="0" y2="10" stroke="currentColor" stroke-width="1.2"/>
          </svg>
        </button>
      </div>
    `;
    return titlebarEl;
  }

  async function init(onAction) {
    const tauri = window.__TAURI__;
    if (!tauri) return;
    const invoke = tauri.core.invoke;

    // Fetch shortcuts for display labels
    let shortcuts = {};
    try {
      shortcuts = await invoke('get_keyboard_shortcuts');
    } catch (_) {}

    let zenShortcut = '';
    try {
      const cfg = await invoke('get_app_config');
      zenShortcut = cfg.zen_mode_shortcut || '';
      debugBuildState = !!cfg.debug_build;
    } catch (_) {}
    shortcutsState = { ...shortcuts, zen_mode: zenShortcut };

    const el = createTitlebar(onAction);
    const app = document.getElementById('app');
    app.insertBefore(el, app.firstChild);

    // Build menu buttons
    menuAreaEl = el.querySelector('.titlebar-menu-area');
    await refresh();

    // Window control buttons
    const win = tauri.window.getCurrentWindow();
    el.querySelector('.titlebar-btn-minimize').addEventListener('click', () => win.minimize());
    el.querySelector('.titlebar-btn-maximize').addEventListener('click', async () => {
      if (await win.isMaximized()) {
        win.unmaximize();
      } else {
        win.maximize();
      }
    });
    el.querySelector('.titlebar-btn-close').addEventListener('click', () => win.close());

    // Close menus when clicking outside the titlebar or the open dropdown.
    document.addEventListener('mousedown', (e) => {
      if (activeMenu && !titlebarEl.contains(e.target) && !activeMenu.contains(e.target)) {
        closeAllMenus();
      }
    });

    // Close menus on Escape
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && activeMenu) {
        closeAllMenus();
        e.preventDefault();
        e.stopPropagation();
      }
    }, true);

  }

  function renderMenuButtons() {
    if (!menuAreaEl) return;
    menuAreaEl.innerHTML = '';
    for (const menu of menuDefState) {
      const btn = document.createElement('button');
      btn.className = 'titlebar-menu-btn';
      btn.textContent = menu.label;
      btn.dataset.menu = menu.label;
      btn.addEventListener('mousedown', (e) => {
        e.preventDefault();
        e.stopPropagation();
        const menuNow = menuDefState.find((m) => m.label === menu.label) || menu;
        if (activeMenu && activeMenu.dataset.owner === menu.label) {
          closeAllMenus();
        } else {
          openDropdown(btn, menuNow);
        }
      });
      btn.addEventListener('mouseenter', () => {
        if (hoverNavEnabled && activeMenu && activeMenu.dataset.owner !== menu.label) {
          const menuNow = menuDefState.find((m) => m.label === menu.label) || menu;
          openDropdown(btn, menuNow);
        }
      });
      menuAreaEl.appendChild(btn);
    }
  }

  async function refresh() {
    const tauri = window.__TAURI__;
    if (!tauri || !tauri.core || !tauri.core.invoke) return;
    let pluginItems = [];
    try {
      pluginItems = await tauri.core.invoke('get_plugin_menu_items');
    } catch (_) {}
    menuDefState = buildMenuDef(shortcutsState, debugBuildState, pluginItems);
    closeAllMenus();
    renderMenuButtons();
    // Register keyboard accelerators since native menu can't provide them
    // when decorations are hidden.
    registerAccelerators(menuDefState);
  }

  // -----------------------------------------------------------------------
  // Dropdown logic
  // -----------------------------------------------------------------------
  function openDropdown(btnEl, menuDef) {
    closeAllMenus();
    hoverNavEnabled = true;

    const dropdown = document.createElement('div');
    dropdown.className = 'titlebar-dropdown';
    dropdown.dataset.owner = menuDef.label;
    activeMenu = dropdown;

    for (const item of menuDef.items) {
      appendDropdownItem(dropdown, item);
    }

    // Position below the button
    const rect = btnEl.getBoundingClientRect();
    dropdown.style.left = rect.left + 'px';
    dropdown.style.top = rect.bottom + 'px';
    document.body.appendChild(dropdown);

    btnEl.classList.add('active');
  }

  function appendDropdownItem(container, item) {
    if (item.type === 'separator') {
      const sep = document.createElement('div');
      sep.className = 'titlebar-dropdown-sep';
      container.appendChild(sep);
      return;
    }
    if (item.submenu) {
      const row = document.createElement('div');
      row.className = 'titlebar-dropdown-item titlebar-submenu-parent';
      const labelSpan = document.createElement('span');
      labelSpan.className = 'titlebar-dropdown-label';
      labelSpan.textContent = item.label;
      row.appendChild(labelSpan);
      const arrow = document.createElement('span');
      arrow.className = 'titlebar-dropdown-shortcut';
      arrow.textContent = '\u25B8';
      row.appendChild(arrow);

      const sub = document.createElement('div');
      sub.className = 'titlebar-dropdown titlebar-submenu';
      for (const si of item.submenu) {
        appendDropdownItem(sub, si);
      }
      row.appendChild(sub);
      container.appendChild(row);
      return;
    }
    const row = document.createElement('div');
    row.className = 'titlebar-dropdown-item';
    const labelSpan = document.createElement('span');
    labelSpan.className = 'titlebar-dropdown-label';
    labelSpan.textContent = item.label;
    row.appendChild(labelSpan);
    if (item.shortcut) {
      const keySpan = document.createElement('span');
      keySpan.className = 'titlebar-dropdown-shortcut';
      keySpan.textContent = formatShortcut(item.shortcut);
      row.appendChild(keySpan);
    }
    row.addEventListener('click', () => {
      closeAllMenus();
      handleItemClick(item.id);
    });
    container.appendChild(row);
  }

  function closeAllMenus() {
    if (activeMenu) {
      activeMenu.remove();
      activeMenu = null;
    }
    hoverNavEnabled = false;
    if (titlebarEl) {
      titlebarEl.querySelectorAll('.titlebar-menu-btn.active').forEach(b => b.classList.remove('active'));
    }
  }

  function handleItemClick(id) {
    const tauri = window.__TAURI__;

    if (typeof id === 'string' && id.startsWith('plugin.')) {
      const parts = id.split('.');
      if (parts.length >= 3) {
        const pluginName = parts[1];
        const action = parts.slice(2).join('.');
        tauri.core.invoke('trigger_plugin_menu_action', { pluginName, action }).catch(() => {});
        return;
      }
    }

    // Window actions handled directly via Tauri window API
    if (id === 'win-minimize') {
      tauri.window.getCurrentWindow().minimize();
      return;
    }
    if (id === 'win-maximize') {
      const win = tauri.window.getCurrentWindow();
      win.isMaximized().then(m => m ? win.unmaximize() : win.maximize());
      return;
    }
    if (id === 'win-fullscreen') {
      const win = tauri.window.getCurrentWindow();
      win.isFullscreen().then(f => win.setFullscreen(!f));
      return;
    }
    if (id === 'close-window') {
      tauri.window.getCurrentWindow().close();
      return;
    }

    // Everything else goes through the menu action handler
    if (menuActionHandler) {
      menuActionHandler(id);
    }
  }

  // -----------------------------------------------------------------------
  // Keyboard accelerators — since the native menu is hidden on Windows,
  // we must handle shortcuts in JS.
  // -----------------------------------------------------------------------

  // Parse a config-style shortcut string ("cmd+shift+z") into a matcher
  // object { cmd, ctrl, shift, alt, key }.
  function parseShortcut(str) {
    if (!str) return null;
    const parts = str.toLowerCase().split('+').map(s => s.trim());
    const combo = { cmd: false, ctrl: false, shift: false, alt: false, key: '' };
    for (const p of parts) {
      if (p === 'cmd') combo.cmd = true;
      else if (p === 'ctrl') combo.ctrl = true;
      else if (p === 'cmdorctrl') {
        if (isMacPlatform) combo.cmd = true;
        else combo.ctrl = true;
      }
      else if (p === 'shift') combo.shift = true;
      else if (p === 'alt') combo.alt = true;
      else combo.key = p;
    }
    return combo.key ? combo : null;
  }

  // Map e.code (physical key) to a plain key name so that Shift+digit
  // combos resolve to the digit rather than the shifted character.
  function codeToKey(code) {
    if (!code) return '';
    if (/^Digit([0-9])$/.test(code)) return code[5];
    if (/^Key([A-Z])$/.test(code)) return code.slice(3).toLowerCase();
    const map = {
      Backquote: '`', Minus: '-', Equal: '=', BracketLeft: '[',
      BracketRight: ']', Backslash: '\\', Semicolon: ';', Quote: "'",
      Comma: ',', Period: '.', Slash: '/',
    };
    return map[code] || '';
  }

  function matchesEvent(combo, e) {
    if (!combo) return false;
    if (combo.cmd !== !!e.metaKey) return false;
    if (combo.ctrl !== !!e.ctrlKey) return false;
    if (combo.shift !== e.shiftKey) return false;
    if (combo.alt !== e.altKey) return false;
    // Use e.code (physical key) so Shift+9 matches '9' not '('.
    const eKey = codeToKey(e.code) || e.key.toLowerCase();
    const cKey = combo.key;
    // Handle special names
    if (cKey === '/' && eKey === '/') return true;
    if (cKey === '=' && (eKey === '=' || eKey === '+')) return true;
    if (cKey === '-' && (eKey === '-' || eKey === '_')) return true;
    if (cKey === '0' && eKey === '0') return true;
    return eKey === cKey;
  }

  function registerAccelerators(menuDef) {
    // Collect all shortcut->action bindings from the menu definition.
    const bindings = [];
    function collect(items) {
      for (const item of items || []) {
        if (item.type === 'separator') continue;
        if (item.submenu) {
          collect(item.submenu);
          continue;
        }
        if (!item.shortcut || item.noAccel) continue;
        const combo = parseShortcut(item.shortcut);
        if (combo && item.id) {
          bindings.push({ combo, id: item.id });
        }
      }
    }
    for (const menu of menuDef) collect(menu.items);

    if (acceleratorHandler) {
      document.removeEventListener('keydown', acceleratorHandler, true);
      acceleratorHandler = null;
    }
    acceleratorHandler = (e) => {
      for (const { combo, id } of bindings) {
        if (matchesEvent(combo, e)) {
          e.preventDefault();
          e.stopPropagation();
          handleItemClick(id);
          return;
        }
      }
    };
    document.addEventListener('keydown', acceleratorHandler, true); // capture phase so we fire before xterm.js
  }

  exports.titlebar = { init, refresh };
})(window);
