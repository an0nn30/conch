// SFTP dual-pane splitter. One orientation-agnostic ratio drives both
// layouts: panes receive flex-GROW weights (never a fixed basis), so the same
// split holds whether the panes sit side-by-side in the bottom zone or
// stacked in a sidebar — the zone move needs no JS reconciliation. The ratio
// is read per-drag (not captured at attach time) because the tool window can
// change zones while the panel DOM lives on.
(function initTermLabFilesSplit(global) {
  'use strict';

  const MIN_RATIO = 0.15;
  const MAX_RATIO = 0.85;
  const STORAGE_KEY = 'termlab.files.splitRatio';
  const EVEN_SPLIT = 0.5;

  function clampRatio(value) {
    const ratio = typeof value === 'number' ? value : Number(value);
    if (!Number.isFinite(ratio)) return EVEN_SPLIT;
    return Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio));
  }

  function ratioFromPointer(options) {
    const rect = options.rect;
    const ratio = options.orientation === 'column'
      ? (options.clientY - rect.top) / rect.height
      : (options.clientX - rect.left) / rect.width;
    return clampRatio(ratio);
  }

  // Grow weights are rounded so the complement of e.g. 0.7 is written as 0.3,
  // not 0.30000000000000004.
  function roundWeight(value) {
    return Math.round(value * 10000) / 10000;
  }

  function applyRatio(firstEl, secondEl, ratio) {
    const clamped = clampRatio(ratio);
    firstEl.style.flex = `${roundWeight(clamped)} 1 0px`;
    secondEl.style.flex = `${roundWeight(1 - clamped)} 1 0px`;
  }

  function loadRatio(storage) {
    if (!storage || typeof storage.getItem !== 'function') return EVEN_SPLIT;
    const stored = storage.getItem(STORAGE_KEY);
    if (stored === null || stored === undefined) return EVEN_SPLIT;
    return clampRatio(Number(stored));
  }

  function saveRatio(storage, ratio) {
    if (!storage || typeof storage.setItem !== 'function') return;
    try {
      storage.setItem(STORAGE_KEY, String(clampRatio(ratio)));
    } catch (error) {
      console.warn('files split: could not persist ratio', error);
    }
  }

  function attach(options) {
    const container = options.container;
    const firstEl = options.firstEl;
    const secondEl = options.secondEl;
    const dividerEl = options.dividerEl;
    const storage = options.storage;
    const getOrientation = options.getOrientation;
    if (!container || !firstEl || !secondEl || !dividerEl) {
      throw new TypeError('Files split requires container, panes, and a divider');
    }
    if (typeof getOrientation !== 'function') {
      throw new TypeError('Files split requires a getOrientation function');
    }

    let ratio = loadRatio(storage);
    let dragPointerId = null;
    applyRatio(firstEl, secondEl, ratio);

    function onPointerDown(event) {
      dragPointerId = event.pointerId;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      if (typeof dividerEl.setPointerCapture === 'function') {
        dividerEl.setPointerCapture(event.pointerId);
      }
    }

    function onPointerMove(event) {
      if (dragPointerId === null || event.pointerId !== dragPointerId) return;
      ratio = ratioFromPointer({
        orientation: getOrientation(),
        rect: container.getBoundingClientRect(),
        clientX: event.clientX,
        clientY: event.clientY,
      });
      applyRatio(firstEl, secondEl, ratio);
    }

    function onPointerEnd(event) {
      if (dragPointerId === null || event.pointerId !== dragPointerId) return;
      dragPointerId = null;
      if (typeof dividerEl.releasePointerCapture === 'function') {
        dividerEl.releasePointerCapture(event.pointerId);
      }
      saveRatio(storage, ratio);
    }

    function onDoubleClick() {
      ratio = EVEN_SPLIT;
      applyRatio(firstEl, secondEl, ratio);
      saveRatio(storage, ratio);
    }

    dividerEl.addEventListener('pointerdown', onPointerDown);
    dividerEl.addEventListener('pointermove', onPointerMove);
    dividerEl.addEventListener('pointerup', onPointerEnd);
    dividerEl.addEventListener('pointercancel', onPointerEnd);
    dividerEl.addEventListener('dblclick', onDoubleClick);

    return {
      dispose() {
        dividerEl.removeEventListener('pointerdown', onPointerDown);
        dividerEl.removeEventListener('pointermove', onPointerMove);
        dividerEl.removeEventListener('pointerup', onPointerEnd);
        dividerEl.removeEventListener('pointercancel', onPointerEnd);
        dividerEl.removeEventListener('dblclick', onDoubleClick);
      },
    };
  }

  global.termlabFilesSplit = {
    MIN_RATIO,
    MAX_RATIO,
    STORAGE_KEY,
    clampRatio,
    ratioFromPointer,
    applyRatio,
    loadRatio,
    saveRatio,
    attach,
  };
})(window);
