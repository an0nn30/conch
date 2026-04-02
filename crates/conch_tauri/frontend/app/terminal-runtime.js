(function initConchTerminalRuntime(global) {
  function create(deps) {
    const tauriOpen = deps.tauriOpen;
    const getTheme = deps.getTheme;
    const getFontFamily = deps.getFontFamily;
    const getFontSize = deps.getFontSize;
    const getCursorStyle = deps.getCursorStyle;
    const getCursorBlink = deps.getCursorBlink;
    const getScrollSensitivity = deps.getScrollSensitivity;
    const isShortcutDebugEnabled = deps.isShortcutDebugEnabled;

    function formatKeyEventForDebug(event) {
      return JSON.stringify({
        type: event.type,
        key: event.key,
        code: event.code,
        keyCode: event.keyCode,
        which: event.which,
        altKey: event.altKey,
        ctrlKey: event.ctrlKey,
        metaKey: event.metaKey,
        shiftKey: event.shiftKey,
        repeat: event.repeat,
        defaultPrevented: event.defaultPrevented,
        targetTag: event.target && event.target.tagName ? event.target.tagName : null,
        targetClass: event.target && event.target.className ? String(event.target.className) : null,
        activeTag: document.activeElement && document.activeElement.tagName ? document.activeElement.tagName : null,
        activeClass: document.activeElement && document.activeElement.className ? String(document.activeElement.className) : null,
      });
    }

    function toDebugHex(str) {
      const out = [];
      for (let i = 0; i < str.length; i++) {
        out.push(str.charCodeAt(i).toString(16).padStart(2, '0'));
      }
      return out.join(' ');
    }

    function toDebugEscaped(str) {
      return String(str)
        .replace(/\x1b/g, '\\x1b')
        .replace(/\r/g, '\\r')
        .replace(/\n/g, '\\n')
        .replace(/\t/g, '\\t');
    }

    function shouldDebugKeyEvent(event) {
      if (!event) return false;
      const key = String(event.key || '');
      return event.altKey || event.ctrlKey || event.metaKey || key.startsWith('Arrow') || key === 'Escape' || /^F\d{1,2}$/.test(key);
    }

    function terminalMouseModeIsActive(term) {
      const modes = term.modes;
      if (!modes || typeof modes.mouseTrackingMode !== 'string') {
        return false;
      }
      return modes.mouseTrackingMode !== 'none';
    }

    function setupTmuxRightClickBridge(term, terminalRoot) {
      let rightButtonGestureActive = false;
      const CORE_MOUSE_BUTTON_RIGHT = 2;
      const CORE_MOUSE_ACTION_UP = 0;
      const CORE_MOUSE_ACTION_DOWN = 1;
      const CORE_MOUSE_ACTION_MOVE = 32;

      function rightButtonIsInUse(event) {
        if (rightButtonGestureActive) return true;
        if (typeof event.button === 'number' && event.button === CORE_MOUSE_BUTTON_RIGHT) return true;
        if (typeof event.buttons === 'number' && (event.buttons & CORE_MOUSE_BUTTON_RIGHT) !== 0) return true;
        return false;
      }

      function suppressContextMenuForTerminalApps(event) {
        if (!terminalMouseModeIsActive(term)) return;
        if (!rightButtonIsInUse(event) && event.type !== 'contextmenu') return;
        event.preventDefault();
        event.stopPropagation();
        if (typeof event.stopImmediatePropagation === 'function') {
          event.stopImmediatePropagation();
        }
      }

      function forwardRightMouseToXterm(event, action) {
        if (!terminalMouseModeIsActive(term)) return;

        const core = term && term._core;
        const mouseService = core && core._mouseService;
        const coreMouseService = core && core.coreMouseService;
        const screenElement = core && core.screenElement;
        if (!mouseService || !coreMouseService || !screenElement) return;
        if (typeof mouseService.getMouseReportCoords !== 'function') return;
        if (typeof coreMouseService.triggerMouseEvent !== 'function') return;

        const pos = mouseService.getMouseReportCoords(event, screenElement);
        if (!pos) return;

        coreMouseService.triggerMouseEvent({
          col: pos.col,
          row: pos.row,
          x: pos.x,
          y: pos.y,
          button: CORE_MOUSE_BUTTON_RIGHT,
          action,
          ctrl: event.ctrlKey,
          alt: event.altKey,
          shift: event.shiftKey,
        });
      }

      const onWindowMouseUp = (event) => {
        if (event.button === CORE_MOUSE_BUTTON_RIGHT) {
          if (rightButtonGestureActive) {
            forwardRightMouseToXterm(event, CORE_MOUSE_ACTION_UP);
            suppressContextMenuForTerminalApps(event);
          }
          rightButtonGestureActive = false;
        }
      };
      const onWindowBlur = () => {
        rightButtonGestureActive = false;
      };
      const onWindowMouseMove = (event) => {
        if (!rightButtonGestureActive) return;
        forwardRightMouseToXterm(event, CORE_MOUSE_ACTION_MOVE);
        suppressContextMenuForTerminalApps(event);
      };
      const onRootMouseDown = (event) => {
        if (event.button === CORE_MOUSE_BUTTON_RIGHT) {
          forwardRightMouseToXterm(event, CORE_MOUSE_ACTION_DOWN);
          rightButtonGestureActive = true;
          suppressContextMenuForTerminalApps(event);
        }
      };

      window.addEventListener('mouseup', onWindowMouseUp, true);
      window.addEventListener('blur', onWindowBlur);
      window.addEventListener('mousemove', onWindowMouseMove, true);
      terminalRoot.addEventListener('mousedown', onRootMouseDown, true);
      terminalRoot.addEventListener('contextmenu', suppressContextMenuForTerminalApps, true);

      return () => {
        window.removeEventListener('mouseup', onWindowMouseUp, true);
        window.removeEventListener('blur', onWindowBlur);
        window.removeEventListener('mousemove', onWindowMouseMove, true);
        terminalRoot.removeEventListener('mousedown', onRootMouseDown, true);
        terminalRoot.removeEventListener('contextmenu', suppressContextMenuForTerminalApps, true);
      };
    }

    function initTerminal(root) {
      const term = new global.Terminal({
        theme: getTheme(),
        fontFamily: getFontFamily(),
        fontSize: getFontSize(),
        cursorStyle: getCursorStyle(),
        cursorBlink: getCursorBlink(),
        scrollSensitivity: getScrollSensitivity(),
        macOptionIsMeta: true,
        allowProposedApi: true,
      });
      term.attachCustomKeyEventHandler((event) => {
        if (isShortcutDebugEnabled() && shouldDebugKeyEvent(event)) {
          const paneId = root && root.dataset ? root.dataset.paneId : '';
          console.log(`[conch-keydbg] xterm.customKeyEvent pane=${paneId}`, formatKeyEventForDebug(event));
        }
        return true;
      });

      const fitAddon = new global.FitAddon.FitAddon();
      term.loadAddon(fitAddon);
      term.loadAddon(new global.WebLinksAddon.WebLinksAddon((_event, uri) => {
        if (!tauriOpen || typeof tauriOpen !== 'function') {
          window.open(uri, '_blank');
          return;
        }
        tauriOpen(uri);
      }));
      if (typeof global.Unicode11Addon !== 'undefined') {
        term.loadAddon(new global.Unicode11Addon.Unicode11Addon());
        term.unicode.activeVersion = '11';
      }
      term.open(root);
      return { term, fitAddon };
    }

    return {
      isShortcutDebugEnabled,
      formatKeyEventForDebug,
      toDebugHex,
      toDebugEscaped,
      shouldDebugKeyEvent,
      terminalMouseModeIsActive,
      setupTmuxRightClickBridge,
      initTerminal,
    };
  }

  global.conchTerminalRuntime = {
    create,
  };
})(window);
