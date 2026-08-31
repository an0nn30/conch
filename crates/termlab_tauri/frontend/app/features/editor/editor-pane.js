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
  // Save As renames a live pane, so the language can no longer be fixed at
  // creation: a scratch saved as `deploy.py` has to start highlighting as
  // Python without losing the document, the selection or the undo history.
  const languageCompartments = new WeakMap();
  // The markdown preview, when a pane has one. Keyed by view like the
  // compartments above, and simply ABSENT for every other file — which is how
  // "the preview is only ever offered for markdown" is enforced structurally
  // rather than by a flag every call site has to remember to check.
  const previews = new WeakMap();
  // The element the preview iframe is mounted into, kept beside its controller
  // so teardown can remove it without re-deriving it from the DOM.
  const previewHosts = new WeakMap();

  const MODE_CLASSES = {
    editor: 'md-mode-editor',
    split: 'md-mode-split',
    preview: 'md-mode-preview',
  };

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
    const languageComp = new CM.Compartment();
    const themeExtensions = global.termlabEditorTheme
      ? global.termlabEditorTheme.buildTheme()
      : [];

    let dirty = false;
    const dirtyWatcher = CM.EditorView.updateListener.of((update) => {
      if (!update.docChanged || dirty) return;
      dirty = true;
      onDirtyChange(true);
    });

    // Both halves no-op until a preview exists, which for a non-markdown pane
    // is never — so this costs a WeakMap miss per update and nothing else.
    // `update.view` rather than a captured binding: the listener is built
    // while the view it belongs to is still being constructed.
    const previewWatcher = CM.EditorView.updateListener.of((update) => {
      const preview = previews.get(update.view);
      if (!preview) return;
      if (update.docChanged) preview.scheduleRender();
      // Editor drives preview and never the reverse, so there is no feedback
      // loop to guard against here.
      if (update.geometryChanged || update.selectionSet) {
        preview.scrollToLine(topVisibleLine(update.view));
      }
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
          languageComp.of(languageExtension(opts.filename || '')),
          themeComp.of(themeExtensions),
          fontComp.of([]),
          dirtyWatcher,
          previewWatcher,
        ],
      }),
    });

    fontCompartments.set(view, fontComp);
    themeCompartments.set(view, themeComp);
    vimCompartments.set(view, vimComp);
    languageCompartments.set(view, languageComp);
    // Callers clear dirty after a save; expose the reset without exposing state.
    view.termlabResetDirty = () => {
      dirty = false;
      onDirtyChange(false);
    };
    return view;
  }

  function destroyEditorView(view) {
    if (!view) return;
    destroyPreview(view);
    if (typeof view.destroy === 'function') view.destroy();
  }

  // ---------------------------------------------------------------------------
  // Markdown preview — mounting and layout only
  // ---------------------------------------------------------------------------
  //
  // What the preview DOES lives in preview/preview-controller.js. This half
  // owns where it hangs in the DOM, which pane it belongs to, and which of the
  // three layouts is showing.

  // The pane element the CodeMirror host and the preview host are siblings in.
  // Derived from the view rather than remembered, so a pane moved by the split
  // system is never wired to the container it used to be in.
  function paneContainer(view) {
    const editorHost = view && view.dom ? view.dom.parentNode : null;
    return editorHost ? editorHost.parentNode : null;
  }

  // The layout is a class on that container, not a re-parent: the EditorView
  // stays exactly where it was mounted, so toggling modes cannot cost the undo
  // history, the selection or the scroll position.
  function applyPreviewLayout(view, mode) {
    const container = paneContainer(view);
    if (!container || !container.classList) return;
    for (const key of Object.keys(MODE_CLASSES)) container.classList.remove(MODE_CLASSES[key]);
    container.classList.add(MODE_CLASSES[mode] || MODE_CLASSES.editor);
  }

  // The source line at the top of the viewport, 0-based to match the
  // `data-src-line` values markdown-it's token maps produce.
  function topVisibleLine(view) {
    if (!view || typeof view.lineBlockAtHeight !== 'function') return -1;
    try {
      const top = view.lineBlockAtHeight(view.scrollDOM.scrollTop).from;
      return view.state.doc.lineAt(top).number - 1;
    } catch (error) {
      // Geometry can be asked for mid-layout. A missed scroll sync is
      // cosmetic and must not take the update cycle down with it.
      return -1;
    }
  }

  function mountPreview(view, source, mode) {
    const module = global.termlabPreviewController;
    const container = paneContainer(view);
    if (!module || typeof module.createController !== 'function' || !container) return null;

    const host = global.document.createElement('div');
    host.className = 'md-preview-host';
    container.appendChild(host);

    const controller = module.createController({
      mountEl: host,
      readDoc: () => view.state.doc.toString(),
      source,
      mode,
    });
    if (!controller) {
      // No vendor bundle (it is gitignored and built by `make frontend-vendor`)
      // or a preview module that never loaded. Leave the pane byte-for-byte
      // the pane it was, rather than a dead host element behind an inert
      // toggle.
      container.removeChild(host);
      return null;
    }
    previews.set(view, controller);
    previewHosts.set(view, host);
    return controller;
  }

  function destroyPreview(view) {
    const controller = previews.get(view);
    if (!controller) return;
    controller.destroy();
    previews.delete(view);
    const host = previewHosts.get(view);
    if (host && host.parentNode) host.parentNode.removeChild(host);
    previewHosts.delete(view);
    applyPreviewLayout(view, 'editor');
  }

  /**
   * Put a pane into `mode`, mounting or tearing down its preview as needed.
   *
   *   source = { filename, docPath, binding }
   *
   * `filename` is what decides whether a preview is offered at all: anything
   * the language map does not call markdown gets none, and a pane that HAD one
   * loses it — which is how `notes.md` saved as `notes.txt` leaves preview
   * mode. Returns the applied mode, or null when the pane has no preview.
   */
  function setPreviewMode(view, mode, source) {
    if (!view) return null;
    const info = source || {};
    const map = global.termlabEditorLanguageMap;
    const markdown = !!(map && typeof map.isMarkdown === 'function' && map.isMarkdown(info.filename));

    if (!markdown) {
      destroyPreview(view);
      return null;
    }

    let controller = previews.get(view);
    if (controller) controller.setSource(info);
    else controller = mountPreview(view, info, mode);
    if (!controller) return null;

    const applied = controller.setMode(mode);
    applyPreviewLayout(view, applied);
    return applied;
  }

  // Editor -> Split -> Preview -> Editor. Returns the new mode, or null when
  // the pane has no preview to toggle — which is what the keyboard shortcut
  // reads to decide whether to consume the keystroke or let it fall through.
  function togglePreview(view) {
    const controller = previews.get(view);
    if (!controller) return null;
    const mode = controller.cycle();
    applyPreviewLayout(view, mode);
    return mode;
  }

  // The pane's current mode, or null when it has no preview.
  function previewMode(view) {
    const controller = previews.get(view);
    return controller ? controller.mode() : null;
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
    // The preview lives in its own document, and design tokens do not cascade
    // across documents — the frame snapshots them on every content write. So
    // it restyles by re-rendering, not by a reconfigure.
    const preview = previews.get(view);
    if (preview) preview.refresh();
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
    setLanguage,
    setPreviewMode,
    togglePreview,
    previewMode,
  };
})(window);
