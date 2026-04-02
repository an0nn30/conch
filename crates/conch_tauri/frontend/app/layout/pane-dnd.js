(function (exports) {
  'use strict';

  function initPaneDnd(opts) {
    const getActiveTabId = opts.getActiveTabId;
    const getPaneById = opts.getPaneById;
    const getActiveCanvasRect = opts.getActiveCanvasRect;
    const getActiveContainerEl = opts.getActiveContainerEl;
    const movePaneByDrop = opts.movePaneByDrop;
    const onFocusPane = opts.onFocusPane;
    const isDebugEnabled = typeof opts.isDebugEnabled === 'function'
      ? opts.isDebugEnabled
      : () => false;
    const debugLog = typeof opts.debugLog === 'function'
      ? opts.debugLog
      : (...args) => console.log(...args);

    const dragHandlers = new Map();
    const state = {
      active: null,
      overlay: null,
      zoneEl: null,
      labelEl: null,
      latestPointer: null,
      pointerRaf: 0,
      lastRectSig: '',
      lastZoneSig: '',
    };
    const DRAG_START_THRESHOLD_PX = 5;
    const TARGET_SWITCH_DELAY_MS = 90;

    function logDebug(msg, extra) {
      if (!isDebugEnabled()) return;
      if (extra === undefined) debugLog(`[conch-keydbg] pane-dnd ${msg}`);
      else debugLog(`[conch-keydbg] pane-dnd ${msg}`, extra);
    }

    ensureOverlay();

    function ensureOverlay() {
      if (state.overlay) return;
      const overlay = document.createElement('div');
      overlay.className = 'pane-dnd-overlay';

      const zone = document.createElement('div');
      zone.className = 'pane-dnd-zone';

      const label = document.createElement('div');
      label.className = 'pane-dnd-label';

      overlay.appendChild(zone);
      overlay.appendChild(label);
      document.body.appendChild(overlay);

      state.overlay = overlay;
      state.zoneEl = zone;
      state.labelEl = label;
    }

    function showOverlay(rect, zone) {
      if (!rect || !zone) return;
      const overlay = state.overlay;
      const zoneEl = state.zoneEl;
      const label = state.labelEl;
      const left = Math.round(rect.left);
      const top = Math.round(rect.top);
      const width = Math.round(rect.width);
      const height = Math.round(rect.height);
      const rectSig = `${left}:${top}:${width}:${height}`;
      const zoneSig = `${zone}:${width}:${height}`;

      if (overlay.style.display !== 'block') overlay.style.display = 'block';
      if (state.lastRectSig !== rectSig) {
        state.lastRectSig = rectSig;
        overlay.style.left = left + 'px';
        overlay.style.top = top + 'px';
        overlay.style.width = width + 'px';
        overlay.style.height = height + 'px';
      }
      if (state.lastZoneSig === zoneSig) return;
      state.lastZoneSig = zoneSig;

      const edge = Math.max(36, Math.min(84, Math.round(Math.min(rect.width, rect.height) * 0.3)));
      zoneEl.style.left = '0px';
      zoneEl.style.top = '0px';
      zoneEl.style.width = width + 'px';
      zoneEl.style.height = height + 'px';

      if (zone === 'left') {
        zoneEl.style.width = edge + 'px';
      } else if (zone === 'right') {
        zoneEl.style.left = Math.max(0, Math.round(rect.width - edge)) + 'px';
        zoneEl.style.width = edge + 'px';
      } else if (zone === 'top') {
        zoneEl.style.height = edge + 'px';
      } else if (zone === 'bottom') {
        zoneEl.style.top = Math.max(0, Math.round(rect.height - edge)) + 'px';
        zoneEl.style.height = edge + 'px';
      } else {
        const cx = Math.round(rect.width * 0.2);
        const cy = Math.round(rect.height * 0.2);
        zoneEl.style.left = cx + 'px';
        zoneEl.style.top = cy + 'px';
        zoneEl.style.width = Math.max(40, Math.round(rect.width - cx * 2)) + 'px';
        zoneEl.style.height = Math.max(28, Math.round(rect.height - cy * 2)) + 'px';
      }

      label.textContent = `Dock ${zone}`;
    }

    function hideOverlay() {
      if (state.overlay) state.overlay.style.display = 'none';
      state.lastRectSig = '';
      state.lastZoneSig = '';
    }

    function zoneForPoint(rect, x, y) {
      const rx = (x - rect.left) / rect.width;
      const ry = (y - rect.top) / rect.height;
      const edge = 0.26;
      if (rx <= edge) return 'left';
      if (rx >= 1 - edge) return 'right';
      if (ry <= edge) return 'top';
      if (ry >= 1 - edge) return 'bottom';
      return 'center';
    }

    function pointInsideRect(rect, x, y) {
      return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
    }

    function distanceToRect(rect, x, y) {
      const dx = x < rect.left ? rect.left - x : (x > rect.right ? x - rect.right : 0);
      const dy = y < rect.top ? rect.top - y : (y > rect.bottom ? y - rect.bottom : 0);
      return Math.hypot(dx, dy);
    }

    function parsePaneIdFromEl(el) {
      if (!el || !el.dataset || !el.dataset.paneId) return null;
      const parsed = parseInt(el.dataset.paneId, 10);
      return Number.isFinite(parsed) ? parsed : null;
    }

    function getPaneRectById(containerEl, paneId) {
      if (!containerEl || paneId == null) return null;
      const paneEl = containerEl.querySelector(`.terminal-pane[data-pane-id="${paneId}"]`);
      return paneEl ? paneEl.getBoundingClientRect() : null;
    }

    function findDropTarget(clientX, clientY, dragPaneId) {
      const rect = getActiveCanvasRect();
      if (!rect) return null;
      if (clientX < rect.left || clientX > rect.right || clientY < rect.top || clientY > rect.bottom) {
        return null;
      }
      const containerEl = typeof getActiveContainerEl === 'function' ? getActiveContainerEl() : null;
      if (containerEl) {
        const paneEls = Array.from(containerEl.querySelectorAll('.terminal-pane[data-pane-id]'));
        let best = null;
        let bestDistance = Infinity;
        for (const paneEl of paneEls) {
          const paneId = parsePaneIdFromEl(paneEl);
          if (paneId == null || paneId === dragPaneId) continue;
          const paneRect = paneEl.getBoundingClientRect();
          const distance = pointInsideRect(paneRect, clientX, clientY)
            ? 0
            : distanceToRect(paneRect, clientX, clientY);
          if (distance < bestDistance) {
            bestDistance = distance;
            best = { paneId, rect: paneRect };
          }
        }
        if (best) {
          return {
            paneId: best.paneId,
            rect: best.rect,
            zone: zoneForPoint(best.rect, clientX, clientY),
            distance: bestDistance,
          };
        }
      }
      return {
        paneId: null,
        rect,
        zone: zoneForPoint(rect, clientX, clientY),
        distance: 0,
      };
    }

    function commitOrCancel() {
      const drag = state.active;
      if (!drag) return;
      const drop = drag.drop;
      const draggedPane = getPaneById(drag.paneId);
      if (draggedPane && draggedPane.root) draggedPane.root.classList.remove('pane-dnd-dragging');

      let moved = false;
      if (drop && drop.zone && drag.dragging) {
        moved = !!movePaneByDrop(drag.paneId, drop.paneId, drop.zone);
        logDebug(`commit dragPane=${drag.paneId} zone=${drop.zone} moved=${moved}`);
      }
      if (!moved) onFocusPane(drag.paneId);

      window.removeEventListener('pointermove', onGlobalPointerMove, true);
      window.removeEventListener('pointerup', onGlobalPointerUp, true);
      window.removeEventListener('keydown', onGlobalKeyDown, true);
      if (state.pointerRaf) {
        cancelAnimationFrame(state.pointerRaf);
        state.pointerRaf = 0;
      }
      state.latestPointer = null;
      state.active = null;
      hideOverlay();
    }

    function cancelDrag() {
      const drag = state.active;
      if (!drag) return;
      const pane = getPaneById(drag.paneId);
      if (pane && pane.root) pane.root.classList.remove('pane-dnd-dragging');
      onFocusPane(drag.paneId);

      window.removeEventListener('pointermove', onGlobalPointerMove, true);
      window.removeEventListener('pointerup', onGlobalPointerUp, true);
      window.removeEventListener('keydown', onGlobalKeyDown, true);
      if (state.pointerRaf) {
        cancelAnimationFrame(state.pointerRaf);
        state.pointerRaf = 0;
      }
      state.latestPointer = null;
      state.active = null;
      hideOverlay();
      logDebug(`cancel dragPane=${drag.paneId}`);
    }

    function processPointerMove() {
      state.pointerRaf = 0;
      const drag = state.active;
      if (!drag) return;
      const p = state.latestPointer;
      if (!p) return;
      if (getActiveTabId() !== drag.tabId) {
        cancelDrag();
        return;
      }
      if (!drag.dragging) {
        const dx = p.clientX - drag.startX;
        const dy = p.clientY - drag.startY;
        if (Math.hypot(dx, dy) < DRAG_START_THRESHOLD_PX) {
          return;
        }
        drag.dragging = true;
        logDebug(`start dragPane=${drag.paneId} tab=${drag.tabId}`);
      }
      const rawTarget = findDropTarget(p.clientX, p.clientY, drag.paneId);
      let target = rawTarget;
      if (rawTarget && rawTarget.paneId != null) {
        if (drag.lockedPaneId == null) {
          drag.lockedPaneId = rawTarget.paneId;
          drag.pendingPaneId = null;
          drag.pendingPaneSince = 0;
        } else if (rawTarget.paneId !== drag.lockedPaneId) {
          const now = performance.now();
          if (rawTarget.distance === 0) {
            drag.lockedPaneId = rawTarget.paneId;
            drag.pendingPaneId = null;
            drag.pendingPaneSince = 0;
          } else if (drag.pendingPaneId !== rawTarget.paneId) {
            drag.pendingPaneId = rawTarget.paneId;
            drag.pendingPaneSince = now;
          } else if (now - drag.pendingPaneSince >= TARGET_SWITCH_DELAY_MS) {
            drag.lockedPaneId = rawTarget.paneId;
            drag.pendingPaneId = null;
            drag.pendingPaneSince = 0;
          }
        } else {
          drag.pendingPaneId = null;
          drag.pendingPaneSince = 0;
        }
        if (drag.lockedPaneId !== rawTarget.paneId) {
          const containerEl = typeof getActiveContainerEl === 'function' ? getActiveContainerEl() : null;
          const lockedRect = getPaneRectById(containerEl, drag.lockedPaneId);
          if (lockedRect) {
            target = {
              paneId: drag.lockedPaneId,
              rect: lockedRect,
              zone: zoneForPoint(lockedRect, p.clientX, p.clientY),
              distance: distanceToRect(lockedRect, p.clientX, p.clientY),
            };
          }
        }
      } else if (drag.lockedPaneId != null) {
        drag.lockedPaneId = null;
        drag.pendingPaneId = null;
        drag.pendingPaneSince = 0;
      }
      drag.drop = target;
      if (target) {
        showOverlay(target.rect, target.zone);
        const sig = `${target.paneId == null ? 'root' : target.paneId}:${target.zone}`;
        if (sig !== drag.lastHoverSig) {
          drag.lastHoverSig = sig;
          logDebug(`hover dragPane=${drag.paneId} targetPane=${target.paneId == null ? 'root' : target.paneId} zone=${target.zone}`);
        }
      } else {
        if (drag.lastHoverSig !== '') {
          drag.lastHoverSig = '';
          logDebug(`hover dragPane=${drag.paneId} target=none`);
        }
        hideOverlay();
      }
    }

    function onGlobalPointerMove(e) {
      state.latestPointer = { clientX: e.clientX, clientY: e.clientY };
      if (state.pointerRaf) return;
      state.pointerRaf = requestAnimationFrame(processPointerMove);
    }

    function onGlobalPointerUp() {
      commitOrCancel();
    }

    function onGlobalKeyDown(e) {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      cancelDrag();
    }

    function beginDrag(e, paneId) {
      if (state.active) return;
      if (e.button !== 0) return;
      if (e.target && e.target.closest('button,input,textarea,select,a')) return;
      const pane = getPaneById(paneId);
      if (!pane || pane.kind !== 'plugin_view') return;

      e.preventDefault();
      e.stopPropagation();
      pane.root.classList.add('pane-dnd-dragging');

      state.active = {
        paneId,
        tabId: pane.tabId,
        startX: e.clientX,
        startY: e.clientY,
        dragging: false,
        lastHoverSig: '',
        lockedPaneId: null,
        pendingPaneId: null,
        pendingPaneSince: 0,
        drop: null,
      };

      window.addEventListener('pointermove', onGlobalPointerMove, true);
      window.addEventListener('pointerup', onGlobalPointerUp, true);
      window.addEventListener('keydown', onGlobalKeyDown, true);
    }

    function registerDraggablePaneHeader(paneId, headerEl, paneKind) {
      unregisterPane(paneId);
      if (!headerEl || paneKind !== 'plugin_view') return;

      headerEl.style.cursor = 'grab';
      const onPointerDown = (e) => beginDrag(e, paneId);
      headerEl.addEventListener('pointerdown', onPointerDown);
      dragHandlers.set(paneId, () => {
        headerEl.removeEventListener('pointerdown', onPointerDown);
        headerEl.style.cursor = '';
      });
    }

    function unregisterPane(paneId) {
      const cleanup = dragHandlers.get(paneId);
      if (!cleanup) return;
      cleanup();
      dragHandlers.delete(paneId);
      if (state.active && state.active.paneId === paneId) {
        cancelDrag();
      }
    }

    return {
      registerDraggablePaneHeader,
      unregisterPane,
    };
  }

  exports.paneDnd = {
    initPaneDnd,
  };
})(window);
