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
  // The markdown preview, when a pane has one. Keyed by view like the
  // compartments above, and simply ABSENT for every other file — which is how
  // "the preview is only ever offered for markdown" is enforced structurally
  // rather than by a flag every call site has to remember to check.
  const previews = new WeakMap();
  // Everything a mounted preview needs UNDONE: the element the iframe lives in,
  // the scroll listener bound to the view, and the mode control. Kept beside
  // the controller so teardown is one lookup and cannot forget any of it.
  const previewMounts = new WeakMap();

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

  // Go to Definition's chooser. Same contract as the three above: [] when the
  // module or the bundle's tooltip exports are missing, so a stale vendor
  // bundle costs the chooser and nothing else. It carries its own precedence —
  // its keydown handler must beat vim while the chooser is open — so where it
  // sits in the list below is not what makes it heard.
  function navigationExtensions() {
    return global.termlabLspNavigation
      && typeof global.termlabLspNavigation.extensions === 'function'
      ? global.termlabLspNavigation.extensions()
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
          ...navigationExtensions(),
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
          previewWatcher,
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

  // Back to a pane with no preview at all. Every mode class goes: their stated
  // purpose is to make the CURRENT mode legible in the DOM, and a pane with
  // nothing to be in a mode of has none.
  function clearPreviewLayout(view) {
    const container = paneContainer(view);
    if (!container || !container.classList) return;
    for (const key of Object.keys(MODE_CLASSES)) container.classList.remove(MODE_CLASSES[key]);
  }

  // The layout is a class on that container, not a re-parent: the EditorView
  // stays exactly where it was mounted, so toggling modes cannot cost the undo
  // history, the selection or the scroll position.
  function applyPreviewLayout(view, mode) {
    const container = paneContainer(view);
    if (!container || !container.classList) return;
    clearPreviewLayout(view);
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

  function requestFrame(fn) {
    return typeof global.requestAnimationFrame === 'function'
      ? global.requestAnimationFrame(fn)
      : global.setTimeout(fn, 0);
  }

  function cancelFrame(handle) {
    if (typeof global.cancelAnimationFrame === 'function') global.cancelAnimationFrame(handle);
    else global.clearTimeout(handle);
  }

  // Scroll sync's real driver.
  //
  // The update listener alone is not enough: wheel and trackpad scrolling in
  // CodeMirror moves the VIEWPORT and does not reliably mark the update
  // geometryChanged, so hanging sync off that flag leaves Split mode's
  // headline behaviour dead for the most common way anyone scrolls — while
  // arrow keys (which set selectionSet) keep working, so the gap is invisible
  // to a test written around them. scrollDOM's own scroll event fires
  // unconditionally for wheel, trackpad and scrollbar drags.
  //
  // Bound only for a pane that actually has a preview, and returned as an
  // unbind so it is torn down on the same path that clears the debounce timer
  // and destroys the frame.
  function bindScrollSync(view, controller) {
    const scroller = view.scrollDOM;
    if (!scroller || typeof scroller.addEventListener !== 'function') return null;
    let pendingFrame = null;
    const onScroll = () => {
      // Coalesced to one sync per animation frame: a flung trackpad fires this
      // far more often than the preview can usefully be moved.
      if (pendingFrame !== null) return;
      pendingFrame = requestFrame(() => {
        pendingFrame = null;
        controller.scrollToLine(topVisibleLine(view));
      });
    };
    scroller.addEventListener('scroll', onScroll);
    return () => {
      if (pendingFrame !== null) cancelFrame(pendingFrame);
      pendingFrame = null;
      if (typeof scroller.removeEventListener === 'function') {
        scroller.removeEventListener('scroll', onScroll);
      }
    };
  }

  // Land every mode change in one place.
  //
  // The shortcut, the View menu, the toolbar and the initial mount all arrive
  // here, so the layout class and the control's pressed state can never
  // disagree with the controller about which mode the pane is in. Callers hand
  // in the mode the controller APPLIED, not the one they asked for — an
  // unknown value degrades to 'editor' inside preview-mode.js, and the DOM has
  // to follow the real state rather than the requested one.
  function afterModeChange(view, mode) {
    applyPreviewLayout(view, mode);
    const mount = previewMounts.get(view);
    if (mount && mount.toolbar) mount.toolbar.setMode(mode);
    return mode;
  }

  // The toolbar's click target, and the reason a click cannot diverge from the
  // keystroke: both end at afterModeChange with a mode the controller applied.
  function setPaneMode(view, mode) {
    const controller = previews.get(view);
    if (!controller) return null;
    return afterModeChange(view, controller.setMode(mode));
  }

  // The visible route into the three modes, mounted into the PANE rather than
  // into the preview host: it has to be there in editor mode too, where that
  // host is hidden. A control you can only see once you have already found the
  // feature would not fix the discoverability problem it exists for.
  //
  // Absent module (or a refused build) simply means no control — the same
  // quiet degradation as a missing markdown bundle, and the shortcut still
  // works.
  function mountToolbar(view, container, controller) {
    const module = global.termlabPreviewToolbar;
    if (!module || typeof module.createToolbar !== 'function') return null;
    return module.createToolbar(container, {
      readMode: () => controller.mode(),
      onSelect: (mode) => { setPaneMode(view, mode); },
    });
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
    previewMounts.set(view, {
      host,
      unbindScroll: bindScrollSync(view, controller),
      toolbar: mountToolbar(view, container, controller),
    });
    return controller;
  }

  function destroyPreview(view) {
    const controller = previews.get(view);
    if (!controller) return;
    controller.destroy();
    previews.delete(view);
    const mount = previewMounts.get(view);
    if (mount) {
      if (typeof mount.unbindScroll === 'function') mount.unbindScroll();
      if (mount.toolbar) mount.toolbar.destroy();
      if (mount.host && mount.host.parentNode) mount.host.parentNode.removeChild(mount.host);
    }
    previewMounts.delete(view);
    clearPreviewLayout(view);
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

    return afterModeChange(view, controller.setMode(mode));
  }

  // Editor -> Split -> Preview -> Editor. Returns the new mode, or null when
  // the pane has no preview to toggle — which is what the keyboard shortcut
  // reads to decide whether to consume the keystroke or let it fall through.
  function togglePreview(view) {
    const controller = previews.get(view);
    if (!controller) return null;
    return afterModeChange(view, controller.cycle());
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
    setPreviewMode,
    togglePreview,
    previewMode,
  };
})(window);
