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

    return {
      dw: Math.round((wantCols - cols) * cellWidth),
      dh: Math.round((wantRows - rows) * cellHeight),
    };
  }

  exports.termlabWindowSize = { sizeDelta };
})(window);
