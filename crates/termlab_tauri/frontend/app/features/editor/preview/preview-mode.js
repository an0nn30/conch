// The Editor / Split / Preview state for one editor pane.
//
// Pure state, deliberately: layout and rendering read from it, nothing is
// stored here about the DOM. The mode is per-pane runtime state and is not
// persisted — `[editor] preview_default_mode` only seeds it.
(function initTermLabPreviewMode(global) {
  'use strict';

  const MODES = ['editor', 'split', 'preview'];

  function normalise(mode) {
    // An unknown value degrades to 'editor' rather than throwing.
    // preview_default_mode is hand-edited in config.toml, and a typo there
    // must not stop markdown files from opening.
    return MODES.includes(mode) ? mode : 'editor';
  }

  function createModeState(initial) {
    let mode = normalise(initial);
    return {
      mode: () => mode,
      set(next) { mode = normalise(next); return mode; },
      cycle() {
        mode = MODES[(MODES.indexOf(mode) + 1) % MODES.length];
        return mode;
      },
      showsEditor: () => mode !== 'preview',
      showsPreview: () => mode !== 'editor',
    };
  }

  global.termlabPreviewMode = { createModeState, MODES };
})(window);
