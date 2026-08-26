// Tool Window Manager — IntelliJ-style zone-based panel system.
// Manages tool windows (built-in panels + plugin panels) across 6 zones:
//   left-top, left-bottom, right-top, right-bottom, bottom-left, bottom-right

(function (exports) {
  'use strict';

  const ZONE_IDS = ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right'];

  // id → { id, title, icon, type, zone, renderFn, renderDisposer, el, active }
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
    bottom: { visible: false },
  };

  const strips = { left: null, right: null, bottom: null };
  const DRAGGABLE_ZONES = ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right'];
  const ZONE_LABELS = {
    'left-top': 'Left Top',
    'left-bottom': 'Left Bottom',
    'right-top': 'Right Top',
    'right-bottom': 'Right Bottom',
    'bottom-left': 'Bottom Left',
    'bottom-right': 'Bottom Right',
  };

  // Last user-set split ratios per side (preserved across toggle cycles)
  const lastSplitRatios = { left: 0.5, right: 0.5, bottom: 0.5 };

  // ---- View mode ------------------------------------------------------------
  // Every tool window — built-in or plugin, no special cases — carries a view
  // mode: docked in one of the five zones, or popped out into its own OS
  // window (a "panel host", see src/panel_host.rs). The mode is a property of
  // the tool window itself, which is why it persists id-keyed alongside
  // tool_window_zones rather than per parent window.
  //
  // While a window is popped out the manager keeps the registration and the
  // zone assignment (that IS the remembered dock target) but owns no DOM for
  // it: `renderFn` is render-once, so the element is dropped on the way out
  // and the HOST renders a fresh one. Dock-back re-renders through the same
  // lazy `ensureWindowElement` path any first activation uses. Nothing is ever
  // moved between documents.
  const VIEW_MODE_DOCK = 'dock';
  const VIEW_MODE_WINDOW = 'window';
  const viewModes = new Map();   // id → 'dock' | 'window'
  // Last known panel-host visibility per popped-out id. Drives the rail
  // button's lit state and the toggle routing; kept in sync by the
  // panel-host-shown/hidden events the parent receives from Rust.
  const hostVisible = new Map(); // id → boolean
  // Generation token per popped-out id: the `req_id` Rust minted for the host
  // currently live for that id (`open_panel_host`'s return value), or a
  // `{ pendingIssue }` marker while an open is in flight. Its only job is to
  // let notifyHostDocked tell a CURRENT dock-back from a STALE echo — see the
  // comment there for the sequence that made this necessary.
  const hostReqIds = new Map(); // id → number | { pendingIssue: number }
  // Monotonic issue number, incremented synchronously per open ATTEMPT and
  // captured in that attempt's own closure — the same discipline as
  // `themeColorsFetchToken` in app/config-runtime.js:67-88, and for the same
  // reason: two `open_panel_host` calls for one id can be in flight at once
  // (pop out, dock, pop out again), and Tauri gives no guarantee that they
  // resolve in the order they were issued. Without a per-attempt identity,
  // "is this the host we are waiting for?" degrades to "is this id popped out
  // at all?", and an older call resolving last would overwrite the newer
  // attempt's marker with a dead generation. Last-INITIATED-wins, not
  // last-resolved-wins.
  let nextHostRequestIssue = 0;

  function isPendingHostRequest(value) {
    return !!value && typeof value === 'object' && typeof value.pendingIssue === 'number';
  }
  let savedViewModes = null;     // populated from backend before registration
  // Window-mode ids whose host was open when the layout was saved, waiting to
  // be summoned. Drained once registrations finish; a plugin that registers
  // later than that (they arrive asynchronously) is summoned on the spot.
  const pendingWindowSummons = new Set();
  let summonImmediately = false;

  let fitActiveTabFn = null;
  let saveLayoutFn = null;
  let invokeFn = null;
  let bottomZoneWrapEl = null;
  let bottomZoneDividerEl = null;
  let savedZoneAssignments = null; // populated from backend before registration
  let savedActiveZoneWindows = null; // populated from backend before registration
  let savedPanelVisibility = { left: null, right: null, bottom: null }; // persisted panel visibility hints
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
    // The only backend the manager talks to: the panel-host commands. Injected
    // rather than reached for, so the module stays loadable (and testable)
    // without a Tauri context, like every other dependency here.
    invokeFn       = typeof opts.invoke === 'function' ? opts.invoke : null;

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
    strips.bottom = document.getElementById('bottom-strip');
    bottomZoneWrapEl = document.getElementById('bottom-zone-wrap');
    bottomZoneDividerEl = document.getElementById('bottom-zone-divider');

    initSidebarResize('left');
    initSidebarResize('right');
    initZoneDivider('left');
    initZoneDivider('right');
    initZoneDivider('bottom');
    ensureStripDragOverlay();
  }

  // 'bottom' predates the bottom-left/right pair and stays accepted forever:
  // old state.toml zone values, plugin location strings, and defaultZone
  // registrations all still say it. Unknown names return null so callers
  // keep their existing fallback behaviour.
  function normalizeZoneName(name) {
    if (name === 'bottom') return 'bottom-left';
    return ZONE_IDS.includes(name) ? name : null;
  }

  // Provide persisted zone map so register() can honour user overrides. Values
  // are normalized on the way in — a legacy 'bottom' value becomes
  // 'bottom-left' — so every reader downstream only ever sees current zone
  // names. A value that normalizes to nothing (junk) is dropped rather than
  // kept as null, so the lookup in register() misses and falls back to
  // defaultZone exactly as it does for an id with no saved entry at all.
  function setPersistedZones(map) {
    const next = {};
    for (const [id, zone] of Object.entries(map || {})) {
      const normalized = normalizeZoneName(zone);
      if (normalized) next[id] = normalized;
    }
    savedZoneAssignments = next;
  }

  // Provide persisted active window map so register() can restore active window per zone.
  // Keys are normalized the same way values are above. A saved layout can
  // carry both the legacy 'bottom' key and a current 'bottom-left' key at once
  // (an old file touched by a newer build, say) — the current-name key always
  // wins that collision, so the alias is applied first and the canonical keys
  // are layered on top of it rather than the other way around.
  function setPersistedActiveZoneWindows(map) {
    const src = map || {};
    const next = {};
    for (const [key, value] of Object.entries(src)) {
      if (key === 'bottom') next['bottom-left'] = value;
    }
    for (const [key, value] of Object.entries(src)) {
      if (key === 'bottom') continue;
      const normalized = normalizeZoneName(key);
      if (normalized) next[normalized] = value;
    }
    savedActiveZoneWindows = next;
  }

  // Provide persisted view modes so register() knows which windows are popped
  // out rather than docked. Must be called before any register().
  function setPersistedViewModes(map) {
    savedViewModes = map || {};
  }

  // Provide persisted panel visibility so boot activation can respect hidden panels.
  function setPersistedPanelVisibility(map) {
    const next = map || {};
    if (typeof next.left === 'boolean') savedPanelVisibility.left = next.left;
    if (typeof next.right === 'boolean') savedPanelVisibility.right = next.right;
    if (typeof next.bottom === 'boolean') savedPanelVisibility.bottom = next.bottom;
  }

  // "This side has a saved arrangement" — which includes a side the user closed.
  // Presence of the key is what matters, not whether it names a window: an empty
  // value means "configured, nothing active", and only a wholly absent key means
  // "never configured", where auto-activating a default is still right.
  //
  // The bottom zone is a left/right pair like the sides, but keyed by
  // 'bottom-left'/'bottom-right' rather than '<side>-top'/'<side>-bottom'.
  // Keys are already normalized by setPersistedActiveZoneWindows by the time
  // this runs, so a legacy 'bottom' key never reaches here — it has already
  // become 'bottom-left'.
  function hasPersistedActiveForSide(side) {
    const active = savedActiveZoneWindows || {};
    const has = (key) => Object.prototype.hasOwnProperty.call(active, key);
    if (side === 'bottom') return has('bottom-left') || has('bottom-right');
    if (side !== 'left' && side !== 'right') return false;
    return has(side + '-top') || has(side + '-bottom');
  }

  // ---- Registration ---------------------------------------------------------

  function register(id, opts) {
    const defaultZone = normalizeZoneName(opts.defaultZone) || 'right-bottom';
    const zone = (savedZoneAssignments && savedZoneAssignments[id]) || defaultZone;

    const tw = {
      id,
      title:    opts.title || id,
      icon:     opts.icon  || null,
      type:     opts.type  || 'plugin',
      zone,
      renderFn: opts.renderFn,
      renderDisposer: null,
      el:       null,
      renderRootEl: null,
      active:   false,
    };
    toolWindows.set(id, tw);
    zones[zone].windows.push(id);

    const side = sideForZone(zone);
    const appRoot = document.getElementById('app');
    const zenActive = !!(appRoot && appRoot.classList.contains('zen-mode'));
    // Applies to the bottom zone too: registration used to force-activate any
    // window there regardless of saved visibility, which re-opened a bottom zone
    // the user had closed immediately after boot restored it as hidden.
    const persistedSideHidden = savedPanelVisibility[side] === false;
    const sideHiddenOnBoot = persistedSideHidden || !isPanelVisible(side);
    const sideHasPersistedActive = hasPersistedActiveForSide(side);
    const shouldAutoActivate = !zenActive && !sideHiddenOnBoot && !sideHasPersistedActive;
    const savedActiveId = savedActiveZoneWindows && typeof savedActiveZoneWindows[zone] === 'string'
      ? savedActiveZoneWindows[zone]
      : null;

    // A window the user had popped out never mounts into its zone on boot: it
    // keeps its place in the zone's window list (so the rail button is there
    // to summon it with) but the zone stays closed as far as the DOM is
    // concerned. Whether its host is re-summoned is decided by whether the
    // saved layout recorded it as its zone's open window — the same bit that
    // records a docked window as open, so "closed while popped out" survives a
    // restart the same way "closed while docked" does.
    if (savedViewModes && savedViewModes[id] === VIEW_MODE_WINDOW) {
      viewModes.set(id, VIEW_MODE_WINDOW);
      if (savedActiveId && savedActiveId === id) {
        hostVisible.set(id, true);
        tw.active = true;
        if (summonImmediately) summonWindowHost(id);
        else pendingWindowSummons.add(id);
      }
      updateZone(zone);
      updateSidebar(side);
      updateBottomZone();
      updateStrips();
      return;
    }

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
        updateBottomZone();
        updateStrips();
      }
    } else if (zones[zone].activeId === null && shouldAutoActivate) {
      activate(id);
    } else {
      updateZone(zone);
      updateSidebar(side);
      updateBottomZone();
      updateStrips();
    }
  }

  function unregister(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;

    const zone = zones[tw.zone];
    zone.windows = zone.windows.filter(w => w !== id);

    if (zone.activeId === id) {
      // A popped-out sibling must never be promoted into the zone's active
      // slot — it has no DOM to activate and its host window is already
      // showing it live (see firstDockableIn, and F1 in the branch review).
      zone.activeId = firstDockableIn(zone.windows);
      if (zone.activeId) {
        const next = toolWindows.get(zone.activeId);
        if (next && next.el) { next.active = true; next.el.style.display = ''; }
      }
    }

    disposeWindowRender(tw);
    if (tw.el && tw.el.parentNode) tw.el.parentNode.removeChild(tw.el);
    // A popped-out window whose plugin was just removed would otherwise leave
    // its host on screen with nothing to host. DESTROY it rather than hide it:
    // the tool window is going away for good — every line below deletes the
    // bookkeeping that could ever bring it back — so a hide would strand a
    // live, invisible webview still running the removed plugin's panel, with
    // no rail entry and no summon path left to reach it. `dock_panel_host`
    // takes a parent caller now (src/panel_host.rs's `resolve_dock_target`)
    // and destroys the window AND drops the registry entry.
    //
    // The `panel-host-docked` echo it emits back is inert here, doubly so: by
    // the time it lands this id fails notifyHostDocked's `toolWindows.has(id)`
    // guard, and getRegistration(id) is null, so there is no renderFn to
    // remount even in principle.
    if (getViewMode(id) === VIEW_MODE_WINDOW) {
      panelHostInvoke('dock_panel_host', { toolWindowId: id }).catch(() => {});
    }
    viewModes.delete(id);
    hostVisible.delete(id);
    hostReqIds.delete(id);
    pendingWindowSummons.delete(id);
    toolWindows.delete(id);

    updateZone(tw.zone);
    updateSidebar(sideForZone(tw.zone));
    updateBottomZone();
    updateStrips();
  }

  function shouldDeferRender(tw) {
    if (!tw) return false;
    const side = sideForZone(tw.zone);
    if (side !== 'left' && side !== 'right') return false;
    const appRoot = document.getElementById('app');
    return !!(appRoot && appRoot.classList.contains('zen-mode'));
  }

  // A renderFn may return either a disposer function or an object with a
  // destroy() method. Most existing tool windows return nothing and keep
  // their historical render-once behavior; stateful panels use this focused
  // contract to release subscriptions before their DOM is detached.
  function disposerForRenderResult(result) {
    if (typeof result === 'function') return result;
    if (result && typeof result.destroy === 'function') return () => result.destroy();
    return null;
  }

  function disposeWindowRender(tw) {
    if (!tw || typeof tw.renderDisposer !== 'function') return;
    const dispose = tw.renderDisposer;
    tw.renderDisposer = null;
    try {
      dispose();
    } catch (error) {
      console.error('tool-window-manager: render disposal failed', tw.id, error);
    }
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
    tw.renderDisposer = disposerForRenderResult(tw.renderFn(renderRootEl));
  }

  // ---- Activation / Deactivation --------------------------------------------

  function activate(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;
    // A popped-out window has no docked presence to activate. Callers that
    // want it on screen go through summonWindowHost() (the rail toggle does);
    // the zone-fallback callers below just skip it.
    if (getViewMode(id) === VIEW_MODE_WINDOW) return;

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
    } else if (side === 'bottom') {
      panelState.bottom.visible = true;
    }

    if (!shouldDeferRender(tw)) ensureWindowElement(tw, zone);
    if (tw.el) tw.el.style.display = '';

    updateZone(tw.zone);
    updateSidebar(side);
    updateBottomZone();
    updateStrips();
    if (fitActiveTabFn) fitActiveTabFn();
    triggerSave();
  }

  function deactivate(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;
    // "Hide" on a popped-out window means hide its host window, not close a
    // zone that isn't showing it. Reachable from the rail's context menu.
    if (getViewMode(id) === VIEW_MODE_WINDOW) { hideWindowHost(id); return; }

    tw.active = false;
    if (tw.el) tw.el.style.display = 'none';

    const zone = zones[tw.zone];
    if (zone.activeId === id) zone.activeId = null;

    updateZone(tw.zone);
    updateSidebar(sideForZone(tw.zone));
    updateBottomZone();
    updateStrips();
    if (fitActiveTabFn) fitActiveTabFn();
    triggerSave();
  }

  function toggle(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;
    // Window mode routes to the host and returns; everything below this line
    // is the unchanged docked path.
    if (getViewMode(id) === VIEW_MODE_WINDOW) {
      if (hostVisible.get(id) === true) hideWindowHost(id);
      else summonWindowHost(id);
      return;
    }
    const side = sideForZone(tw.zone);
    if (tw.active && (side === 'left' || side === 'right') && !isPanelVisible(side)) {
      setPanelVisibility(side, true);
      return;
    }
    if (tw.active && side === 'bottom' && !isPanelVisible('bottom')) {
      setPanelVisibility('bottom', true);
      return;
    }
    if (tw.active) deactivate(id); else activate(id);
  }

  // ---- View mode (dock <-> own window) --------------------------------------

  function getViewMode(id) {
    return viewModes.get(id) === VIEW_MODE_WINDOW ? VIEW_MODE_WINDOW : VIEW_MODE_DOCK;
  }

  // One entry per registered id, exactly like getZoneAssignments() — an
  // explicit 'dock' rather than an omission, so the persisted map is a full
  // picture rather than a diff against a default nobody wrote down.
  function getViewModes() {
    const map = {};
    for (const id of toolWindows.keys()) { map[id] = getViewMode(id); }
    return map;
  }

  // What Task 4's host needs to mount a panel: identity plus the render
  // function. Deliberately not the live `el`/`zone`/`active` bookkeeping —
  // a host owns its own DOM.
  function getRegistration(id) {
    const tw = toolWindows.get(id);
    if (!tw) return null;
    return { id: tw.id, title: tw.title, icon: tw.icon, type: tw.type, renderFn: tw.renderFn };
  }

  function panelHostInvoke(cmd, args) {
    if (typeof invokeFn !== 'function') {
      return Promise.reject(new Error('tool-window-manager: no invoke available'));
    }
    try {
      return Promise.resolve(invokeFn(cmd, args));
    } catch (err) {
      return Promise.reject(err);
    }
  }

  function setViewMode(id, mode) {
    const tw = toolWindows.get(id);
    if (!tw) return;
    const next = mode === VIEW_MODE_WINDOW ? VIEW_MODE_WINDOW : VIEW_MODE_DOCK;
    if (next === getViewMode(id)) return;
    if (next === VIEW_MODE_WINDOW) enterWindowMode(tw);
    else dockFromWindowMode(id);
  }

  // Drop the manager's DOM for a window without touching its zone assignment:
  // the zone it is in stays the zone it docks back into.
  function detachFromZone(tw) {
    const zone = zones[tw.zone];
    if (zone && zone.activeId === tw.id) zone.activeId = null;
    disposeWindowRender(tw);
    if (tw.el && tw.el.parentNode) tw.el.parentNode.removeChild(tw.el);
    tw.el = null;
    tw.renderRootEl = null;
  }

  function refreshZoneChrome(zoneName) {
    updateZone(zoneName);
    updateSidebar(sideForZone(zoneName));
    updateBottomZone();
    updateStrips();
    if (fitActiveTabFn) fitActiveTabFn();
  }

  // Claim the generation slot for `id` on behalf of ONE open attempt, and
  // return that attempt's issue number for the caller to hand back on
  // resolution. Called at REQUEST time — synchronously, before the invoke —
  // so the window between asking and being answered is never mistaken for
  // "no host is being built".
  function markHostRequested(id) {
    nextHostRequestIssue += 1;
    hostReqIds.set(id, { pendingIssue: nextHostRequestIssue });
    return nextHostRequestIssue;
  }

  // Record the req_id Rust minted — but only if THIS attempt still owns the
  // slot. Identity, not view mode: an attempt that has been superseded (the
  // user docked and popped out again while it was in flight) or abandoned (a
  // dock-back cleared the slot) must discard its answer, however late it
  // arrives. Checking "is this id still in window mode" instead would accept
  // an older call's generation whenever a NEWER pop-out had put the id back
  // into window mode — resurrecting a dead generation for a stale echo to
  // match.
  function markHostOpened(id, issue, reqId) {
    const current = hostReqIds.get(id);
    if (!isPendingHostRequest(current) || current.pendingIssue !== issue) return;
    hostReqIds.set(id, typeof reqId === 'number' ? reqId : undefined);
  }

  function enterWindowMode(tw) {
    const id = tw.id;
    detachFromZone(tw);
    viewModes.set(id, VIEW_MODE_WINDOW);
    hostVisible.set(id, true);
    tw.active = true;
    const issue = markHostRequested(id);
    refreshZoneChrome(tw.zone);
    triggerSave();
    panelHostInvoke('open_panel_host', { toolWindowId: id, title: tw.title })
      .then((reqId) => markHostOpened(id, issue, reqId))
      .catch(() => {
        // The pop-out never happened, so put back what the user was looking
        // at rather than leaving them staring at an empty zone.
        dockFromWindowMode(id, { teardownHost: false });
      });
  }

  // Back into the zone: mode flips first so activate() takes its normal path,
  // and the render goes through the same lazy ensureWindowElement() a first
  // activation uses — a fresh element, a fresh renderFn call.
  //
  // `teardownHost` (the default) DESTROYS the host window via
  // `dock_panel_host`, which now accepts a parent caller naming one of its own
  // popped-out ids (src/panel_host.rs's `resolve_dock_target`). Merely hiding
  // it would be wrong: the panel is mounted and stateful inside that host, so
  // a hide would leave two live instances of one panel — the hidden host's,
  // and the one activate() re-renders into the zone below. Rust answers the
  // destroy by emitting `panel-host-docked` back to this window; by then the
  // mode is already 'dock', so notifyHostDocked's mode guard makes that echo
  // inert rather than a second remount.
  //
  // The remount is deliberately NOT deferred until that event arrives: the
  // command fails whenever no host is live for the id (a mode restored from
  // the saved layout whose host was never summoned, say), and waiting on an
  // event that will never come would strand the panel nowhere.
  function dockFromWindowMode(id, opts) {
    const tw = toolWindows.get(id);
    if (!tw) return;
    const teardownHost = !opts || opts.teardownHost !== false;
    resetToDock(id);
    if (teardownHost) panelHostInvoke('dock_panel_host', { toolWindowId: id }).catch(() => {});
    activate(id);
  }

  // Forget everything about a window's popped-out state without remounting it.
  function resetToDock(id) {
    const tw = toolWindows.get(id);
    viewModes.set(id, VIEW_MODE_DOCK);
    hostVisible.delete(id);
    // The generation goes with it: whatever host was live for this id is on
    // its way out, so a later echo naming it must not match anything.
    hostReqIds.delete(id);
    pendingWindowSummons.delete(id);
    if (tw) tw.active = false;
  }

  function hideWindowHost(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;
    hostVisible.set(id, false);
    tw.active = false;
    updateStrips();
    triggerSave();
    panelHostInvoke('hide_panel_host', { toolWindowId: id }).catch(() => {});
  }

  // Show a popped-out window's host. focus_panel_host answers Err when no host
  // exists for this id in this session — after an app relaunch the mode
  // survived in the saved layout but the window did not — so the summon falls
  // back to building one.
  function summonWindowHost(id) {
    const tw = toolWindows.get(id);
    if (!tw) return;
    pendingWindowSummons.delete(id);
    hostVisible.set(id, true);
    tw.active = true;
    updateStrips();
    triggerSave();
    panelHostInvoke('focus_panel_host', { toolWindowId: id })
      .catch(() => {
        // No host in this session (a mode restored from the saved layout, say):
        // building one mints a NEW generation, so claim the slot before asking.
        const issue = markHostRequested(id);
        return panelHostInvoke('open_panel_host', { toolWindowId: id, title: tw.title })
          .then((reqId) => markHostOpened(id, issue, reqId));
      })
      .catch(() => {
        resetToDock(id);
        refreshZoneChrome(tw.zone);
      });
  }

  // Called once registrations are done. Anything registering later (plugin
  // tool windows arrive asynchronously) is summoned as it registers instead.
  function summonPendingWindowHosts() {
    summonImmediately = true;
    for (const id of Array.from(pendingWindowSummons)) summonWindowHost(id);
  }

  // ---- Panel-host events (emitted by Rust to this, the parent, window) -------

  function notifyHostShown(id) {
    if (!toolWindows.has(id) || getViewMode(id) !== VIEW_MODE_WINDOW) return;
    const tw = toolWindows.get(id);
    hostVisible.set(id, true);
    tw.active = true;
    updateStrips();
    triggerSave();
  }

  function notifyHostHidden(id) {
    if (!toolWindows.has(id) || getViewMode(id) !== VIEW_MODE_WINDOW) return;
    const tw = toolWindows.get(id);
    hostVisible.set(id, false);
    tw.active = false;
    updateStrips();
    triggerSave();
  }

  // Is a `panel-host-docked` echo naming a host that is no longer the one live
  // for this id?
  //
  // The sequence this exists for: pick Dock, then immediately pick Window
  // again, both inside one IPC round trip. The first dock's echo then lands
  // while the SECOND host is being built — the mode is back to 'window', so
  // the mode guard waves it through, and the remount puts a live panel in the
  // zone while a live host window shows the same panel. Exactly the "two live
  // instances of one stateful panel" the parent-dock ruling set out to kill.
  //
  // Rust stamps the event with the entry's `req_id`
  // (`PanelHostDockedEvent`), which is the same token `open_panel_host`
  // returned, so the comparison is against a generation both sides agree on.
  // Three deliberate accept-anyway cases:
  //   * no entry at all — this id was never opened through this manager in
  //     this session (a mode seeded from the saved layout and summoned by a
  //     successful focus), so there is no generation to disagree with;
  //   * the echo carries no reqId — an older Rust, or a direct call from a
  //     test; fall back to the pre-generation behaviour rather than dropping
  //     a legitimate dock-back;
  //   * the stored generation is not a number — `open_panel_host` answered
  //     with something unexpected; same fallback.
  // The one REJECT that matters is a PENDING slot: a newer host has been
  // requested and has not been minted yet, so any echo arriving with a
  // generation necessarily names an older one.
  function dockedEchoIsStale(id, reqId) {
    if (!hostReqIds.has(id)) return false;
    if (reqId == null) return false;
    const current = hostReqIds.get(id);
    if (isPendingHostRequest(current)) return true;
    if (typeof current !== 'number') return false;
    return current !== reqId;
  }

  // The host asked to come home. It destroys itself, so there is nothing to
  // hide from this side — just remount.
  function notifyHostDocked(id, reqId) {
    if (!toolWindows.has(id) || getViewMode(id) !== VIEW_MODE_WINDOW) return;
    if (dockedEchoIsStale(id, reqId)) return;
    dockFromWindowMode(id, { teardownHost: false });
  }

  // The host self-aborted before mounting anything (its boot found no
  // registration for the id). Nothing is coming back, so reset the trait and
  // leave the window closed in its zone for the rail to reopen.
  //
  // The mode guard matters as much here as in the three handlers above: an
  // abort can land AFTER the parent already docked the window (pick Dock
  // while the host is still booting, and the host aborts into a window that
  // is mounted and active again). Without the guard, resetToDock() would
  // clear `tw.active` underneath a still-mounted panel whose zone.activeId
  // still names it — a dark rail button over a visible panel.
  function notifyHostAborted(id) {
    const tw = toolWindows.get(id);
    if (!tw || getViewMode(id) !== VIEW_MODE_WINDOW) return;
    resetToDock(id);
    refreshZoneChrome(tw.zone);
    triggerSave();
  }

  // ---- Moving ---------------------------------------------------------------

  function moveTo(id, targetZone) {
    targetZone = normalizeZoneName(targetZone);
    if (!targetZone) return;
    const tw = toolWindows.get(id);
    if (!tw || tw.zone === targetZone) return;
    if (!zones[targetZone] || !zones[targetZone].contentEl) return;

    const oldZoneName = tw.zone;
    const oldZone = zones[oldZoneName];

    // Moving a popped-out window re-aims its dock target only: there is no
    // element to reparent and nothing to activate, so it must not take over
    // the destination zone the way a docked move does.
    if (getViewMode(id) === VIEW_MODE_WINDOW) {
      oldZone.windows = oldZone.windows.filter(w => w !== id);
      tw.zone = targetZone;
      zones[targetZone].windows.push(id);
      updateZone(oldZoneName);
      updateZone(targetZone);
      updateSidebar(sideForZone(oldZoneName));
      updateSidebar(sideForZone(targetZone));
      updateBottomZone();
      updateStrips();
      triggerSave();
      return;
    }

    // Remove from old zone
    oldZone.windows = oldZone.windows.filter(w => w !== id);
    if (oldZone.activeId === id) {
      // Same guard as unregister(): a popped-out sibling left behind in the
      // old zone must not be picked as the new active/rendered occupant.
      oldZone.activeId = firstDockableIn(oldZone.windows);
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
    } else if (targetSide === 'bottom') {
      panelState.bottom.visible = true;
    }

    updateZone(oldZoneName);
    updateZone(targetZone);
    updateSidebar(sideForZone(oldZoneName));
    updateSidebar(sideForZone(targetZone));
    updateBottomZone();
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
        headerEl.className = 'zone-header tl-toolwindow__header';
        zone.el.insertBefore(headerEl, zone.el.firstChild);
      }
      headerEl.style.display = '';
      headerEl.innerHTML = '';
      const titleSpan = document.createElement('span');
      titleSpan.className = 'zone-header-title';
      titleSpan.textContent = activeTw ? activeTw.title : '';
      const actionsEl = document.createElement('span');
      actionsEl.className = 'tl-toolwindow__header-actions';
      const gearBtn = document.createElement('button');
      gearBtn.className = 'tl-icon-btn';
      gearBtn.title = 'Options';
      if (window.tlIcon) {
        gearBtn.appendChild(window.tlIcon.create('gear', { size: 16, alt: 'Options' }));
      } else {
        gearBtn.textContent = '⚙';
      }
      gearBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        if (!zone.activeId) return;
        // Position from the button's own rect, not e.clientX/Y — a
        // keyboard-activated click (Enter/Space) synthesizes a click event
        // with clientX/clientY at 0,0 in most webviews, which would open the
        // menu at the top-left corner instead of near the button.
        const rect = gearBtn.getBoundingClientRect();
        showContextMenu(rect.left, rect.bottom, zone.activeId);
      });
      const hideBtn = document.createElement('button');
      hideBtn.className = 'tl-icon-btn';
      hideBtn.title = 'Hide';
      if (window.tlIcon) {
        hideBtn.appendChild(window.tlIcon.create('hideToolWindow', { size: 16, alt: 'Hide' }));
      } else {
        hideBtn.textContent = '—';
      }
      hideBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        if (zone.activeId) deactivate(zone.activeId);
      });
      actionsEl.appendChild(gearBtn);
      actionsEl.appendChild(hideBtn);
      headerEl.appendChild(titleSpan);
      headerEl.appendChild(actionsEl);
      headerEl.oncontextmenu = (e) => { e.preventDefault(); if (zone.activeId) showContextMenu(e.clientX, e.clientY, zone.activeId); };
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

  // The bottom zone is a left/right pair like the sidebars, just laid out
  // horizontally instead of vertically — mirrors updateSidebar().
  function updateBottomZone() {
    if (!bottomZoneWrapEl) return;
    const appRoot = document.getElementById('app');
    const zenActive = !!(appRoot && appRoot.classList.contains('zen-mode'));
    const leftZone = zones['bottom-left'];
    const rightZone = zones['bottom-right'];
    const leftActive = leftZone.activeId !== null;
    const rightActive = rightZone.activeId !== null;
    const shouldShow = !zenActive && !!(panelState.bottom.visible && (leftActive || rightActive));
    bottomZoneWrapEl.classList.toggle('hidden', !shouldShow);

    if (bottomZoneDividerEl) {
      if (leftActive && rightActive) bottomZoneDividerEl.classList.remove('hidden');
      else bottomZoneDividerEl.classList.add('hidden');
    }

    if (leftZone.el && rightZone.el) {
      if (leftActive && !rightActive) {
        leftZone.el.style.flex = '1';
        rightZone.el.style.flex = '0';
      } else if (!leftActive && rightActive) {
        leftZone.el.style.flex = '0';
        rightZone.el.style.flex = '1';
      } else if (leftActive && rightActive) {
        const lf = parseFloat(leftZone.el.style.flex) || 0;
        const rf = parseFloat(rightZone.el.style.flex) || 0;
        if (lf < 0.1 || rf < 0.1) {
          const ratio = lastSplitRatios.bottom || 0.5;
          leftZone.el.style.flex = ratio.toString();
          rightZone.el.style.flex = (1 - ratio).toString();
        }
      }
    }
    if (fitActiveTabFn) fitActiveTabFn();
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
    const bottomBarH = clamp(Math.round(vh * 0.12), 70, 140);
    const bottomBarY = Math.max(bottomY + zoneH + pad, vh - bottomBarH - pad);

    return {
      'left-top': { left: leftX, top: topY, width: zoneW, height: zoneH },
      'left-bottom': { left: leftX, top: bottomY, width: zoneW, height: zoneH },
      'right-top': { left: rightX, top: topY, width: zoneW, height: zoneH },
      'right-bottom': { left: rightX, top: bottomY, width: zoneW, height: zoneH },
      'bottom-left':  { left: pad, top: bottomBarY, width: Math.round((vw - pad * 2 - gap) / 2), height: Math.min(bottomBarH, Math.max(40, vh - bottomBarY - pad)) },
      'bottom-right': { left: pad + Math.round((vw - pad * 2 - gap) / 2) + gap, top: bottomBarY, width: Math.round((vw - pad * 2 - gap) / 2), height: Math.min(bottomBarH, Math.max(40, vh - bottomBarY - pad)) },
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
        topSection.appendChild(makeStripBtn(wid, topZone, false, side));
      }
      stripEl.appendChild(topSection);

      // Bottom section — windows assigned to the bottom zone (pushed to bottom)
      const botSection = document.createElement('div');
      botSection.className = 'strip-section strip-section-bottom';
      for (const wid of botZone.windows) {
        botSection.appendChild(makeStripBtn(wid, botZone, false, side));
      }
      stripEl.appendChild(botSection);
    }

    const bottomStripEl = strips.bottom;
    if (bottomStripEl) {
      bottomStripEl.innerHTML = '';
      // Interim shim: only bottom-left's windows show up in the strip until
      // Task 4 replaces this block with a real two-section layout.
      const bottomZone = zones['bottom-left'];
      const hasWindows = bottomZone.windows.length > 0;
      bottomStripEl.classList.toggle('hidden', !hasWindows);
      for (const wid of bottomZone.windows) {
        bottomStripEl.appendChild(makeStripBtn(wid, bottomZone, true, 'bottom'));
      }
    }
  }

  // `side` is the panel side ('left' | 'right' | 'bottom') this button's zone
  // belongs to. The `active` class must reflect real on-screen visibility —
  // not just tw.active — so a strip tab for a zone whose panel is currently
  // hidden (e.g. bottom zone toggled closed) never renders as filled/active.
  function makeStripBtn(windowId, zone, horizontal, side) {
    const tw = toolWindows.get(windowId);
    if (!tw) return document.createTextNode('');

    // A popped-out window's rail button reflects its HOST window's visibility,
    // which is independent of whether this window's docked panel is showing.
    const isActive = getViewMode(windowId) === VIEW_MODE_WINDOW
      ? hostVisible.get(windowId) === true
      : (tw.active && isPanelVisible(side));
    const btn = document.createElement('button');
    btn.className = 'strip-btn' + (horizontal ? ' strip-btn--horizontal' : '') + (isActive ? ' active' : '');
    if (tw.icon && window.tlIcon) {
      btn.appendChild(window.tlIcon.create(tw.icon, { size: 16, alt: '' }));
    }
    const labelSpan = document.createElement('span');
    labelSpan.className = 'strip-btn-label';
    labelSpan.textContent = tw.title;
    btn.appendChild(labelSpan);
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
    btn.addEventListener('contextmenu', (e) => { e.preventDefault(); showContextMenu(e.clientX, e.clientY, windowId); });
    return btn;
  }

  // ---- Context menu ("Move to <zone>" + View Mode + Hide) --------------------
  // Renders through the shared window.tlMenu component
  // (styles/design-system/components/menu.css, app/ui/tl-menu.js). The
  // current zone is included in the target list (rather than omitted, as the
  // old hand-rolled menu did) so it can carry a checked/current indicator via
  // tlMenu's `checked` item property; it's also disabled since moving a
  // window to the zone it's already in is a no-op.
  //
  // The View Mode choice is TWO FLATTENED items rather than a submenu: tl-menu
  // has no submenu support, and the trait is not worth growing the shared
  // component for. The current mode gets the same checked+disabled treatment
  // the current zone gets, for the same reason.
  //
  // Split out from showContextMenu() as pure item-building (no tlMenu, no DOM)
  // for scripts/tests/test_panel_host.mjs — the trait check is "every
  // registered id, built-in or plugin, gets both entries".
  function buildContextMenuItems(windowId) {
    const tw = toolWindows.get(windowId);
    if (!tw) return null;

    const targets = [
      { zone: 'left-top',     label: 'Left (Top)' },
      { zone: 'left-bottom',  label: 'Left (Bottom)' },
      { zone: 'right-top',    label: 'Right (Top)' },
      { zone: 'right-bottom', label: 'Right (Bottom)' },
      { zone: 'bottom-left',  label: 'Bottom (Left)' },
      { zone: 'bottom-right', label: 'Bottom (Right)' },
    ];

    const items = targets.map((t) => {
      const isCurrent = t.zone === tw.zone;
      return {
        label: 'Move to ' + t.label,
        checked: isCurrent,
        disabled: isCurrent,
        onSelect: () => moveTo(windowId, t.zone),
      };
    });

    const mode = getViewMode(windowId);
    items.push({ separator: true });
    items.push({
      label: 'View Mode: Dock',
      checked: mode === VIEW_MODE_DOCK,
      disabled: mode === VIEW_MODE_DOCK,
      onSelect: () => setViewMode(windowId, VIEW_MODE_DOCK),
    });
    items.push({
      label: 'View Mode: Window',
      checked: mode === VIEW_MODE_WINDOW,
      disabled: mode === VIEW_MODE_WINDOW,
      onSelect: () => setViewMode(windowId, VIEW_MODE_WINDOW),
    });

    items.push({ separator: true });
    items.push({ label: 'Hide', onSelect: () => deactivate(windowId) });

    return items;
  }

  function showContextMenu(x, y, windowId) {
    if (!window.tlMenu || typeof window.tlMenu.open !== 'function') {
      console.error('tool-window-manager: window.tlMenu is unavailable');
      return;
    }

    const items = buildContextMenuItems(windowId);
    if (!items) return;

    window.tlMenu.open({
      x,
      y,
      items,
      ariaLabel: 'Tool window actions',
      routerName: 'twm-move-menu',
    });
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
    const horizontal = side === 'bottom';
    const dividerEl = horizontal ? bottomZoneDividerEl : sidebars[side].dividerEl;
    const firstZoneEl = horizontal ? zones['bottom-left'].el : zones[side + '-top'].el;
    const secondZoneEl = horizontal ? zones['bottom-right'].el : zones[side + '-bottom'].el;
    if (!dividerEl || !firstZoneEl || !secondZoneEl) return;

    let dragging = false, startPos = 0, startFirstFlex = 0, startSecondFlex = 0;

    dividerEl.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      dividerEl.setPointerCapture(e.pointerId);
      dragging = true;
      startPos = horizontal ? e.clientX : e.clientY;
      const firstSize = horizontal ? firstZoneEl.offsetWidth : firstZoneEl.offsetHeight;
      const secondSize = horizontal ? secondZoneEl.offsetWidth : secondZoneEl.offsetHeight;
      const total = firstSize + secondSize;
      startFirstFlex = total > 0 ? firstSize / total : 0.5;
      startSecondFlex = 1 - startFirstFlex;
      dividerEl.classList.add('dragging');
      beginResizeDrag();
      document.body.style.cursor = horizontal ? 'col-resize' : 'row-resize';
      document.body.style.userSelect = 'none';
    });

    dividerEl.addEventListener('pointermove', (e) => {
      if (!dragging) return;
      const container = firstZoneEl.parentElement;
      const containerSize = horizontal
        ? container.clientWidth - dividerEl.offsetWidth
        : container.clientHeight - dividerEl.offsetHeight;
      if (containerSize <= 0) return;
      const delta = (horizontal ? e.clientX : e.clientY) - startPos;
      const newFirstRatio = Math.max(0.15, Math.min(0.85, startFirstFlex + delta / containerSize));
      firstZoneEl.style.flex = newFirstRatio.toString();
      secondZoneEl.style.flex = (1 - newFirstRatio).toString();
      lastSplitRatios[side] = newFirstRatio;
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

  function openWindowModeIdInZone(zone) {
    if (!zone) return null;
    for (const wid of zone.windows) {
      if (getViewMode(wid) === VIEW_MODE_WINDOW && hostVisible.get(wid) === true) return wid;
    }
    return null;
  }

  // A zone's fallback pick when a panel is revealed with nothing active in it.
  // Popped-out windows are skipped: revealing a sidebar must never yank an OS
  // window onto the screen.
  function firstDockableIn(list) {
    if (!list) return null;
    for (const wid of list) {
      if (getViewMode(wid) === VIEW_MODE_DOCK) return wid;
    }
    return null;
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

    if (side === 'bottom') {
      // Same pattern as the left/right branch below, generalized to a pair:
      // reveal-with-nothing-active tries the left half before the right half.
      const leftZone = zones['bottom-left'];
      const rightZone = zones['bottom-right'];
      if (panelState.bottom.visible && leftZone.activeId === null && rightZone.activeId === null) {
        const candidate = firstDockableIn(leftZone.windows) || firstDockableIn(rightZone.windows);
        if (candidate) activate(candidate);
      }
      if (panelState.bottom.visible) {
        updateZone('bottom-left');
        updateZone('bottom-right');
      }
      updateBottomZone();
      updateStrips();
      if (!opts || opts.save !== false) triggerSave();
      return;
    }

    if (panelState[side].visible) {
      const topZone = zones[side + '-top'];
      const botZone = zones[side + '-bottom'];
      if (topZone && botZone && topZone.activeId === null && botZone.activeId === null) {
        const candidate = firstDockableIn(topZone.windows) || firstDockableIn(botZone.windows);
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

  // A zone that holds windows but has none active is a zone the user closed, and
  // that has to survive a restart. Emitting nothing for it made "closed" and
  // "never configured" the same bytes on disk, so the next boot re-opened it.
  // An empty value records the closed state; zones with no windows at all stay
  // absent, since there is nothing there to have an opinion about.
  function getActiveZoneAssignments() {
    const map = {};
    for (const zoneName of ZONE_IDS) {
      const zone = zones[zoneName];
      if (!zone) continue;
      const activeId = zone.activeId;
      if (typeof activeId === 'string' && activeId.length > 0) {
        map[zoneName] = activeId;
      } else if (zone.windows.length > 0) {
        // A popped-out window is open, it is just open somewhere else. Record
        // it here (the zone has no docked window competing for the slot) so
        // the next boot can tell "was showing, summon it" from "was closed,
        // wait for the rail" — the same distinction an empty string draws for
        // a docked window.
        map[zoneName] = openWindowModeIdInZone(zone) || '';
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
    const blEl = zones['bottom-left'].el;
    const brEl = zones['bottom-right'].el;
    if (blEl && brEl) {
      const lf = parseFloat(blEl.style.flex) || 1;
      const rf = parseFloat(brEl.style.flex) || 1;
      ratios.bottom = lf / (lf + rf);
    }
    return ratios;
  }

  function setSplitRatio(side, ratio) {
    const firstEl = side === 'bottom' ? zones['bottom-left'].el : zones[side + '-top'].el;
    const secondEl = side === 'bottom' ? zones['bottom-right'].el : zones[side + '-bottom'].el;
    if (firstEl && secondEl && ratio > 0 && ratio < 1) {
      firstEl.style.flex = ratio.toString();
      secondEl.style.flex = (1 - ratio).toString();
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
    normalizeZoneName,
    setPersistedZones,
    setPersistedActiveZoneWindows,
    setPersistedPanelVisibility,
    setPersistedViewModes,
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
    // View mode (pop-out) trait
    getViewMode,
    getViewModes,
    setViewMode,
    getRegistration,
    summonPendingWindowHosts,
    notifyHostShown,
    notifyHostHidden,
    notifyHostDocked,
    notifyHostAborted,
    // Exposed for scripts/tests/test_panel_host.mjs: showContextMenu's pure
    // item list, so the "every registered id carries the trait" check needs no
    // popup DOM.
    buildContextMenuItems,
    showContextMenu,
  };
})(window);
