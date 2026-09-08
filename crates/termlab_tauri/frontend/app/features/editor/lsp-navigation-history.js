// This window's back/forward navigation history.
//
// Split out of lsp-navigation.js, which orchestrates definition requests and
// jumps; this half is only the bookkeeping: what a location IS, how many are
// remembered, and which stack an entry moves to when the user steps.
//
// Per webview window on purpose. Document OWNERSHIP is app-wide, but "where I
// was" is not: two windows are two reading positions, and a shared stack would
// make Back in one window yank the other one's editor around. Entries record
// the owner (window label and pane) they were taken against as a preference;
// the routing decision on every jump still belongs to editor-service, because
// the document may have moved windows since.
//
// Bounded at 100 per stack: deep enough to keep working across a long reading
// session, small enough that it cannot grow without limit.
(function initTermLabLspNavigationHistory(global) {
  'use strict';

  const MAX_ENTRIES = 100;

  const backStack = [];
  const forwardStack = [];

  function stackFor(direction) {
    return direction === 'forward' ? forwardStack : backStack;
  }

  function otherStack(direction) {
    return direction === 'forward' ? backStack : forwardStack;
  }

  function pushBounded(stack, entry) {
    stack.push(entry);
    while (stack.length > MAX_ENTRIES) stack.shift();
  }

  function clamp(value, low, high) {
    return Math.min(Math.max(value, low), high);
  }

  function positionAt(document, offset) {
    const helper = global.termlabLspPosition;
    return helper ? helper.positionAt(document, offset) : { line: 0, character: 0 };
  }

  function samePosition(position) {
    return { line: position.line, character: position.character };
  }

  // Where a jump is starting from. `pos` is the position the request was made
  // at — the caret for F12, the clicked character for Command-click — because
  // that, not wherever the caret drifts to while the server thinks, is where
  // Back should return to. A non-empty selection is preserved as the range so
  // Back restores it too. An untitled or remote buffer has no URI to come back
  // to and yields null rather than a half-entry.
  function capture(pane, pos, windowLabel) {
    const uri = global.termlabLspUri;
    if (!pane || pane.kind !== 'editor' || !pane.view || pane.remote || !pane.filePath) return null;
    if (!uri || typeof uri.pathToUri !== 'function') return null;
    const state = pane.view.state;
    if (!state || !state.doc) return null;
    const document = state.doc;
    const main = state.selection.main;
    const at = Number.isInteger(pos) ? clamp(pos, 0, document.length) : main.head;
    const position = positionAt(document, at);
    const range = main.empty
      ? { start: samePosition(position), end: samePosition(position) }
      : { start: positionAt(document, main.from), end: positionAt(document, main.to) };
    return {
      uri: uri.pathToUri(pane.filePath),
      position,
      range,
      owner: { windowLabel: windowLabel || null, paneId: String(pane.paneId) },
    };
  }

  // An entry, as the thing a jump takes: a path for editor-service and a range
  // to select once it is open.
  function entryTarget(entry) {
    const uri = global.termlabLspUri;
    return {
      uri: entry.uri,
      path: uri && typeof uri.uriToPath === 'function' ? uri.uriToPath(entry.uri) : entry.uri,
      range: entry.range,
    };
  }

  // A completed definition jump. The forward stack is discarded, exactly as a
  // browser does: Forward from here would lead somewhere the user chose to
  // leave.
  function record(entry) {
    if (!entry) return false;
    pushBounded(backStack, entry);
    forwardStack.length = 0;
    return true;
  }

  function peek(direction) {
    const stack = stackFor(direction);
    return stack.length ? stack[stack.length - 1] : null;
  }

  // Consume the entry a step landed on, putting where the user WAS on the
  // opposite stack. Called only after the jump actually happened — a step that
  // failed must consume nothing, or a second press becomes a jump two places
  // back.
  function advance(direction, here) {
    const stack = stackFor(direction);
    if (!stack.length) return false;
    stack.pop();
    if (here) pushBounded(otherStack(direction), here);
    return true;
  }

  // Whether two entries name the same place. The switch recorder uses it to
  // avoid stacking a location the top of the trail already holds.
  function equals(a, b) {
    if (!a || !b) return false;
    return a.uri === b.uri
      && a.position.line === b.position.line
      && a.position.character === b.position.character;
  }

  function state() {
    return { back: backStack.slice(), forward: forwardStack.slice() };
  }

  function reset() {
    backStack.length = 0;
    forwardStack.length = 0;
  }

  global.termlabLspNavigationHistory = {
    MAX_ENTRIES,
    capture,
    entryTarget,
    record,
    peek,
    advance,
    equals,
    state,
    reset,
  };
})(window);
