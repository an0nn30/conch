// Tool Window Manager — IntelliJ-style zone-based panel system.
// Manages tool windows (built-in panels + plugin panels) across 5 zones:
//   left-top, left-bottom, right-top, right-bottom, bottom

(function (exports) {
  'use strict';

  const ZONE_IDS = ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom'];

  // id → { id, title, icon, type, zone, renderFn, el, active }
  const toolWindows = new Map();

  const zones = {};
  for (const z of ZONE_IDS) {
    zones[z] = { windows: [], activeId: null, el: null, contentEl: null, tabStripEl: null };
  }

  const sidebars = {
    left:  { wrapEl: null, panelEl: null, resizeEl: null, dividerEl: null },
    right: { wrapEl: null, panelEl: null, resizeEl: null, dividerEl: null },
  };
  const panelState = {
    left: { visible: true },
    right: { visible: true },
  };

  const strips = { left: null, right: null };
  const DRAGGABLE_ZONES = ['left-top', 'left-bottom', 'right-top', 'right-bottom'];
  const ZONE_LABELS = {
    'left-top': 'Left Top',
    'left-bottom': 'Left Bottom',
    'right-top': 'Right Top',
    'right-bottom': 'Right Bottom',
  };

  // Last user-set split ratios per side (preserved across toggle cycles)
  const lastSplitRatios = { left: 0.5, right: 0.5 };

  let fitActiveTabFn = null;
  let saveLayoutFn = null;
  let savedZoneAssignments = null; // populated from backend before registration
  let savedActiveZoneWindows = null; // populated from backend before registration
  let savedPanelVisibility = { left: null, right: null }; // persisted panel visibility hints
  const stripDrag = {
    active: null,
    overlayEl: null,
    labelEl: null,
    previewEl: null,
    zoneEls: new Map(),
  };
  let resizeDragDepth = 0;

  function beginResizeDrag() {
    resizeDragDepth += 1;
    document.body.classList.add('panel-resize-dragging');
  }

  function endResizeDrag() {
    resizeDragDepth = Math.max(0, resizeDragDepth - 1);
    if (resizeDragDepth === 0) {
      document.body.classList.remove('panel-resize-dragging');
    }
  }

  // ---- Initialisation -------------------------------------------------------

  function init(opts) {
    fitActiveTabFn = opts.fitActiveTab || null;
    saveLayoutFn   = opts.saveLayout   || null;

    for (const z of ZONE_IDS) {
      const el = document.querySelector(`[data-zone="${z}"]`);
      if (el) {
        zones[z].el         = el;
        zones[z].contentEl  = el.querySelector('.zone-content');
        zones[z].tabStripEl = el.querySelector('.zone-tab-strip');
      }
    }

    sidebars.left.wrapEl    = document.getElementById('left-sidebar');
    sidebars.left.panelEl   = document.getElementById('left-panel-container');
    sidebars.left.resizeEl  = document.getElementById('left-sidebar-resize');
    sidebars.left.dividerEl = document.getElementById('left-zone-divider');

    sidebars.right.wrapEl    = document.getElementById('right-sidebar');
    sidebars.right.panelEl   = document.getElementById('right-panel-container');
    sidebars.right.resizeEl  = document.getElementById('right-sidebar-resize');
    sidebars.right.dividerEl = document.getElementById('right-zone-divider');

    strips.left  = document.getElementById('left-strip');
    strips.right = document.getElementById('right-strip');

    initSidebarResize('left');
    initSidebarResize('right');
    initZoneDivider('left');
    initZoneDivider('right');
    ensureStripDragOverlay();
  }

  // Provide persisted zone map so register() can honour user overrides.
  function setPersistedZones(map) {
    savedZoneAssignments = map || {};
  }

  // Provide persisted active window map so register() can restore active window per zone.
  function setPersistedActiveZoneWindows(map) {
    savedActiveZoneWindows = map || {};
  }

  // Provide persisted panel visibility so boot activation can respect hidden panels.
  function setPersistedPanelVisibility(map) {
    const next = map || {};
    if (typeof next.left === 'boolean') savedPanelVisibility.left = next.left;
    if (typeof next.right === 'boolean') savedPanelVisibility.right = next.right;
  }

  function hasPersistedActiveForSide(side) {
    if (side !== 'left' && side !== 'right') return false;
    const active = savedActiveZoneWindows || {};
    const top = active[side + '-top'];
    const bottom = active[side + '-bottom'];
    return (typeof top === 'string' && top.length > 0) || (typeof bottom === 'string' && bottom.length > 0);
  }

  // ---- Registration ---------------------------------------------------------

  function register(id, opts) {
    const defaultZone = opts.defaultZone || 'right-bottom';
    const zone = (savedZoneAssignments && savedZoneAssignments[id]) || defaultZone;

    const tw = {
      id,
      title:    opts.title || id,
      icon:     opts.icon  || null,
      type:     opts.type  || 'plugin',
      zone,
      renderFn: opts.renderFn,
      el:       null,
      renderRootEl: null,
      active:   false,
    };
    toolWindows.set(id, tw);
    zones[zone].windows.push(id);

    const side = sideForZone(zone);
    const appRoot = document.getElementById('app');
    const zenActive = !!(appRoot && appRoot.classList.contains('zen-mode'));
    const persistedSideHidden = (side === 'left' || side === 'right') && savedPanelVisibility[side] === false;
    const sideHiddenOnBoot = (side === 'left' || side === 'right') && (persistedSideHidden || !isPanelVisible(side));
    const sideHasPersistedActive = hasPersistedActiveForSide(side);
    const shouldAutoActivate = !zenActive && !sideHiddenOnBoot && !sideHasPersistedActive;
    const savedActiveId = savedActiveZoneWindows && typeof savedActiveZoneWindows[zone] === 'string'
      ? savedActiveZoneWindows[zone]
      : null;

    if (savedActiveId && savedActiveId === id) {
      if (shouldAutoActivate) {
        activate(id);
      } else {
        if (zones[zone].activeId && zones[zone].activeId !== id) {
          const prev = toolWindows.get(zones[zone].activeId);
          if (prev) {
            prev.active = false;
            if (prev.el) prev.el.style.display = 'none';
          }
        }
        zones[zone].activeId = id;
        tw.active = true;
        updateZone(zone);
        updateSidebar(side);
        updateStrips();
      }
    } else if (zones[zone].activeId === null && shouldAutoActivate) {
      activate(id);
    } else {
      updateZone(zone);
      updateSidebar(side);
      updateStrips();
    }
  }

  function unregister(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;

    const zone = zones[tw.zone];
    zone.windows = zone.windows.filter(w => w !== id);

    if (zone.activeId === id) {
      zone.activeId = zone.windows.length > 0 ? zone.windows[0] : null;
      if (zone.activeId) {
        const next = toolWindows.get(zone.activeId);
        if (next && next.el) { next.active = true; next.el.style.display = ''; }
      }
    }

    if (tw.el && tw.el.parentNode) tw.el.parentNode.removeChild(tw.el);
    toolWindows.delete(id);

    updateZone(tw.zone);
    updateSidebar(sideForZone(tw.zone));
    updateStrips();
  }

  function shouldDeferRender(tw) {
    if (!tw) return false;
    const side = sideForZone(tw.zone);
    if (side !== 'left' && side !== 'right') return false;
    const appRoot = document.getElementById('app');
    return !!(appRoot && appRoot.classList.contains('zen-mode'));
  }

  function ensureWindowElement(tw, zone) {
    if (!tw || tw.el) return;
    const targetZone = zone || zones[tw.zone];
    if (!targetZone || !targetZone.contentEl) return;
    tw.el = document.createElement('div');
    tw.el.className = 'tool-window-content';
    tw.el.dataset.toolWindow = tw.id;
    const renderRootEl = document.createElement('div');
    renderRootEl.className = 'tool-window-scroll-viewport';
    tw.el.appendChild(renderRootEl);
    tw.renderRootEl = renderRootEl;
    targetZone.contentEl.appendChild(tw.el);
    tw.renderFn(renderRootEl);
  }

  // ---- Activation / Deactivation --------------------------------------------

  function activate(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;

    const zone = zones[tw.zone];

    // Deactivate previous
    if (zone.activeId && zone.activeId !== id) {
      const prev = toolWindows.get(zone.activeId);
      if (prev) { prev.active = false; if (prev.el) prev.el.style.display = 'none'; }
    }

    zone.activeId = id;
    tw.active = true;
    const side = sideForZone(tw.zone);
    if (side === 'left' || side === 'right') {
      panelState[side].visible = true;
    }

    if (!shouldDeferRender(tw)) ensureWindowElement(tw, zone);
    if (tw.el) tw.el.style.display = '';

    updateZone(tw.zone);
    updateSidebar(side);
    updateStrips();
    if (fitActiveTabFn) fitActiveTabFn();
    triggerSave();
  }

  function deactivate(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;

    tw.active = false;
    if (tw.el) tw.el.style.display = 'none';

    const zone = zones[tw.zone];
    if (zone.activeId === id) zone.activeId = null;

    updateZone(tw.zone);
    updateSidebar(sideForZone(tw.zone));
    updateStrips();
    if (fitActiveTabFn) fitActiveTabFn();
    triggerSave();
  }

  function toggle(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;
    const side = sideForZone(tw.zone);
    if (tw.active && (side === 'left' || side === 'right') && !isPanelVisible(side)) {
      setPanelVisibility(side, true);
      return;
    }
    if (tw.active) deactivate(id); else activate(id);
  }

  // ---- Moving ---------------------------------------------------------------

  function moveTo(id, targetZone) {
    const tw = toolWindows.get(id);
    if (!tw || tw.zone === targetZone) return;
    if (!zones[targetZone] || !zones[targetZone].contentEl) return;

    const oldZoneName = tw.zone;
    const oldZone = zones[oldZoneName];

    // Remove from old zone
    oldZone.windows = oldZone.windows.filter(w => w !== id);
    if (oldZone.activeId === id) {
      oldZone.activeId = oldZone.windows.length > 0 ? oldZone.windows[0] : null;
      if (oldZone.activeId) {
        const n = toolWindows.get(oldZone.activeId);
        if (n) { n.active = true; if (n.el) n.el.style.display = ''; }
      }
    }

    // Detach DOM
    if (tw.el && tw.el.parentNode) tw.el.parentNode.removeChild(tw.el);

    // Insert into new zone
    tw.zone = targetZone;
    const newZone = zones[targetZone];
    newZone.windows.push(id);

    if (tw.el) newZone.contentEl.appendChild(tw.el);

    // Activate in new zone
    if (newZone.activeId && newZone.activeId !== id) {
      const prev = toolWindows.get(newZone.activeId);
      if (prev) { prev.active = false; if (prev.el) prev.el.style.display = 'none'; }
    }
    newZone.activeId = id;
    tw.active = true;
    if (tw.el) tw.el.style.display = '';
    const targetSide = sideForZone(targetZone);
    if (targetSide === 'left' || targetSide === 'right') {
      panelState[targetSide].visible = true;
    }

    updateZone(oldZoneName);
    updateZone(targetZone);
    updateSidebar(sideForZone(oldZoneName));
    updateSidebar(sideForZone(targetZone));
    updateStrips();
    if (fitActiveTabFn) fitActiveTabFn();
    triggerSave();
  }

  // ---- Zone rendering -------------------------------------------------------

  function updateZone(zoneName) {
    const zone = zones[zoneName];
    if (!zone.el) return;

    const wins = zone.windows;
    const hasActive = zone.activeId !== null;
    const activeTw = hasActive ? toolWindows.get(zone.activeId) : null;

    if (hasActive && activeTw && !shouldDeferRender(activeTw)) {
      ensureWindowElement(activeTw, zone);
    }

    // Zone visibility
    if (wins.length === 0 || !hasActive) {
      zone.el.classList.add('empty');
    } else {
      zone.el.classList.remove('empty');
    }

    // Zone header — just shows active window title, no tab buttons (strip handles tabs)
    let headerEl = zone.el.querySelector('.zone-header');
    if (hasActive && wins.length >= 1) {
      if (!headerEl) {
        headerEl = document.createElement('div');
        headerEl.className = 'zone-header';
        zone.el.insertBefore(headerEl, zone.el.firstChild);
      }
      headerEl.style.display = '';
      headerEl.innerHTML = '';
      const titleSpan = document.createElement('span');
      titleSpan.className = 'zone-header-title';
      titleSpan.textContent = activeTw ? activeTw.title : '';
      headerEl.appendChild(titleSpan);
      headerEl.oncontextmenu = (e) => { e.preventDefault(); if (zone.activeId) showContextMenu(e, zone.activeId); };
    } else if (headerEl) {
      headerEl.style.display = 'none';
    }

    // Tab strip — hidden (we use the header tabs now instead)
    if (zone.tabStripEl) {
      zone.tabStripEl.classList.add('hidden');
    }

    // Show/hide content for each window
    for (const wid of wins) {
      const tw = toolWindows.get(wid);
      if (tw && tw.el) tw.el.style.display = (zone.activeId === wid) ? '' : 'none';
    }
  }

  function updateSidebar(side) {
    if (!side || side === 'bottom') return;
    const sb = sidebars[side];
    if (!sb.wrapEl) return;
    const appRoot = document.getElementById('app');
    const zenActive = !!(appRoot && appRoot.classList.contains('zen-mode'));

    const topZone = zones[side + '-top'];
    const botZone = zones[side + '-bottom'];
    const topActive = topZone.activeId !== null;
    const botActive = botZone.activeId !== null;
    const panelVisible = panelState[side] ? panelState[side].visible : true;

    if (zenActive || !panelVisible || (!topActive && !botActive)) {
      sb.wrapEl.classList.add('hidden');
    } else {
      sb.wrapEl.classList.remove('hidden');
    }

    // Zone divider visible only when both halves are active
    if (sb.dividerEl) {
      if (topActive && botActive) sb.dividerEl.classList.remove('hidden');
      else sb.dividerEl.classList.add('hidden');
    }

    // When only one zone has content, give it all space
    if (topZone.el && botZone.el) {
      if (topActive && !botActive) {
        topZone.el.style.flex = '1';
        botZone.el.style.flex = '0';
      } else if (!topActive && botActive) {
        topZone.el.style.flex = '0';
        botZone.el.style.flex = '1';
      }
      // When both active, restore the last user-set ratio (not a blind 50/50)
      if (topActive && botActive) {
        const tf = parseFloat(topZone.el.style.flex) || 0;
        const bf = parseFloat(botZone.el.style.flex) || 0;
        if (bf < 0.1 || tf < 0.1) {
          const ratio = lastSplitRatios[side] || 0.5;
          topZone.el.style.flex = ratio.toString();
          botZone.el.style.flex = (1 - ratio).toString();
        }
      }
    }
  }

  function clamp(val, min, max) {
    return Math.max(min, Math.min(max, val));
  }

  function ensureStripDragOverlay() {
    if (stripDrag.overlayEl) return;
    const overlay = document.createElement('div');
    overlay.className = 'twm-dnd-overlay';

    for (const zone of DRAGGABLE_ZONES) {
      const z = document.createElement('div');
      z.className = 'twm-dnd-zone';
      z.dataset.zone = zone;
      const title = document.createElement('div');
      title.className = 'twm-dnd-zone-title';
      title.textContent = ZONE_LABELS[zone] || zone;
      z.appendChild(title);
      overlay.appendChild(z);
      stripDrag.zoneEls.set(zone, z);
    }

    const label = document.createElement('div');
    label.className = 'twm-dnd-label';
    overlay.appendChild(label);

    const preview = document.createElement('div');
    preview.className = 'twm-drag-preview';
    overlay.appendChild(preview);

    document.body.appendChild(overlay);
    stripDrag.overlayEl = overlay;
    stripDrag.labelEl = label;
    stripDrag.previewEl = preview;
  }

  function getStripDropZoneRects() {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const pad = clamp(Math.round(vw * 0.018), 12, 28);
    const zoneW = clamp(Math.round(vw * 0.21), 180, 320);
    const zoneH = clamp(Math.round(vh * 0.3), 128, 280);
    const gap = 14;
    const centerY = Math.round(vh / 2);
    const topY = clamp(centerY - zoneH - Math.round(gap / 2), pad, vh - zoneH - pad);
    const bottomY = clamp(centerY + Math.round(gap / 2), pad, vh - zoneH - pad);
    const leftX = pad;
    const rightX = vw - zoneW - pad;

    return {
      'left-top': { left: leftX, top: topY, width: zoneW, height: zoneH },
      'left-bottom': { left: leftX, top: bottomY, width: zoneW, height: zoneH },
      'right-top': { left: rightX, top: topY, width: zoneW, height: zoneH },
      'right-bottom': { left: rightX, top: bottomY, width: zoneW, height: zoneH },
    };
  }

  function setStripDragOverlayVisible(visible) {
    ensureStripDragOverlay();
    stripDrag.overlayEl.style.display = visible ? 'block' : 'none';
  }

  function layoutStripDragPreview(x, y) {
    const preview = stripDrag.previewEl;
    const drag = stripDrag.active;
    if (!preview || !drag) return;
    if (preview.style.display !== 'block') preview.style.display = 'block';
    const width = Math.max(110, drag.previewWidth || 110);
    const height = 28;
    preview.style.left = Math.round(x + 16) + 'px';
    preview.style.top = Math.round(y - height / 2) + 'px';
    preview.style.width = width + 'px';
    preview.style.height = height + 'px';
  }

  function hideStripDragPreview() {
    const preview = stripDrag.previewEl;
    if (!preview) return;
    preview.classList.remove('drop-animating');
    preview.style.display = 'none';
    preview.style.opacity = '';
    preview.style.left = '';
    preview.style.top = '';
    preview.style.width = '';
    preview.style.height = '';
    preview.textContent = '';
  }

  function getDropAnimationRect(targetZone) {
    const zone = zones[targetZone];
    if (zone && zone.el) {
      const rect = zone.el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        const insetX = Math.max(10, Math.round(rect.width * 0.05));
        const insetY = Math.max(8, Math.round(rect.height * 0.06));
        return {
          left: rect.left + insetX,
          top: rect.top + insetY,
          width: Math.max(72, rect.width - insetX * 2),
          height: Math.max(48, rect.height - insetY * 2),
        };
      }
    }
    return getStripDropZoneRects()[targetZone] || null;
  }

  function animateStripDrop(targetZone, done) {
    const preview = stripDrag.previewEl;
    const targetRect = getDropAnimationRect(targetZone);
    if (!preview || !targetRect || preview.style.display !== 'block') {
      hideStripDragPreview();
      done();
      return;
    }
    preview.classList.add('drop-animating');
    preview.style.left = Math.round(targetRect.left) + 'px';
    preview.style.top = Math.round(targetRect.top) + 'px';
    preview.style.width = Math.round(targetRect.width) + 'px';
    preview.style.height = Math.round(targetRect.height) + 'px';
    preview.style.opacity = '0.22';
    window.setTimeout(() => {
      hideStripDragPreview();
      done();
    }, 190);
  }

  function layoutStripDragOverlay(activeZone) {
    ensureStripDragOverlay();
    const rects = getStripDropZoneRects();
    for (const zone of DRAGGABLE_ZONES) {
      const zEl = stripDrag.zoneEls.get(zone);
      const rect = rects[zone];
      if (!zEl || !rect) continue;
      zEl.style.left = rect.left + 'px';
      zEl.style.top = rect.top + 'px';
      zEl.style.width = rect.width + 'px';
      zEl.style.height = rect.height + 'px';
      zEl.classList.toggle('active', zone === activeZone);
      zEl.classList.toggle('forbidden', stripDrag.active && stripDrag.active.sourceZone === zone);
    }
    const label = stripDrag.labelEl;
    if (!label) return;
    const labelText = activeZone ? `Drop into ${ZONE_LABELS[activeZone]}` : 'Drag to dock this tool window';
    label.textContent = labelText;
  }

  function hitStripDropZone(x, y, sourceZone) {
    const rects = getStripDropZoneRects();
    let chosen = null;
    let best = Infinity;
    const capturePad = 18;
    for (const zone of DRAGGABLE_ZONES) {
      if (zone === sourceZone) continue;
      const r = rects[zone];
      const left = r.left - capturePad;
      const right = r.left + r.width + capturePad;
      const top = r.top - capturePad;
      const bottom = r.top + r.height + capturePad;
      const inside = x >= left && x <= right && y >= top && y <= bottom;
      if (!inside) continue;
      const cx = r.left + r.width / 2;
      const cy = r.top + r.height / 2;
      const d = Math.hypot(x - cx, y - cy);
      if (d < best) {
        best = d;
        chosen = zone;
      }
    }
    return chosen;
  }

  function endStripDrag(commit) {
    const drag = stripDrag.active;
    if (!drag) return;
    window.removeEventListener('pointermove', onStripDragMove, true);
    window.removeEventListener('pointerup', onStripDragUp, true);
    window.removeEventListener('keydown', onStripDragKeyDown, true);
    window.removeEventListener('resize', onStripDragResize);
    document.body.style.userSelect = '';
    document.body.style.cursor = '';
    if (drag.buttonEl) drag.buttonEl.classList.remove('twm-strip-dragging');
    if (commit && drag.dragging && drag.targetZone && drag.targetZone !== drag.sourceZone) {
      if (drag.buttonEl) drag.buttonEl.dataset.suppressClick = '1';
      animateStripDrop(drag.targetZone, () => {
        moveTo(drag.windowId, drag.targetZone);
        setStripDragOverlayVisible(false);
      });
    } else {
      hideStripDragPreview();
      setStripDragOverlayVisible(false);
    }
    stripDrag.active = null;
  }

  function onStripDragMove(e) {
    const drag = stripDrag.active;
    if (!drag) return;
    const dx = e.clientX - drag.startX;
    const dy = e.clientY - drag.startY;
    if (!drag.dragging && Math.hypot(dx, dy) >= 4) {
      drag.dragging = true;
      document.body.style.userSelect = 'none';
      document.body.style.cursor = 'grabbing';
      if (drag.buttonEl) drag.buttonEl.classList.add('twm-strip-dragging');
      setStripDragOverlayVisible(true);
    }
    if (!drag.dragging) return;
    layoutStripDragPreview(e.clientX, e.clientY);
    drag.targetZone = hitStripDropZone(e.clientX, e.clientY, drag.sourceZone);
    layoutStripDragOverlay(drag.targetZone);
  }

  function onStripDragUp() {
    endStripDrag(true);
  }

  function onStripDragKeyDown(e) {
    if (e.key !== 'Escape') return;
    e.preventDefault();
    e.stopPropagation();
    endStripDrag(false);
  }

  function onStripDragResize() {
    if (!stripDrag.active || !stripDrag.active.dragging) return;
    layoutStripDragOverlay(stripDrag.active.targetZone);
  }

  function beginStripDrag(e, windowId, sourceZone, buttonEl) {
    if (e.button !== 0) return;
    if (stripDrag.active) return;
    const tw = toolWindows.get(windowId);
    if (!tw) return;
    const previewRect = buttonEl ? buttonEl.getBoundingClientRect() : null;
    stripDrag.active = {
      windowId,
      sourceZone,
      buttonEl,
      startX: e.clientX,
      startY: e.clientY,
      dragging: false,
      targetZone: null,
      previewWidth: previewRect ? previewRect.width : 92,
      previewHeight: 28,
    };
    if (stripDrag.previewEl) {
      stripDrag.previewEl.textContent = tw.title;
      stripDrag.previewEl.style.display = 'block';
      stripDrag.previewEl.style.opacity = '1';
      if (previewRect) {
        stripDrag.previewEl.style.left = Math.round(previewRect.left) + 'px';
        stripDrag.previewEl.style.top = Math.round(previewRect.top) + 'px';
        stripDrag.previewEl.style.width = Math.round(previewRect.width) + 'px';
        stripDrag.previewEl.style.height = Math.round(previewRect.height) + 'px';
      }
    }
    window.addEventListener('pointermove', onStripDragMove, true);
    window.addEventListener('pointerup', onStripDragUp, true);
    window.addEventListener('keydown', onStripDragKeyDown, true);
    window.addEventListener('resize', onStripDragResize);
  }

  // ---- Side strips (IntelliJ-style outer-edge buttons) ----------------------

  function updateStrips() {
    for (const side of ['left', 'right']) {
      const stripEl = strips[side];
      if (!stripEl) continue;

      stripEl.innerHTML = '';

      const topZone = zones[side + '-top'];
      const botZone = zones[side + '-bottom'];
      const totalWindows = topZone.windows.length + botZone.windows.length;

      stripEl.classList.toggle('hidden', totalWindows === 0);
      if (totalWindows === 0) continue;

      // Top section — windows assigned to the top zone
      const topSection = document.createElement('div');
      topSection.className = 'strip-section';
      for (const wid of topZone.windows) {
        topSection.appendChild(makeStripBtn(wid, topZone));
      }
      stripEl.appendChild(topSection);

      // Bottom section — windows assigned to the bottom zone (pushed to bottom)
      const botSection = document.createElement('div');
      botSection.className = 'strip-section strip-section-bottom';
      for (const wid of botZone.windows) {
        botSection.appendChild(makeStripBtn(wid, botZone));
      }
      stripEl.appendChild(botSection);
    }
  }

  function makeStripBtn(windowId, zone) {
    const tw = toolWindows.get(windowId);
    if (!tw) return document.createTextNode('');

    const btn = document.createElement('button');
    btn.className = 'strip-btn' + (tw.active ? ' active' : '');
    btn.textContent = tw.title;
    btn.dataset.toolWindow = windowId;
    btn.addEventListener('click', (e) => {
      if (btn.dataset.suppressClick === '1') {
        delete btn.dataset.suppressClick;
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      toggle(windowId);
    });
    btn.addEventListener('pointerdown', (e) => beginStripDrag(e, windowId, zone && zone.el ? zone.el.dataset.zone : tw.zone, btn));
    btn.addEventListener('contextmenu', (e) => { e.preventDefault(); showContextMenu(e, windowId); });
    return btn;
  }

  // ---- Context menu (Phase 3 stub — simple implementation) ------------------

  function showContextMenu(event, windowId) {
    // Remove any existing context menu
    const old = document.getElementById('twm-ctx-menu');
    if (old) old.remove();

    const tw = toolWindows.get(windowId);
    if (!tw) return;

    const menu = document.createElement('div');
    menu.id = 'twm-ctx-menu';
    menu.className = 'twm-context-menu';

    const targets = [
      { zone: 'left-top',     label: 'Left (Top)' },
      { zone: 'left-bottom',  label: 'Left (Bottom)' },
      { zone: 'right-top',    label: 'Right (Top)' },
      { zone: 'right-bottom', label: 'Right (Bottom)' },
    ];

    for (const t of targets) {
      if (t.zone === tw.zone) continue;
      const item = document.createElement('div');
      item.className = 'twm-ctx-item';
      item.textContent = 'Move to ' + t.label;
      item.addEventListener('click', () => { menu.remove(); moveTo(windowId, t.zone); });
      menu.appendChild(item);
    }

    // Separator + Hide
    const sep = document.createElement('div');
    sep.className = 'twm-ctx-sep';
    menu.appendChild(sep);

    const hideItem = document.createElement('div');
    hideItem.className = 'twm-ctx-item';
    hideItem.textContent = 'Hide';
    hideItem.addEventListener('click', () => { menu.remove(); deactivate(windowId); });
    menu.appendChild(hideItem);

    menu.style.position = 'fixed';
    menu.style.left = event.clientX + 'px';
    menu.style.top  = event.clientY + 'px';
    menu.style.visibility = 'hidden';
    document.body.appendChild(menu);

    // Clamp to viewport so the menu doesn't clip off-screen
    const rect = menu.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    if (rect.right > vw) menu.style.left = Math.max(0, vw - rect.width - 4) + 'px';
    if (rect.bottom > vh) menu.style.top = Math.max(0, vh - rect.height - 4) + 'px';
    if (rect.left < 0) menu.style.left = '4px';
    if (rect.top < 0) menu.style.top = '4px';
    menu.style.visibility = '';

    const dismiss = (e) => {
      if (!menu.contains(e.target)) { menu.remove(); document.removeEventListener('pointerdown', dismiss, true); }
    };
    setTimeout(() => document.addEventListener('pointerdown', dismiss, true), 0);
  }

  // ---- Sidebar edge resize --------------------------------------------------

  function initSidebarResize(side) {
    const sb = sidebars[side];
    if (!sb.resizeEl || !sb.panelEl) return;

    let dragging = false, startX = 0, startWidth = 0;
    const minW = side === 'left' ? 200 : 180;
    const maxW = side === 'left' ? 600 : 500;

    sb.resizeEl.addEventListener('dragstart', (e) => e.preventDefault());
    sb.resizeEl.style.touchAction = 'none';

    sb.resizeEl.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      sb.resizeEl.setPointerCapture(e.pointerId);
      dragging = true;
      startX = e.clientX;
      startWidth = sb.panelEl.offsetWidth;
      sb.resizeEl.classList.add('dragging');
      beginResizeDrag();
      document.body.style.cursor = 'col-resize';
      document.body.style.userSelect = 'none';
    });

    sb.resizeEl.addEventListener('pointermove', (e) => {
      if (!dragging) return;
      const delta = side === 'left' ? (e.clientX - startX) : (startX - e.clientX);
      const newWidth = Math.max(minW, Math.min(maxW, startWidth + delta));
      sb.panelEl.style.width = newWidth + 'px';
      if (fitActiveTabFn) fitActiveTabFn();
    });

    sb.resizeEl.addEventListener('pointerup', (e) => {
      if (!dragging) return;
      sb.resizeEl.releasePointerCapture(e.pointerId);
      dragging = false;
      sb.resizeEl.classList.remove('dragging');
      endResizeDrag();
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      triggerSave();
    });

    sb.resizeEl.addEventListener('pointercancel', () => {
      if (!dragging) return;
      dragging = false;
      sb.resizeEl.classList.remove('dragging');
      endResizeDrag();
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      triggerSave();
    });
  }

  // ---- Zone divider resize --------------------------------------------------

  function initZoneDivider(side) {
    const dividerEl = sidebars[side].dividerEl;
    const topZoneEl = zones[side + '-top'].el;
    const botZoneEl = zones[side + '-bottom'].el;
    if (!dividerEl || !topZoneEl || !botZoneEl) return;

    let dragging = false, startY = 0, startTopFlex = 0, startBotFlex = 0;

    dividerEl.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      dividerEl.setPointerCapture(e.pointerId);
      dragging = true;
      startY = e.clientY;
      const topH = topZoneEl.offsetHeight;
      const botH = botZoneEl.offsetHeight;
      const total = topH + botH;
      startTopFlex = total > 0 ? topH / total : 0.5;
      startBotFlex = 1 - startTopFlex;
      dividerEl.classList.add('dragging');
      beginResizeDrag();
      document.body.style.cursor = 'row-resize';
      document.body.style.userSelect = 'none';
    });

    dividerEl.addEventListener('pointermove', (e) => {
      if (!dragging) return;
      const container = topZoneEl.parentElement;
      const containerH = container.clientHeight - dividerEl.offsetHeight;
      if (containerH <= 0) return;
      const delta = e.clientY - startY;
      const newTopRatio = Math.max(0.15, Math.min(0.85, startTopFlex + delta / containerH));
      topZoneEl.style.flex = newTopRatio.toString();
      botZoneEl.style.flex = (1 - newTopRatio).toString();
      lastSplitRatios[side] = newTopRatio;
      if (fitActiveTabFn) fitActiveTabFn();
    });

    dividerEl.addEventListener('pointerup', (e) => {
      if (!dragging) return;
      dividerEl.releasePointerCapture(e.pointerId);
      dragging = false;
      dividerEl.classList.remove('dragging');
      endResizeDrag();
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      triggerSave();
    });

    dividerEl.addEventListener('pointercancel', () => {
      if (!dragging) return;
      dragging = false;
      dividerEl.classList.remove('dragging');
      endResizeDrag();
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      triggerSave();
    });
  }

  // ---- Helpers --------------------------------------------------------------

  function sideForZone(zoneName) {
    if (zoneName.startsWith('left'))  return 'left';
    if (zoneName.startsWith('right')) return 'right';
    return 'bottom';
  }

  function isPanelVisible(side) {
    return !!(panelState[side] && panelState[side].visible);
  }

  function hasActiveWindowOnSide(side) {
    if (side !== 'left' && side !== 'right') return false;
    const topZone = zones[side + '-top'];
    const botZone = zones[side + '-bottom'];
    return !!((topZone && topZone.activeId) || (botZone && botZone.activeId));
  }

  function isPanelOpen(side) {
    return isPanelVisible(side) && hasActiveWindowOnSide(side);
  }

  function setPanelVisibility(side, visible, opts) {
    if (!panelState[side]) return;
    panelState[side].visible = !!visible;
    if (panelState[side].visible) {
      const topZone = zones[side + '-top'];
      const botZone = zones[side + '-bottom'];
      if (topZone && botZone && topZone.activeId === null && botZone.activeId === null) {
        const candidate = (topZone.windows && topZone.windows[0]) || (botZone.windows && botZone.windows[0]) || null;
        if (candidate) activate(candidate);
      }
    }
    if (panelState[side].visible) {
      updateZone(side + '-top');
      updateZone(side + '-bottom');
    }
    updateSidebar(side);
    updateStrips();
    if (!opts || opts.save !== false) triggerSave();
  }

  function togglePanel(side) {
    if (!panelState[side]) return;
    setPanelVisibility(side, !panelState[side].visible);
  }

  function triggerSave() {
    if (saveLayoutFn) saveLayoutFn();
  }

  // ---- Query helpers --------------------------------------------------------

  function isVisible(id) {
    const tw = toolWindows.get(id);
    return tw ? tw.active : false;
  }

  function getZoneForWindow(id) {
    const tw = toolWindows.get(id);
    return tw ? tw.zone : null;
  }

  function getWindowsInZone(zoneName) {
    return zones[zoneName] ? [...zones[zoneName].windows] : [];
  }

  function getZoneAssignments() {
    const map = {};
    for (const [id, tw] of toolWindows) { map[id] = tw.zone; }
    return map;
  }

  function getActiveZoneAssignments() {
    const map = {};
    for (const zoneName of ZONE_IDS) {
      const activeId = zones[zoneName] ? zones[zoneName].activeId : null;
      if (typeof activeId === 'string' && activeId.length > 0) {
        map[zoneName] = activeId;
      }
    }
    return map;
  }

  function getSplitRatios() {
    const ratios = {};
    for (const side of ['left', 'right']) {
      const topEl = zones[side + '-top'].el;
      const botEl = zones[side + '-bottom'].el;
      if (topEl && botEl) {
        const tf = parseFloat(topEl.style.flex) || 1;
        const bf = parseFloat(botEl.style.flex) || 1;
        ratios[side] = tf / (tf + bf);
      }
    }
    return ratios;
  }

  function setSplitRatio(side, ratio) {
    const topEl = zones[side + '-top'].el;
    const botEl = zones[side + '-bottom'].el;
    if (topEl && botEl && ratio > 0 && ratio < 1) {
      topEl.style.flex = ratio.toString();
      botEl.style.flex = (1 - ratio).toString();
      lastSplitRatios[side] = ratio;
    }
  }

  function getSidebarWidths() {
    return {
      left:  sidebars.left.panelEl  ? sidebars.left.panelEl.offsetWidth  : 0,
      right: sidebars.right.panelEl ? sidebars.right.panelEl.offsetWidth : 0,
    };
  }

  function setSidebarWidth(side, width) {
    const sb = sidebars[side];
    if (sb && sb.panelEl && width > 0) sb.panelEl.style.width = width + 'px';
  }

  // Expose content container for a window (used by plugin-widgets.js)
  function getContentElement(id) {
    const tw = toolWindows.get(id);
    return tw ? (tw.renderRootEl || tw.el) : null;
  }

  function listWindows() {
    return Array.from(toolWindows.values()).map((tw) => ({
      id: tw.id,
      title: tw.title,
      type: tw.type,
      zone: tw.zone,
      active: tw.active,
    }));
  }

  // ---- Public API -----------------------------------------------------------

  exports.toolWindowManager = {
    init,
    setPersistedZones,
    setPersistedActiveZoneWindows,
    setPersistedPanelVisibility,
    register,
    unregister,
    activate,
    deactivate,
    toggle,
    moveTo,
    isVisible,
    isPanelVisible,
    isPanelOpen,
    setPanelVisibility,
    togglePanel,
    getZoneForWindow,
    getWindowsInZone,
    getZoneAssignments,
    getActiveZoneAssignments,
    getSplitRatios,
    setSplitRatio,
    getSidebarWidths,
    setSidebarWidth,
    getContentElement,
    listWindows,
  };
})(window);
