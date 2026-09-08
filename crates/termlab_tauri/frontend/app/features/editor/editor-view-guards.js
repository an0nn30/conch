// Keeps CodeMirror's coordinate lookups off an editor view that is not on
// screen.
//
// The bug this exists for, in full:
//
// An editor tab that is not the active one is hidden by CSS — tab-manager.js
// toggles `.tab-tree-root.active`, and layout.css gives the inactive root
// `display: none`. Its EditorView is untouched: the DOM is still built, the
// state is still current, only the boxes are gone, so every
// getClientRects() inside it comes back empty.
//
// CodeMirror's own lint extension (features/editor/lsp-diagnostics.js mounts
// `linter(null, …)`, and @codemirror/lint's `lintExtensions` includes
// `hoverTooltip(lintTooltip)`) arms a 300ms dwell timer on any mousemove over
// the editor. Nothing cancels that timer when the pane is hidden — and hiding
// a pane is exactly what `gd`, `gr`, `<C-o>`, `<C-i>` and an ordinary tab
// click all do, well inside 300ms of the pointer having been over the code.
// When the timer fires, HoverPlugin.startHover calls `view.posAtCoords(...)`
// on a view with no layout, and:
//
//   * posAtCoords hands the line tile to InlineCoordsScan.scanTile;
//   * scan() finds no rectangle overlapping, above OR below the query, so it
//     takes its `if (!below && !above) return { i: positions[0], after: false }`
//     branch — and `positions[0]` there is the tile's DOCUMENT OFFSET, not an
//     index into its children. (This is a CodeMirror bug; the returned `i` is
//     an index everywhere else.)
//   * scanTile then does `let child = tile.children[scan.i]` and calls
//     `child.isText()`.
//
// For any line whose start offset is past its child count — every line but the
// first, in practice — `child` is undefined, and the app's status banner shows
//
//     Frontend error: TypeError: undefined is not an object (evaluating 'n.isText')
//
// The fix is one shim, on our own view object: while the content has no
// layout, `posAtCoords` answers null instead of reaching the scan. null is not
// a new state for any caller — it is what posAtCoords already returns for a
// position its DOM does not cover, and every caller in @codemirror/view
// already handles it (startHover returns early, moveToLineBoundary falls back
// to the line end, drawSelection's wrappedLine keeps its input range). A view
// that IS laid out is never touched: the guard delegates unchanged.
//
// Scope, stated honestly: the shim is an own property on the view, so it
// catches every caller that goes through `view.posAtCoords` — the lint hover,
// mouse selection, drag-and-drop, drawSelection's wrapped-line probe, and our
// own two call sites in lsp-tooltips.js and lsp-navigation.js.
// @codemirror/view's `moveVertically` calls the module-private function
// directly and so bypasses it; that path does not throw (it passes a scan
// direction, which walks the query off the end of the height map and returns
// the document end) and is left alone.
(function initTermLabEditorViewGuards(global) {
  'use strict';

  // Whether this view's content is actually laid out. A `display: none`
  // ancestor and a detached node both produce zero client rects; a visible
  // content element always has at least one, however far it is scrolled out of
  // sight.
  //
  // Errs towards "yes" whenever it cannot tell (no view, no measuring API):
  // this predicate exists to SUPPRESS work, and a wrong "no" would silently
  // break coordinate lookups in an environment we simply failed to measure.
  function hasLayout(view) {
    try {
      const dom = view && view.contentDOM;
      if (!dom) return true;
      if (dom.isConnected === false) return false;
      if (typeof dom.getClientRects !== 'function') return true;
      const rects = dom.getClientRects();
      return !!rects && rects.length > 0;
    } catch (_) {
      return true;
    }
  }

  // Install the shim. Idempotent: a second call on the same view is a no-op,
  // so wrapping can never stack. Returns whether it installed anything.
  function guardPosAtCoords(view) {
    if (!view || typeof view.posAtCoords !== 'function') return false;
    if (view.posAtCoords.termlabGuarded === true) return false;
    const original = view.posAtCoords.bind(view);
    // `precise` is forwarded rather than defaulted here — passing it through
    // as undefined lets CodeMirror's own default (true) apply, which is what a
    // one-argument caller expects.
    const guarded = function guardedPosAtCoords(coords, precise) {
      if (!hasLayout(view)) return null;
      return original(coords, precise);
    };
    guarded.termlabGuarded = true;
    view.posAtCoords = guarded;
    return true;
  }

  // Everything createEditorView applies to a freshly built view. One entry
  // point so a future guard is added in one place and picked up by every
  // creation path at once.
  function install(view) {
    return guardPosAtCoords(view);
  }

  global.termlabEditorViewGuards = {
    hasLayout,
    guardPosAtCoords,
    install,
  };
})(window);
