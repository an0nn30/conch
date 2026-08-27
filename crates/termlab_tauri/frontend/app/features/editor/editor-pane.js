// Owns a CodeMirror EditorView inside a pane element.
//
// Deliberately knows nothing about files, saving, or tabs — it renders a
// document and reports when it becomes dirty. editor-service.js supplies the
// meaning; this supplies the surface.
(function initTermLabEditorPane(global) {
  'use strict';

  // Compartments let the font size, theme and vim keymap be reconfigured on a
  // live view without rebuilding its state (which would discard undo history).
  const fontCompartments = new WeakMap();
  const themeCompartments = new WeakMap();
  const vimCompartments = new WeakMap();
  const readOnlyCompartments = new WeakMap();
  // Save As renames a live pane, so the language can no longer be fixed at
  // creation: a scratch saved as `deploy.py` has to start highlighting as
  // Python without losing the document, the selection or the undo history.
  const languageCompartments = new WeakMap();

  function vimExtensions(enabled) {
    return global.termlabVimMode && typeof global.termlabVimMode.vimExtensions === 'function'
      ? global.termlabVimMode.vimExtensions(enabled)
      : [];
  }

  // LSP completion. Returns [] when the module or the bundle's autocomplete
  // export is missing, so a stale vendor bundle costs completion and nothing
  // else. The extensions carry their own precedence (Prec.highest on the
  // keymap) — where they sit in this list is not what makes them heard.
  function completionExtensions() {
    return global.termlabLspCompletion
      && typeof global.termlabLspCompletion.extensions === 'function'
      ? global.termlabLspCompletion.extensions()
      : [];
  }

  // LSP diagnostics. Same contract as completionExtensions: [] when the
  // module or the bundle's lint exports are missing, so a stale vendor bundle
  // costs squiggles and nothing else. No precedence concerns — these are
  // decorations and a gutter, not key handlers.
  function diagnosticsExtensions() {
    return global.termlabLspDiagnostics
      && typeof global.termlabLspDiagnostics.extensions === 'function'
      ? global.termlabLspDiagnostics.extensions()
      : [];
  }

  // LSP hover and signature help. Same contract as the two above: [] when the
  // module or the bundle's tooltip exports are missing, so a stale vendor
  // bundle costs the overlays and nothing else. These DO carry precedence —
  // the Escape handler is a Prec.highest domEventHandler, for the same reason
  // the completion one is — but it travels with the extension, not with the
  // position in this list.
  function tooltipExtensions() {
    return global.termlabLspTooltips
      && typeof global.termlabLspTooltips.extensions === 'function'
      ? global.termlabLspTooltips.extensions()
      : [];
  }

  function languageExtension(filename) {
    const CM = global.CM6;
    const map = global.termlabEditorLanguageMap;
    if (!CM || !map) return [];
    const key = map.languageKeyFor(filename);
    if (!key) return [];
    const entry = CM[key];
    if (!entry) return [];
    // Two shapes arrive here. The @codemirror/lang-* packages export a
    // FUNCTION returning a LanguageSupport. The legacy modes export a plain
    // StreamParser OBJECT, which has to be wrapped in StreamLanguage before
    // CodeMirror will take it. Discriminating on typeof is the whole trick —
    // treating the object as a factory silently yields no highlighting.
    if (typeof entry === 'function') return [entry()];
    return [CM.StreamLanguage.define(entry)];
  }

  function createEditorView(hostEl, options) {
    const CM = global.CM6;
    if (!CM || !hostEl) return null;
    const opts = options || {};
    const onDirtyChange = typeof opts.onDirtyChange === 'function' ? opts.onDirtyChange : () => {};
    const onDocumentTransaction = typeof opts.onDocumentTransaction === 'function'
      ? opts.onDocumentTransaction
      : () => {};

    const fontComp = new CM.Compartment();
    const themeComp = new CM.Compartment();
    const vimComp = new CM.Compartment();
    const languageComp = new CM.Compartment();
    const readOnlyComp = new CM.Compartment();
    const themeExtensions = global.termlabEditorTheme
      ? global.termlabEditorTheme.buildTheme()
      : [];

    let dirty = false;
    const dirtyWatcher = CM.EditorView.updateListener.of((update) => {
      if (!update.docChanged || dirty) return;
      dirty = true;
      onDirtyChange(true);
    });
    const transactionWatcher = CM.EditorView.updateListener.of((update) => {
      if (update.docChanged) onDocumentTransaction(update);
    });

    const view = new CM.EditorView({
      parent: hostEl,
      state: CM.EditorState.create({
        doc: typeof opts.doc === 'string' ? opts.doc : '',
        extensions: [
          // FIRST, and it has to stay first. vim is a ViewPlugin with a
          // `keydown` DOM handler, and the view runs plugin handlers in
          // plugin order — which is extension order at equal precedence.
          // Anything ahead of vim that consumes keydown wins the keystroke,
          // so a plugin placed above this one could stop `i` entering insert
          // mode. (The *keymap* below is a different mechanism: every keymap
          // in the state shares one DOM handler that @codemirror/view
          // registers at Prec.default, BEHIND vim's plugin. That is why the
          // completion extensions cannot own their keys with a keymap and use
          // a Prec.highest domEventHandler instead.)
          // test_editor_vim_glue.mjs pins the position.
          vimComp.of(vimExtensions(opts.vimMode === true)),
          // Position here is not what makes these heard: the completion key
          // handler carries its own Prec.highest, which is what puts it ahead
          // of vim's plugin while the popup is open.
          ...completionExtensions(),
          ...diagnosticsExtensions(),
          ...tooltipExtensions(),
          CM.lineNumbers(),
          CM.highlightActiveLineGutter(),
          CM.highlightSpecialChars(),
          CM.history(),
          CM.foldGutter(),
          CM.drawSelection(),
          CM.rectangularSelection(),
          CM.indentOnInput(),
          CM.bracketMatching(),
          CM.highlightActiveLine(),
          CM.highlightSelectionMatches(),
          CM.keymap.of([
            ...CM.defaultKeymap,
            ...CM.historyKeymap,
            ...CM.searchKeymap,
            ...CM.foldKeymap,
            CM.indentWithTab,
          ]),
          languageComp.of(languageExtension(opts.filename || '')),
          readOnlyComp.of(CM.EditorState.readOnly.of(false)),
          themeComp.of(themeExtensions),
          fontComp.of([]),
          dirtyWatcher,
          transactionWatcher,
        ],
      }),
    });

    fontCompartments.set(view, fontComp);
    themeCompartments.set(view, themeComp);
    vimCompartments.set(view, vimComp);
    languageCompartments.set(view, languageComp);
    readOnlyCompartments.set(view, readOnlyComp);
    // Callers clear dirty after a save; expose the reset without exposing state.
    view.termlabResetDirty = () => {
      dirty = false;
      onDirtyChange(false);
    };
    view.termlabSetReadOnly = (readOnly) => setReadOnly(view, readOnly);
    return view;
  }

  function destroyEditorView(view) {
    if (view && typeof view.destroy === 'function') view.destroy();
  }

  function setFontSize(view, px) {
    const CM = global.CM6;
    const comp = fontCompartments.get(view);
    if (!CM || !view || !comp || !px) return;
    view.dispatch({
      effects: comp.reconfigure(
        CM.EditorView.theme({ '&': { fontSize: `${px}px` } }),
      ),
    });
  }

  function refreshTheme(view) {
    const comp = themeCompartments.get(view);
    if (!view || !comp || !global.termlabEditorTheme) return;
    view.dispatch({ effects: comp.reconfigure(global.termlabEditorTheme.buildTheme()) });
  }

  // Turn vim keybindings on or off on a view that is already open. A
  // compartment reconfigure keeps the document, the selection and the undo
  // history — rebuilding the state would throw all three away, and the
  // setting is meant to take effect without reopening the file.
  function setVimMode(view, enabled) {
    const comp = vimCompartments.get(view);
    if (!view || !comp) return;
    view.dispatch({ effects: comp.reconfigure(vimExtensions(enabled === true)) });
  }

  function setReadOnly(view, readOnly) {
    const CM = global.CM6;
    const comp = readOnlyCompartments.get(view);
    if (!CM || !view || !comp || !CM.EditorState || !CM.EditorState.readOnly) return;
    view.dispatch({ effects: comp.reconfigure(CM.EditorState.readOnly.of(readOnly === true)) });
  }

  // Re-derive the highlighting from a new name on a view that is already
  // open. Save As is the only caller: the pane keeps its document, so this is
  // a compartment reconfigure rather than a fresh state (which would discard
  // undo history and the selection). `filename` may be a bare basename or a
  // full path — languageKeyFor takes the basename either way — and a name
  // with no known language reconfigures to an empty array, which is how a
  // `.py` saved as `.txt` correctly loses its highlighting.
  function setLanguage(view, filename) {
    const comp = languageCompartments.get(view);
    if (!view || !comp) return;
    view.dispatch({ effects: comp.reconfigure(languageExtension(filename || '')) });
  }

  global.termlabEditorPane = {
    createEditorView,
    destroyEditorView,
    setFontSize,
    refreshTheme,
    setVimMode,
    setReadOnly,
    setLanguage,
  };
})(window);
