(function initTermLabShortcutRuntime(global) {
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
    let coreShortcutFallbacks = [];

    const coreShortcutActionByKey = {
      new_tab: 'new-tab',
      new_plain_shell_tab: 'new-plain-shell-tab',
      close_tab: 'close-tab',
      rename_tab: 'rename-tab',
      new_window: 'new-window',
      manage_tunnels: 'manage-tunnels',
      vault_open: 'vault-open',
      quit: null,
      zen_mode: 'zen-mode',
      toggle_left_panel: 'toggle-left-panel',
      toggle_right_panel: 'toggle-right-panel',
      toggle_bottom_panel: 'toggle-bottom-panel',
      split_vertical: 'split-vertical',
      split_horizontal: 'split-horizontal',
      close_pane: 'close-pane',
      navigate_pane_up: 'navigate-pane-up',
      navigate_pane_down: 'navigate-pane-down',
      navigate_pane_left: 'navigate-pane-left',
      navigate_pane_right: 'navigate-pane-right',
      settings: 'settings',
      new_file: 'new-file',
      save_file: 'save-file',
      // Scoped exactly like save_file (see the guard in runShortcutFallbacks):
      // in a terminal, cmd+shift+s must reach the shell.
      save_file_as: 'save-file-as',
      // Unlike save_file below, open_file is NOT dropped when a terminal pane
      // is focused: the chooser is how you get an editor in the first place,
      // so gating it on already having one would make it unreachable from a
      // fresh window. The cost is that cmd+o no longer reaches the shell.
      open_file: 'open-file',
      editor_completion: 'editor-completion',
      editor_signature_help: 'editor-signature-help',
      editor_go_to_definition: 'editor-go-to-definition',
      editor_navigate_back: 'editor-navigate-back',
      editor_navigate_forward: 'editor-navigate-forward',
      editor_next_problem: 'editor-next-problem',
      editor_previous_problem: 'editor-previous-problem',
    };

    // Core actions that mean something only inside a focused editor pane. A
    // hit on one of these with any other pane focused is DROPPED rather than
    // consumed — see runShortcutFallbacks.
    const EDITOR_SCOPED_ACTIONS = [
      'save-file',
      'save-file-as',
      'editor-completion',
      'editor-signature-help',
      'editor-go-to-definition',
      'editor-navigate-back',
      'editor-navigate-forward',
      'editor-next-problem',
      'editor-previous-problem',
    ];

    function runCoreAction(action) {
      if (action === 'navigate-pane-up') navigatePane('up');
      else if (action === 'navigate-pane-down') navigatePane('down');
      else if (action === 'navigate-pane-left') navigatePane('left');
      else if (action === 'navigate-pane-right') navigatePane('right');
      else if (action.startsWith('editor-')) {
        global.dispatchEvent(new global.CustomEvent(`termlab:${action}`));
      } else handleMenuAction(action);
    }

    function navigatePane(direction) {
      const tab = getActiveTab();
      const focusedPaneId = getFocusedPaneId();
      if (!tab || focusedPaneId == null) return;
      const adj = findAdjacentPane(focusedPaneId, direction, tab.containerEl);
      if (adj != null) setFocusedPane(adj);
    }

    function codeToKey(code) {
      if (!code) return '';
      if (/^Digit([0-9])$/.test(code)) return code[5];
      if (/^Key([A-Z])$/.test(code)) return code.slice(3).toLowerCase();
      const map = {
        Backquote: '`', Minus: '-', Equal: '=', BracketLeft: '[', Space: 'space',
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
        if (token === 'cmd' || token === 'cmdorctrl') {
          // "cmd" in config means the platform's primary modifier:
          // Meta on macOS, Ctrl on Windows/Linux.
          mods.add(isMacPlatform ? 'cmd' : 'ctrl');
        } else if (token === 'ctrl' || token === 'alt' || token === 'shift') {
          mods.add(token);
        } else {
          key = token;
        }
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
        const overrides = settings && settings.termlab && settings.termlab.keyboard
          ? (settings.termlab.keyboard.plugin_shortcuts || {})
          : {};
        const toolWindowOverrides = settings && settings.termlab && settings.termlab.keyboard
          ? (settings.termlab.keyboard.tool_window_shortcuts || {})
          : {};
        const keyboard = settings && settings.termlab ? (settings.termlab.keyboard || {}) : {};
        const pluginCtrlAltNext = [];
        const pluginAllNext = [];
        const toolWindowNext = [];
        const functionKeyNext = [];
        const coreNext = [];

        for (const [settingsKey, action] of Object.entries(coreShortcutActionByKey)) {
          if (!action) continue;
          const combo = normalizeShortcutString(keyboard[settingsKey]);
          if (!combo) continue;
          coreNext.push({ combo, action });
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
        coreShortcutFallbacks = coreNext;
      } catch (_) {
        pluginCtrlAltShortcutFallbacks = [];
        pluginAllShortcutFallbacks = [];
        toolWindowShortcutFallbacks = [];
        functionKeyShortcutFallbacks = [];
        coreShortcutFallbacks = [];
      }
    }

    function initListeners() {
      const keyboardRouter = global.termlabKeyboardRouter;
      const runShortcutFallbacks = (event) => {
        const combo = normalizeShortcutEventForPluginFallback(event);
        let coreHit = combo ? coreShortcutFallbacks.find((s) => s.combo === combo) : null;
        // Returning true consumes the keystroke. cmd+s (and cmd+shift+s) are a
        // save only inside an editor pane; in a terminal they must reach the
        // shell unchanged, so an EDITOR_SCOPED_ACTIONS binding is dropped while
        // a non-editor pane is focused. getCurrentPane() is the composed
        // paneManager.currentPane() (via managerDelegates) — the only accessor
        // that actually resolves the focused pane.
        //
        // Dropped rather than returned: the user may have bound the same combo
        // to a tool window or a plugin action, and those tables are consulted
        // further down. Returning here would swallow the keystroke for them too.
        // It has to be dropped from the function-key table as well, because
        // refreshKeyboardShortcutFallbacks pushes *every* core binding there,
        // function key or not — so a bare `coreHit = null` would only move the
        // save one table down. Which action was suppressed is remembered (not
        // just THAT one was), so dropping cmd+shift+s cannot also drop a
        // cmd+s the user has bound to something else in that table.
        let suppressedCoreAction = null;
        if (coreHit && EDITOR_SCOPED_ACTIONS.indexOf(coreHit.action) !== -1) {
          const pane = getCurrentPane();
          if (!pane || pane.kind !== 'editor') {
            suppressedCoreAction = coreHit.action;
            coreHit = null;
          }
        }
        if (coreHit) {
          runCoreAction(coreHit.action);
          return true;
        }

        if (isTextInputTarget(event.target)) return false;
        if (!combo) return false;
        const fKeyHit = functionKeyShortcutFallbacks.find((s) => s.combo === combo
          && !(suppressedCoreAction && s.kind === 'core' && s.action === suppressedCoreAction));
        if (fKeyHit) {
          if (fKeyHit.kind === 'core') {
            runCoreAction(fKeyHit.action);
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
          return true;
        }
        if (!isMacPlatform || !event.ctrlKey || !event.altKey || event.metaKey) {
          const toolWindowHit = toolWindowShortcutFallbacks.find((s) => s.combo === combo);
          if (toolWindowHit) {
            if (window.toolWindowManager) {
              window.toolWindowManager.toggle(toolWindowHit.windowId);
            }
            return true;
          }
          const allHit = pluginAllShortcutFallbacks.find((s) => s.combo === combo);
          if (allHit) {
            invoke('trigger_plugin_menu_action', {
              pluginName: allHit.plugin,
              action: allHit.action,
            }).catch(() => {});
            return true;
          }
          return false;
        }
        const hit = pluginCtrlAltShortcutFallbacks.find((s) => s.combo === combo);
        if (!hit) return false;
        invoke('trigger_plugin_menu_action', {
          pluginName: hit.plugin,
          action: hit.action,
        }).catch(() => {});
        return true;
      };

      const togglePaletteShortcut = (event) => {
        const key = (event.key || '').toLowerCase();
        const superPressed = isMacPlatform ? event.metaKey : (event.metaKey || event.ctrlKey);
        if (!superPressed || !event.shiftKey || key !== 'p') return false;
        if (isTextInputTarget(event.target)) return false;
        if (isCommandPaletteOpen()) closeCommandPalette();
        else openCommandPalette();
        return true;
      };

      const tabSwitchShortcut = (event) => {
        if (!(event.metaKey || event.ctrlKey) || event.key < '1' || event.key > '9') return false;
        const idx = parseInt(event.key, 10) - 1;
        const tabIds = getTabIds();
        if (idx < tabIds.length) activateTab(tabIds[idx]);
        return true;
      };

      const macAltArrowShortcut = (event) => {
        if (!isMacPlatform) return false;
        if (!event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return false;
        if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return false;
        if (isTextInputTarget(event.target) || isTextInputTarget(document.activeElement)) return false;
        const pane = getCurrentPane();
        if (!pane || pane.kind !== 'terminal' || !pane.term) return false;
        const seq = event.key === 'ArrowLeft' ? '\x1b[1;3D' : '\x1b[1;3C';
        writeTextToCurrentPane(seq);
        return true;
      };

      if (keyboardRouter && typeof keyboardRouter.register === 'function') {
        keyboardRouter.register({
          name: 'shortcut-debug-down',
          priority: 25,
          onKeyDown: (event) => {
            if (!shortcutDebugEnabled || !shouldDebugKeyEvent(event)) return false;
            console.log('[termlab-keydbg] keydown(capture)', formatKeyEventForDebug(event));
            return false;
          },
        });
        keyboardRouter.register({
          name: 'shortcut-debug-up',
          priority: 25,
          onKeyUp: (event) => {
            if (!shortcutDebugEnabled || !shouldDebugKeyEvent(event)) return false;
            console.log('[termlab-keydbg] keyup(capture)', formatKeyEventForDebug(event));
            return false;
          },
        });
        keyboardRouter.register({
          name: 'shortcut-fallbacks',
          priority: 120,
          onKeyDown: (event) => runShortcutFallbacks(event),
        });
        keyboardRouter.register({
          name: 'shortcut-palette-toggle',
          priority: 110,
          onKeyDown: (event) => togglePaletteShortcut(event),
        });
        keyboardRouter.register({
          name: 'shortcut-tab-switch',
          priority: 80,
          onKeyDown: (event) => tabSwitchShortcut(event),
        });
        keyboardRouter.register({
          name: 'shortcut-mac-alt-arrow',
          priority: 75,
          onKeyDown: (event) => macAltArrowShortcut(event),
        });
        return;
      }
      console.warn('shortcut-runtime: keyboard router unavailable, shortcut handlers were not registered');
    }

    async function init() {
      initListeners();
      await refreshKeyboardShortcutFallbacks();
      global.__termlabRefreshKeyboardShortcutFallbacks = refreshKeyboardShortcutFallbacks;
      return {
        refreshKeyboardShortcutFallbacks,
      };
    }

    return {
      init,
      refreshKeyboardShortcutFallbacks,
    };
  }

  global.termlabShortcutRuntime = {
    create,
  };
})(window);
