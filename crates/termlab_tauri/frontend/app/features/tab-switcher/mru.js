(function initTermLabTabMru(global) {
  'use strict';

  // Most-recently-used activation history for the ctrl+tab switcher. Pure
  // bookkeeping: the runtime feeds it activation/close events, order() maps
  // the live tab list into MRU order when the switcher opens.
  function create() {
    let history = [];

    function touch(tabId) {
      history = [tabId, ...history.filter((id) => id !== tabId)];
    }

    function remove(tabId) {
      history = history.filter((id) => id !== tabId);
    }

    // MRU-ordered view of the ids that exist right now. Ids the history has
    // never seen (tabs opened before this module loaded, or activation events
    // that were missed) append after the known ones in their given order, so
    // every live tab always shows up exactly once.
    function order(currentIds) {
      const current = Array.isArray(currentIds) ? currentIds : [];
      const known = history.filter((id) => current.includes(id));
      const unseen = current.filter((id) => !known.includes(id));
      return [...known, ...unseen];
    }

    return { touch, remove, order };
  }

  // Wrap-around selection stepping for the switcher list.
  function stepIndex(length, index, delta) {
    if (!length || length <= 0) return 0;
    return ((index + delta) % length + length) % length;
  }

  global.termlabTabMru = {
    create,
    stepIndex,
  };
})(window);
