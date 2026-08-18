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

  function vimExtensions(enabled) {
    return global.termlabVimMode && typeof global.termlabVimMode.vimExtensions === 'function'
      ? global.termlabVimMode.vimExtensions(enabled)
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

    const fontComp = new CM.Compartment();
    const themeComp = new CM.Compartment();
    const vimComp = new CM.Compartment();
    const themeExtensions = global.termlabEditorTheme
      ? global.termlabEditorTheme.buildTheme()
      : [];

    let dirty = false;
    const dirtyWatcher = CM.EditorView.updateListener.of((update) => {
      if (!update.docChanged || dirty) return;
      dirty = true;
      onDirtyChange(true);
    });

    const view = new CM.EditorView({
      parent: hostEl,
      state: CM.EditorState.create({
        doc: typeof opts.doc === 'string' ? opts.doc : '',
        extensions: [
          // FIRST, and it has to stay first. CodeMirror resolves keymaps in
          // extension order, so anything ahead of vim wins the keystroke:
          // put this after CM.keymap.of([...defaultKeymap]) below and `i`
          // types an "i" instead of entering insert mode, `dd` deletes
          // nothing, and the feature looks broken rather than absent.
          // test_editor_vim_glue.mjs pins the position.
          vimComp.of(vimExtensions(opts.vimMode === true)),
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
          languageExtension(opts.filename || ''),
          themeComp.of(themeExtensions),
          fontComp.of([]),
          dirtyWatcher,
        ],
      }),
    });

    fontCompartments.set(view, fontComp);
    themeCompartments.set(view, themeComp);
    vimCompartments.set(view, vimComp);
    // Callers clear dirty after a save; expose the reset without exposing state.
    view.termlabResetDirty = () => {
      dirty = false;
      onDirtyChange(false);
    };
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

  global.termlabEditorPane = {
    createEditorView,
    destroyEditorView,
    setFontSize,
    refreshTheme,
    setVimMode,
  };
})(window);
