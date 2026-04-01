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

  const strips = { left: null, right: null };

  // Last user-set split ratios per side (preserved across toggle cycles)
  const lastSplitRatios = { left: 0.5, right: 0.5 };

  let fitActiveTabFn = null;
  let saveLayoutFn = null;
  let savedZoneAssignments = null; // populated from backend before registration

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
  }

  // Provide persisted zone map so register() can honour user overrides.
  function setPersistedZones(map) {
    savedZoneAssignments = map || {};
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
      active:   false,
    };
    toolWindows.set(id, tw);
    zones[zone].windows.push(id);

    if (zones[zone].activeId === null) {
      activate(id);
    } else {
      updateZone(zone);
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

    // Lazily create content container
    if (!tw.el) {
      tw.el = document.createElement('div');
      tw.el.className = 'tool-window-content';
      tw.el.dataset.toolWindow = id;
      zone.contentEl.appendChild(tw.el);
      tw.renderFn(tw.el);
    }
    tw.el.style.display = '';

    updateZone(tw.zone);
    updateSidebar(sideForZone(tw.zone));
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

    // Zone visibility
    if (wins.length === 0 || !hasActive) {
      zone.el.classList.add('empty');
    } else {
      zone.el.classList.remove('empty');
    }

    // Zone header — just shows active window title, no tab buttons (strip handles tabs)
    let headerEl = zone.el.querySelector('.zone-header');
    if (hasActive && wins.length >= 1) {
      const activeTw = toolWindows.get(zone.activeId);
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

    const topZone = zones[side + '-top'];
    const botZone = zones[side + '-bottom'];
    const topActive = topZone.activeId !== null;
    const botActive = botZone.activeId !== null;

    if (!topActive && !botActive) {
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

  // ---- Side strips (IntelliJ-style outer-edge buttons) ----------------------

  function updateStrips() {
    for (const side of ['left', 'right']) {
      const stripEl = strips[side];
      if (!stripEl) continue;

      stripEl.innerHTML = '';

      const topZone = zones[side + '-top'];
      const botZone = zones[side + '-bottom'];

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
    btn.addEventListener('click', () => toggle(windowId));
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

  let _saveTimer = null;
  function triggerSave() {
    if (_saveTimer) clearTimeout(_saveTimer);
    _saveTimer = setTimeout(() => { if (saveLayoutFn) saveLayoutFn(); }, 400);
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
    return tw ? tw.el : null;
  }

  // ---- Public API -----------------------------------------------------------

  exports.toolWindowManager = {
    init,
    setPersistedZones,
    register,
    unregister,
    activate,
    deactivate,
    toggle,
    moveTo,
    isVisible,
    getZoneForWindow,
    getWindowsInZone,
    getZoneAssignments,
    getSplitRatios,
    setSplitRatio,
    getSidebarWidths,
    setSidebarWidth,
    getContentElement,
  };
})(window);
