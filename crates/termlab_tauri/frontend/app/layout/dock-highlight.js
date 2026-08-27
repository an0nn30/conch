(function initTermLabDockHighlight(global) {
  'use strict';

  // IntelliJ-style drop guide: the zone under the pointer maps to the real
  // screen area the tool window would occupy, and the drag overlay paints
  // exactly that rectangle. Pure math over layout metrics so it stays
  // testable — the manager supplies live measurements:
  //   main:            #main-area bounding rect
  //   bottomRect:      #bottom-zone-wrap rect when the bar is visible, null
  //                    when hidden (fall back to bottomHeight over main)
  //   left/rightStripWidth:  visible rail widths (0 when hidden)
  //   left/rightPanelWidth:  sidebar widths (saved size, or a default when
  //                          the panel is closed)
  //   bottomHeight:    fallback bar height for a hidden bottom bar
  function computeDockHighlightRect(zone, m) {
    if (!m || !m.main) return null;
    const main = m.main;

    if (zone === 'left-top' || zone === 'left-bottom') {
      const band = {
        left: main.left + (m.leftStripWidth || 0),
        top: main.top,
        width: m.leftPanelWidth,
        height: main.height,
      };
      return halfBandVertical(band, zone === 'left-top');
    }

    if (zone === 'right-top' || zone === 'right-bottom') {
      const band = {
        left: main.left + main.width - (m.rightStripWidth || 0) - m.rightPanelWidth,
        top: main.top,
        width: m.rightPanelWidth,
        height: main.height,
      };
      return halfBandVertical(band, zone === 'right-top');
    }

    if (zone === 'bottom-left' || zone === 'bottom-right') {
      const band = m.bottomRect
        ? { left: m.bottomRect.left, top: m.bottomRect.top, width: m.bottomRect.width, height: m.bottomRect.height }
        : { left: main.left, top: main.top + main.height - m.bottomHeight, width: main.width, height: m.bottomHeight };
      const half = band.width / 2;
      return {
        left: zone === 'bottom-left' ? band.left : band.left + half,
        top: band.top,
        width: half,
        height: band.height,
      };
    }

    return null;
  }

  function halfBandVertical(band, top) {
    const half = band.height / 2;
    return {
      left: band.left,
      top: top ? band.top : band.top + half,
      width: band.width,
      height: half,
    };
  }

  global.termlabDockHighlight = {
    computeDockHighlightRect,
  };
})(window);
