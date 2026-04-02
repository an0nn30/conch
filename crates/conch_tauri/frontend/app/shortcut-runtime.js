(function initConchShortcutRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const isMacPlatform = deps.isMacPlatform;
    const isTextInputTarget = deps.isTextInputTarget;
    const handleMenuAction = deps.handleMenuAction;
    const shouldDebugKeyEvent = deps.shouldDebugKeyEvent;
    const formatKeyEventForDebug = deps.formatKeyEventForDebug;
    const shortcutDebugEnabled = deps.shortcutDebugEnabled;
    const openCommandPalette = deps.openCommandPalette;
    const closeCommandPalette = deps.closeCommandPalette;
    const isCommandPaletteOpen = deps.isCommandPaletteOpen;
    const getTabIds = deps.getTabIds;
    const activateTab = deps.activateTab;
    const getCurrentPane = deps.getCurrentPane;
    const writeTextToCurrentPane = deps.writeTextToCurrentPane;
    const getActiveTab = deps.getActiveTab;
    const getFocusedPaneId = deps.getFocusedPaneId;
    const setFocusedPane = deps.setFocusedPane;
    const findAdjacentPane = deps.findAdjacentPane;

    let pluginCtrlAltShortcutFallbacks = [];
    let pluginAllShortcutFallbacks = [];
    let toolWindowShortcutFallbacks = [];
    let functionKeyShortcutFallbacks = [];

    const coreShortcutActionByKey = {
      new_tab: 'new-tab',
      close_tab: 'close-tab',
      rename_tab: 'rename-tab',
      new_window: 'new-window',
      manage_tunnels: 'manage-tunnels',
      quit: null,
      zen_mode: 'zen-mode',
      toggle_left_panel: 'toggle-left-panel',
      toggle_right_panel: 'toggle-right-panel',
      toggle_bottom_panel: 'toggle-bottom-panel',
      split_vertical: 'split-vertical',
      split_horizontal: 'split-horizontal',
      close_pane: 'close-pane',
      settings: 'settings',
    };

    function codeToKey(code) {
      if (!code) return '';
      if (/^Digit([0-9])$/.test(code)) return code[5];
      if (/^Key([A-Z])$/.test(code)) return code.slice(3).toLowerCase();
      const map = {
        Backquote: '`', Minus: '-', Equal: '=', BracketLeft: '[',
        BracketRight: ']', Backslash: '\\', Semicolon: ';', Quote: "'",
        Comma: ',', Period: '.', Slash: '/',
      };
      if (map[code]) return map[code];
      return '';
    }

    function normalizeShortcutEventForPluginFallback(event) {
      const parts = [];
      if (event.metaKey) parts.push('cmd');
      if (event.ctrlKey) parts.push('ctrl');
      if (event.altKey) parts.push('alt');
      if (event.shiftKey) parts.push('shift');
      const key = codeToKey(event.code) || String(event.key || '').toLowerCase();
      if (!key || ['meta', 'control', 'alt', 'shift'].includes(key)) return null;
      parts.push(key);
      return parts.join('+');
    }

    function normalizeShortcutString(raw) {
      const text = String(raw || '').trim().toLowerCase();
      if (!text) return '';
      const tokens = text.split('+').map((t) => t.trim()).filter(Boolean);
      if (tokens.length === 0) return '';
      const mods = new Set();
      let key = '';
      for (const token of tokens) {
        if (token === 'cmd' || token === 'ctrl' || token === 'alt' || token === 'shift') mods.add(token);
        else key = token;
      }
      if (!key) return '';
      const ordered = [];
      if (mods.has('cmd')) ordered.push('cmd');
      if (mods.has('ctrl')) ordered.push('ctrl');
      if (mods.has('alt')) ordered.push('alt');
      if (mods.has('shift')) ordered.push('shift');
      ordered.push(key);
      return ordered.join('+');
    }

    function isFunctionKeyCombo(combo) {
      return /^((cmd|ctrl|alt|shift)\+)*f([1-9]|1[0-9]|2[0-4])$/.test(combo);
    }

    async function refreshKeyboardShortcutFallbacks() {
      try {
        const [settings, pluginItems] = await Promise.all([
          invoke('get_all_settings'),
          invoke('get_plugin_menu_items').catch(() => []),
        ]);
        const overrides = settings && settings.conch && settings.conch.keyboard
          ? (settings.conch.keyboard.plugin_shortcuts || {})
          : {};
        const toolWindowOverrides = settings && settings.conch && settings.conch.keyboard
          ? (settings.conch.keyboard.tool_window_shortcuts || {})
          : {};
        const keyboard = settings && settings.conch ? (settings.conch.keyboard || {}) : {};
        const pluginCtrlAltNext = [];
        const pluginAllNext = [];
        const toolWindowNext = [];
        const functionKeyNext = [];

        for (const [settingsKey, action] of Object.entries(coreShortcutActionByKey)) {
          if (!action) continue;
          const combo = normalizeShortcutString(keyboard[settingsKey]);
          if (!combo) continue;
          functionKeyNext.push({ combo, kind: 'core', action });
        }

        const byPluginAction = new Map();
        for (const item of (pluginItems || [])) {
          if (!item || !item.plugin || !item.action) continue;
          const uniqueKey = `${item.plugin}:${item.action}`;
          if (byPluginAction.has(uniqueKey)) continue;
          byPluginAction.set(uniqueKey, item);
        }
        for (const item of byPluginAction.values()) {
          const overrideKey = `${item.plugin}:${item.action}`;
          const raw = Object.prototype.hasOwnProperty.call(overrides, overrideKey)
            ? overrides[overrideKey]
            : item.keybind;
          const combo = normalizeShortcutString(raw);
          if (!combo) continue;
          if (isFunctionKeyCombo(combo)) {
            functionKeyNext.push({ combo, kind: 'plugin', plugin: item.plugin, action: item.action });
          }
          if (isMacPlatform && combo.includes('ctrl') && combo.includes('alt') && !combo.includes('cmd')) {
            pluginCtrlAltNext.push({ combo, plugin: item.plugin, action: item.action });
          }
          pluginAllNext.push({ combo, plugin: item.plugin, action: item.action });
        }

        const twm = window.toolWindowManager;
        const toolWindows = twm && typeof twm.listWindows === 'function'
          ? twm.listWindows()
          : [];
        for (const item of toolWindows) {
          if (!item || !item.id) continue;
          const combo = normalizeShortcutString(toolWindowOverrides[item.id]);
          if (!combo) continue;
          if (isFunctionKeyCombo(combo)) {
            functionKeyNext.push({ combo, kind: 'tool-window', windowId: item.id });
          }
          toolWindowNext.push({ combo, windowId: item.id });
        }

        pluginCtrlAltShortcutFallbacks = pluginCtrlAltNext;
        pluginAllShortcutFallbacks = pluginAllNext;
        toolWindowShortcutFallbacks = toolWindowNext;
        functionKeyShortcutFallbacks = functionKeyNext;
      } catch (_) {
        pluginCtrlAltShortcutFallbacks = [];
        pluginAllShortcutFallbacks = [];
        toolWindowShortcutFallbacks = [];
        functionKeyShortcutFallbacks = [];
      }
    }

    function initListeners() {
      document.addEventListener('keydown', (event) => {
        if (!shortcutDebugEnabled || !shouldDebugKeyEvent(event)) return;
        console.log('[conch-keydbg] keydown(capture)', formatKeyEventForDebug(event));
      }, true);
      document.addEventListener('keyup', (event) => {
        if (!shortcutDebugEnabled || !shouldDebugKeyEvent(event)) return;
        console.log('[conch-keydbg] keyup(capture)', formatKeyEventForDebug(event));
      }, true);

      document.addEventListener('keydown', (event) => {
        if (isTextInputTarget(event.target)) return;
        const combo = normalizeShortcutEventForPluginFallback(event);
        if (!combo) return;
        const fKeyHit = functionKeyShortcutFallbacks.find((s) => s.combo === combo);
        if (fKeyHit) {
          event.preventDefault();
          event.stopPropagation();
          if (fKeyHit.kind === 'core') {
            handleMenuAction(fKeyHit.action);
          } else if (fKeyHit.kind === 'tool-window') {
            if (window.toolWindowManager) {
              window.toolWindowManager.toggle(fKeyHit.windowId);
            }
          } else {
            invoke('trigger_plugin_menu_action', {
              pluginName: fKeyHit.plugin,
              action: fKeyHit.action,
            }).catch(() => {});
          }
          return;
        }
        if (!isMacPlatform || !event.ctrlKey || !event.altKey || event.metaKey) {
          const toolWindowHit = toolWindowShortcutFallbacks.find((s) => s.combo === combo);
          if (toolWindowHit) {
            event.preventDefault();
            event.stopPropagation();
            if (window.toolWindowManager) {
              window.toolWindowManager.toggle(toolWindowHit.windowId);
            }
            return;
          }
          const allHit = pluginAllShortcutFallbacks.find((s) => s.combo === combo);
          if (allHit) {
            event.preventDefault();
            event.stopPropagation();
            invoke('trigger_plugin_menu_action', {
              pluginName: allHit.plugin,
              action: allHit.action,
            }).catch(() => {});
          }
          return;
        }
        const hit = pluginCtrlAltShortcutFallbacks.find((s) => s.combo === combo);
        if (!hit) return;
        event.preventDefault();
        event.stopPropagation();
        invoke('trigger_plugin_menu_action', {
          pluginName: hit.plugin,
          action: hit.action,
        }).catch(() => {});
      }, true);

      document.addEventListener('keydown', (event) => {
        const key = (event.key || '').toLowerCase();
        const superPressed = isMacPlatform ? event.metaKey : (event.metaKey || event.ctrlKey);
        if (!superPressed || !event.shiftKey || key !== 'p') return;
        if (isTextInputTarget(event.target)) return;
        event.preventDefault();
        event.stopPropagation();
        if (isCommandPaletteOpen()) closeCommandPalette();
        else openCommandPalette();
      }, true);

      document.addEventListener('keydown', (event) => {
        if ((event.metaKey || event.ctrlKey) && event.key >= '1' && event.key <= '9') {
          event.preventDefault();
          const idx = parseInt(event.key, 10) - 1;
          const tabIds = getTabIds();
          if (idx < tabIds.length) activateTab(tabIds[idx]);
        }
      });

      document.addEventListener('keydown', (event) => {
        if (!isMacPlatform) return;
        if (!event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return;
        if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
        if (isTextInputTarget(event.target) || isTextInputTarget(document.activeElement)) return;
        const pane = getCurrentPane();
        if (!pane || pane.kind !== 'terminal' || !pane.term) return;
        event.preventDefault();
        event.stopPropagation();
        const seq = event.key === 'ArrowLeft' ? '\x1b[1;3D' : '\x1b[1;3C';
        writeTextToCurrentPane(seq);
      }, true);

      document.addEventListener('keydown', (event) => {
        if ((event.metaKey || event.ctrlKey) && event.altKey && ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(event.key)) {
          event.preventDefault();
          event.stopPropagation();
          const dir = event.key.replace('Arrow', '').toLowerCase();
          const tab = getActiveTab();
          const focusedPaneId = getFocusedPaneId();
          if (!tab || focusedPaneId == null) return;
          const adj = findAdjacentPane(focusedPaneId, dir, tab.containerEl);
          if (adj != null) setFocusedPane(adj);
        }
      }, true);
    }

    async function init() {
      initListeners();
      await refreshKeyboardShortcutFallbacks();
      global.__conchRefreshKeyboardShortcutFallbacks = refreshKeyboardShortcutFallbacks;
      return {
        refreshKeyboardShortcutFallbacks,
      };
    }

    return {
      init,
      refreshKeyboardShortcutFallbacks,
    };
  }

  global.conchShortcutRuntime = {
    create,
  };
})(window);
