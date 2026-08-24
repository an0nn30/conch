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
    if (!pendingSettings.editor.lsp || typeof pendingSettings.editor.lsp !== 'object') {
      pendingSettings.editor.lsp = {};
    }
    if (!pendingSettings.editor.lsp.languages || typeof pendingSettings.editor.lsp.languages !== 'object') {
      pendingSettings.editor.lsp.languages = {};
    }

    const lsp = pendingSettings.editor.lsp;
    const languages = lsp.languages;
    if (typeof lsp.enabled !== 'boolean') lsp.enabled = true;
    if (typeof lsp.suggestions_while_typing !== 'boolean') lsp.suggestions_while_typing = true;
    for (const key of ['typescript', 'json', 'python', 'rust', 'go', 'clangd', 'java']) {
      if (typeof languages[key] !== 'boolean') languages[key] = true;
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

    addSectionLabel(container, 'Language Services');

    const lspCheckbox = makeCheckbox(
      lsp.enabled,
      (val) => { lsp.enabled = val; }
    );
    setRowTarget(
      addRow(
        container,
        'Enable language services',
        'Enable code intelligence for supported local files',
        lspCheckbox
      ),
      'editor:lsp-enabled'
    );

    const suggestionsCheckbox = makeCheckbox(
      lsp.suggestions_while_typing,
      (val) => { lsp.suggestions_while_typing = val; }
    );
    setRowTarget(
      addRow(
        container,
        'Suggestions while typing',
        'Show completion suggestions as you type',
        suggestionsCheckbox
      ),
      'editor:lsp-suggestions'
    );

    addSectionLabel(container, 'Languages');
    const languageRows = [
      ['typescript', 'TypeScript / JavaScript'],
      ['json', 'JSON'],
      ['python', 'Python'],
      ['rust', 'Rust'],
      ['go', 'Go'],
      ['clangd', 'C / C++'],
      ['java', 'Java'],
    ];
    for (const [key, label] of languageRows) {
      const checkbox = makeCheckbox(languages[key], (val) => { languages[key] = val; });
      setRowTarget(
        addRow(container, label, `Enable language services for ${label}`, checkbox),
        `editor:lsp-language:${key}`
      );
    }

    return true;
  }

  global.termlabSettingsSectionsEditor = { renderEditor };
})(window);
