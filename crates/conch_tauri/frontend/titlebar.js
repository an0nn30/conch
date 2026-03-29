// Custom VS Code-style titlebar with inline menus and window controls.
// Only active on Windows where native decorations are disabled.

(function (exports) {
  'use strict';

  let titlebarEl = null;
  let activeMenu = null; // currently open dropdown
  let hoverNavEnabled = false; // when a menu is open, hovering others opens them
  let menuActionHandler = null; // callback for menu item clicks

  // -----------------------------------------------------------------------
  // Menu definition — mirrors the native Tauri menu built in Rust.
  // Shortcuts are display-only; the native menu handles actual accelerators.
  // -----------------------------------------------------------------------
  function buildMenuDef(shortcuts) {
    const ctrl = 'Ctrl';
    return [
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
          { id: 'toggle-left-panel', label: 'Toggle File Explorer', shortcut: shortcuts.toggle_left_panel || '' },
          { id: 'toggle-right-panel', label: 'Toggle Sessions Panel', shortcut: shortcuts.toggle_right_panel || '' },
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
          { id: 'manage-tunnels', label: 'Manage SSH Tunnels\u2026', shortcut: `${ctrl}+Shift+T` },
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
  }

  // Format a shortcut string for display: "cmd+shift+t" -> "Ctrl+Shift+T"
  function formatShortcut(s) {
    if (!s) return '';
    return s
      .split('+')
      .map(part => {
        const p = part.trim().toLowerCase();
        if (p === 'cmd' || p === 'ctrl' || p === 'cmdorctrl') return 'Ctrl';
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
    } catch (_) {}
    shortcuts.zen_mode = zenShortcut;

    const el = createTitlebar(onAction);
    const app = document.getElementById('app');
    app.insertBefore(el, app.firstChild);

    // Build menu buttons
    const menuArea = el.querySelector('.titlebar-menu-area');
    const menuDef = buildMenuDef(shortcuts);
    for (const menu of menuDef) {
      const btn = document.createElement('button');
      btn.className = 'titlebar-menu-btn';
      btn.textContent = menu.label;
      btn.dataset.menu = menu.label;
      btn.addEventListener('mousedown', (e) => {
        e.preventDefault();
        e.stopPropagation();
        if (activeMenu && activeMenu.dataset.owner === menu.label) {
          closeAllMenus();
        } else {
          openDropdown(btn, menu);
        }
      });
      btn.addEventListener('mouseenter', () => {
        if (hoverNavEnabled && activeMenu && activeMenu.dataset.owner !== menu.label) {
          openDropdown(btn, menu);
        }
      });
      menuArea.appendChild(btn);
    }

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

    // Register keyboard accelerators since native menu can't provide them
    // when decorations are hidden.
    registerAccelerators(menuDef);
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
      if (item.type === 'separator') {
        const sep = document.createElement('div');
        sep.className = 'titlebar-dropdown-sep';
        dropdown.appendChild(sep);
        continue;
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
          const sr = document.createElement('div');
          sr.className = 'titlebar-dropdown-item';
          sr.textContent = si.label;
          sr.addEventListener('click', () => { closeAllMenus(); handleItemClick(si.id); });
          sub.appendChild(sr);
        }
        row.appendChild(sub);
        dropdown.appendChild(row);
        continue;
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
      dropdown.appendChild(row);
    }

    // Position below the button
    const rect = btnEl.getBoundingClientRect();
    dropdown.style.left = rect.left + 'px';
    dropdown.style.top = rect.bottom + 'px';
    document.body.appendChild(dropdown);

    btnEl.classList.add('active');
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
  // object { ctrl, shift, alt, key }.
  function parseShortcut(str) {
    if (!str) return null;
    const parts = str.toLowerCase().split('+').map(s => s.trim());
    const combo = { ctrl: false, shift: false, alt: false, key: '' };
    for (const p of parts) {
      if (p === 'cmd' || p === 'ctrl' || p === 'cmdorctrl') combo.ctrl = true;
      else if (p === 'shift') combo.shift = true;
      else if (p === 'alt') combo.alt = true;
      else combo.key = p;
    }
    return combo.key ? combo : null;
  }

  function matchesEvent(combo, e) {
    if (!combo) return false;
    if (combo.ctrl !== (e.ctrlKey || e.metaKey)) return false;
    if (combo.shift !== e.shiftKey) return false;
    if (combo.alt !== e.altKey) return false;
    // Normalize key comparison
    const eKey = e.key.toLowerCase();
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
    for (const menu of menuDef) {
      for (const item of menu.items) {
        if (item.type === 'separator' || !item.shortcut || item.noAccel) continue;
        const combo = parseShortcut(item.shortcut);
        if (combo) {
          bindings.push({ combo, id: item.id });
        }
      }
    }

    document.addEventListener('keydown', (e) => {
      for (const { combo, id } of bindings) {
        if (matchesEvent(combo, e)) {
          e.preventDefault();
          e.stopPropagation();
          handleItemClick(id);
          return;
        }
      }
    }, true); // capture phase so we fire before xterm.js
  }

  exports.titlebar = { init };
})(window);
