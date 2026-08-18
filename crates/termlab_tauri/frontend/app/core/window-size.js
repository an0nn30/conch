// Measure the window's real terminal geometry — never change it.
//
// Windows open at their final size: Rust converts the configured columns x
// lines to pixels before creation, using either the measurement this module
// persisted on an earlier launch or native metrics parsed from the bundled
// font file (src/font_metrics.rs). Nothing in the frontend resizes a visible
// window; a launch that opens slightly off (first ever run under a new font)
// STAYS at that size and simply persists a fresh measurement so the next
// launch opens right. That trade — one launch a couple of columns off, in
// exchange for windows that never visibly move — is deliberate; the previous
// design corrected the window after show and every variant of it animated.
(function (exports) {
  'use strict';

  /**
   * The persisted geometry that lets the next launch open exactly: logical
   * pixels per cell, and how much window is not terminal. Returns null unless
   * every input was actually measured — a cell size divided out of zero
   * columns is Infinity, and persisting that would make every future launch
   * open at a garbage size.
   */
  function metricsFor(current, innerLogical) {
    const c = current || {};
    const cols = Number(c.cols) || 0;
    const rows = Number(c.rows) || 0;
    const width = Number(c.width) || 0;
    const height = Number(c.height) || 0;
    const winW = Number(innerLogical && innerLogical.width) || 0;
    const winH = Number(innerLogical && innerLogical.height) || 0;
    if (cols <= 0 || rows <= 0 || width <= 0 || height <= 0) return null;
    if (winW <= 0 || winH <= 0) return null;

    // Prefer the renderer's own cell size when the caller measured it —
    // dividing the host by the column count inflates the cell by whatever
    // partial cell the host happens to contain.
    const cellWidth = Number(c.cellWidth) > 0 ? Number(c.cellWidth) : width / cols;
    const cellHeight = Number(c.cellHeight) > 0 ? Number(c.cellHeight) : height / rows;
    const chromeWidth = winW - cols * cellWidth;
    const chromeHeight = winH - rows * cellHeight;
    // A terminal reported as larger than its window is a mid-layout read,
    // not a measurement.
    if (chromeWidth < 0 || chromeHeight < 0) return null;
    return { cellWidth, cellHeight, chromeWidth, chromeHeight };
  }

  /**
   * The renderer's own CSS cell size, when xterm exposes it. Private API by
   * necessity — it is the only place the true cell exists — so every level is
   * guarded and the caller falls back to division when it is absent.
   */
  function rendererCellSize(term) {
    try {
      const cell = term
        && term._core
        && term._core._renderService
        && term._core._renderService.dimensions
        && term._core._renderService.dimensions.css
        && term._core._renderService.dimensions.css.cell;
      if (cell && Number(cell.width) > 0 && Number(cell.height) > 0) {
        return { width: Number(cell.width), height: Number(cell.height) };
      }
    } catch (_e) { /* private API moved — division fallback covers it */ }
    return null;
  }

  exports.termlabWindowSize = { metricsFor, rendererCellSize };
})(window);
