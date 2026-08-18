// Settings → Editor.
//
// The built-in editor's own preferences. Its font is deliberately NOT here —
// the editor shares the terminal's font by design, so that row lives under
// Terminal and changing it in two places would let them drift apart.
//
// Same shape as the sibling section modules: a render function returning
// truthy when it handled the container, so renderers.js can report a missing
// module rather than silently drawing nothing.
(function initTermLabSettingsSectionsEditor(global) {
  'use strict';

  function renderEditor(container, d) {
    const addSectionLabel = d.addSectionLabel || function () {};
    const addRow = d.addRow;
    const setRowTarget = d.setRowTarget || function () {};
    const makeCheckbox = d.makeCheckbox;
    const pendingSettings = d.pendingSettings;

    if (!addRow || !makeCheckbox || !pendingSettings) return false;

    // A config saved by a build without the [editor] table has no `editor` key
    // at all. Create it rather than dropping the row: the checkbox has to have
    // somewhere to write, and Rust's `#[serde(default)]` fills in the rest.
    if (!pendingSettings.editor || typeof pendingSettings.editor !== 'object') {
      pendingSettings.editor = {};
    }

    addSectionLabel(container, 'Keys');

    const vimCheckbox = makeCheckbox(
      pendingSettings.editor.vim_mode === true,
      (val) => { pendingSettings.editor.vim_mode = val; }
    );
    setRowTarget(
      addRow(
        container,
        'Vim keybindings',
        'Modal editing in the editor. :w saves (uploading a remote file to its host) and :q closes the tab, still asking about unsaved changes',
        vimCheckbox
      ),
      'editor:vim-mode'
    );

    return true;
  }

  global.termlabSettingsSectionsEditor = { renderEditor };
})(window);
