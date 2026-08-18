// Apply the configured default window size, expressed in terminal cells.
//
// Other terminals let you say "102 columns by 46 lines" and mean it. The Rust
// side cannot: cell width and height depend on the font, which only exists
// inside the webview, so `lib.rs` opens the window at a rough estimate
// (columns * 8px, lines * 16px) that is wrong for any font that is not exactly
// 8x16. This corrects that estimate once the terminal has measured itself.
//
// The correction is expressed as a *delta* on the window's current size rather
// than an absolute size, so whatever chrome happens to be present — tab bar,
// status bar, zen mode's grab strip, open tool windows — is accounted for
// without this module knowing anything about it.
(function (exports) {
  'use strict';

  /**
   * Pixel delta needed to reach a target cell count.
   *
   * Pure so the arithmetic can be tested: given what the terminal currently
   * measures and what we want, return how much bigger or smaller the window
   * must get. Returns null when the inputs cannot yield a sane answer — a
   * terminal that has not been fitted yet reports 0 columns, and dividing by
   * that produces an infinity that would resize the window off-screen.
   */
  function sizeDelta(current, target) {
    const c = current || {};
    const t = target || {};
    const cols = Number(c.cols) || 0;
    const rows = Number(c.rows) || 0;
    const width = Number(c.width) || 0;
    const height = Number(c.height) || 0;
    const wantCols = Number(t.cols) || 0;
    const wantRows = Number(t.rows) || 0;

    // 0 means "leave the window as the OS sized it".
    if (wantCols <= 0 || wantRows <= 0) return null;
    if (cols <= 0 || rows <= 0 || width <= 0 || height <= 0) return null;

    const cellWidth = width / cols;
    const cellHeight = height / rows;
    if (!isFinite(cellWidth) || !isFinite(cellHeight)) return null;

    // Already exactly right — say so, so the caller can stop.
    if (cols === wantCols && rows === wantRows) return { dw: 0, dh: 0 };

    // Aim for the MIDDLE of the band that yields the target count, not its
    // lower edge. xterm's fit floors (cols = floor(width / cellWidth)), so
    // sizing to exactly `target * cellWidth` lands on the boundary and a
    // sub-pixel shortfall silently costs a column — and because the next pass
    // computes the same sub-pixel delta, it never recovers. Half a cell of
    // headroom puts us safely inside the band in either direction.
    const targetW = (wantCols + 0.5) * cellWidth;
    const targetH = (wantRows + 0.5) * cellHeight;
    return {
      dw: Math.round(targetW - width),
      dh: Math.round(targetH - height),
    };
  }

  /**
   * The persisted geometry that lets the NEXT launch skip the correction:
   * logical pixels per cell, and how much window is not terminal. Returns
   * null unless every input was actually measured — a cell size divided out
   * of zero columns is Infinity, and persisting that would make every future
   * launch open at a garbage size.
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

    const cellWidth = width / cols;
    const cellHeight = height / rows;
    const chromeWidth = winW - width;
    const chromeHeight = winH - height;
    // A terminal reported as larger than its window is a mid-layout read,
    // not a measurement.
    if (chromeWidth < 0 || chromeHeight < 0) return null;
    return { cellWidth, cellHeight, chromeWidth, chromeHeight };
  }

  /**
   * Resolve once `measure()` reports a size different from `before`, plus one
   * extra frame so the rAF-coalesced terminal refit can consume the new size.
   *
   * This is the condition the correction loop must synchronise on. It used to
   * sleep a flat 60ms after setSize; whenever the OS resize landed later than
   * that, the next pass read the NEW host width against the OLD column count,
   * derived a garbage cell size from the mismatched pair, and issued an
   * overshooting second resize that a later pass walked back — the visible
   * grow-then-shrink on every launch, and a loop that exhausted its pass
   * budget without ever converging (so the metrics were never persisted and
   * the next launch repeated the whole dance).
   *
   * Returns false after `maxFrames` without a change, so a resize the OS
   * swallowed stops the loop instead of wedging it.
   */
  async function waitForSizeChange(measure, before, raf, maxFrames) {
    const b = before || {};
    for (let i = 0; i < maxFrames; i++) {
      await new Promise((resolve) => raf(resolve));
      const now = typeof measure === 'function' ? measure() : null;
      if (now && (now.width !== b.width || now.height !== b.height)) {
        // One more frame: the refit is coalesced to the next animation frame,
        // so measuring immediately would repeat the exact stale-pair bug this
        // function exists to prevent.
        await new Promise((resolve) => raf(resolve));
        return true;
      }
    }
    return false;
  }

  exports.termlabWindowSize = { sizeDelta, metricsFor, waitForSizeChange };
})(window);
