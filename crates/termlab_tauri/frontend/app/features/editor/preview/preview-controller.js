// One markdown preview, bound to one editor pane.
//
// editor-pane.js owns mounting and layout; everything else the preview needs
// lives here — assembling the parser, the fence highlighter and the image
// resolver, owning the frame, the render debounce and the render generation.
// Keeping it out of editor-pane.js is what lets that file stay a file about
// CodeMirror, and it keeps this wiring testable without an EditorView.
//
// Nothing here throws on a missing dependency. The markdown vendor bundle is
// gitignored and produced by `make frontend-vendor`, so a fresh checkout run
// with plain `cargo run` has no `window.MDLib` at all; createController then
// returns null and the caller leaves the pane in editor mode with the toggle
// inert. Same quiet degradation as editor-service.js's bundleMissing(), minus
// the toast: a markdown file still opens and edits normally, so there is
// nothing to interrupt the user for.
(function initTermLabPreviewController(global) {
  'use strict';

  // Parsing a typical README is sub-millisecond. The debounce is not about
  // parse cost — it is there so image resolution and frame layout do not run
  // on every keystroke, which over SFTP is the difference between one fetch
  // and one per character.
  const RENDER_DEBOUNCE_MS = 150;

  // Only schemes the OS can be handed. Everything else — including relative
  // links to sibling files — stays inert: preview-frame.js has already
  // cancelled the navigation, and opening a file from the preview is a
  // separate feature, not a side effect of clicking a link.
  const EXTERNAL_LINK = /^(https?:|mailto:)/i;

  function invoke(command, args) {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    if (!client || typeof client.invoke !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }
    return client.invoke(command, args);
  }

  // The fence highlighter reuses the EDITOR's grammars (window.CM6) with the
  // markdown bundle's lezer helpers, so a ```rust fence and a .rs file can
  // never disagree. Absent either side, fences fall back to plain escaped
  // text rather than the preview failing.
  function buildHighlighter() {
    const MDLib = global.MDLib;
    const fence = global.termlabFenceHighlight;
    if (!MDLib || !fence || !global.CM6) return null;
    return fence.createHighlighter({
      CM: global.CM6,
      highlightCode: MDLib.highlightCode,
      classHighlighter: MDLib.classHighlighter,
    });
  }

  function buildRenderer() {
    const MDLib = global.MDLib;
    const module = global.termlabMarkdownRenderer;
    if (!MDLib || !module || typeof module.createRenderer !== 'function') return null;
    return module.createRenderer({
      MarkdownIt: MDLib.MarkdownIt,
      DOMPurify: MDLib.DOMPurify,
      taskListsPlugin: MDLib.taskListsPlugin,
      footnotePlugin: MDLib.footnotePlugin,
      highlight: buildHighlighter(),
    });
  }

  function buildImages() {
    const module = global.termlabPreviewImages;
    if (!module || typeof module.createResolver !== 'function') return null;
    return module.createResolver({ invoke });
  }

  function normaliseSource(source) {
    const info = source || {};
    return {
      filename: typeof info.filename === 'string' ? info.filename : '',
      docPath: typeof info.docPath === 'string' ? info.docPath : '',
      binding: info.binding || null,
    };
  }

  function createController(deps) {
    const options = deps || {};
    const mountEl = options.mountEl;
    const readDoc = typeof options.readDoc === 'function' ? options.readDoc : () => '';
    if (!mountEl) return null;

    const renderer = options.renderer || buildRenderer();
    const frames = options.frames || global.termlabPreviewFrame;
    const modes = global.termlabPreviewMode;
    if (!renderer || !frames || typeof frames.createFrame !== 'function' || !modes) return null;

    const images = options.images === undefined ? buildImages() : options.images;
    const state = modes.createModeState(options.mode);
    let source = normaliseSource(options.source);
    let frame = null;
    let renderTimer = null;
    let destroyed = false;
    // The frame's current `load` handler, owned by the most recent render.
    // Held so it can be replaced when a render supersedes it and removed on
    // teardown.
    let loadHandler = null;

    function frameDoc() {
      return frame && frame.element ? frame.element.contentDocument : null;
    }

    // A link click reaches the PARENT (the frame cannot run script) with the
    // navigation already cancelled, so every kind of href has to be answered
    // here or it does nothing at all.
    function openLink(href) {
      const target = String(href || '');
      if (target.startsWith('#')) {
        // An in-document anchor — a README's table of contents. Nothing inside
        // the frame can act on the fragment, so the parent scrolls it.
        const doc = frameDoc();
        const el = doc && typeof doc.getElementById === 'function'
          ? doc.getElementById(target.slice(1))
          : null;
        if (el && typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'start' });
        return;
      }
      if (!EXTERNAL_LINK.test(target)) return;
      const tauri = global.__TAURI__;
      if (tauri && tauri.shell && typeof tauri.shell.open === 'function') tauri.shell.open(target);
    }

    function ensureFrame() {
      if (frame || destroyed) return frame;
      frame = frames.createFrame(mountEl, { onLinkClick: openLink });
      return frame;
    }

    // Arm image resolution for the render that has just written `srcdoc`.
    //
    // srcdoc parses ASYNCHRONOUSLY. The <img> elements of a freshly injected
    // document do not exist until the frame has loaded, so walking it straight
    // after setContent() would query the PREVIOUS document and silently
    // resolve nothing. Hanging resolution off the load event is also what puts
    // it after injection, so the text is readable while the images are still
    // in flight.
    //
    // The generation is CAPTURED here, at the moment the render bumped it, and
    // the previous render's handler is REMOVED. Sampling currentGeneration()
    // inside the handler instead would let a superseded render's late load
    // event pass its own check and paint into the document that is about to be
    // replaced — bounded, cache-served waste, but a weaker invariant than the
    // one this is meant to provide.
    //
    // Deliberately not `{ once: true }`: preview-frame.js re-renders when its
    // shared stylesheet fetch settles, which rewrites srcdoc with the same
    // HTML and so wipes the data: URIs already swapped in. That second load
    // belongs to the SAME generation and has to re-resolve (from cache, at no
    // I/O cost), so the handler stays armed until the next render replaces it.
    function armResolution(generation) {
      const el = frame && frame.element;
      if (!el || typeof el.addEventListener !== 'function') return;
      if (loadHandler && typeof el.removeEventListener === 'function') {
        el.removeEventListener('load', loadHandler);
      }
      loadHandler = () => { resolveImages(generation); };
      el.addEventListener('load', loadHandler);
    }

    function disarmResolution() {
      const el = frame && frame.element;
      if (loadHandler && el && typeof el.removeEventListener === 'function') {
        el.removeEventListener('load', loadHandler);
      }
      loadHandler = null;
    }

    // Give each image a data: URI. `generation` is the one its render
    // captured, and it is re-checked against the resolver on return — so a
    // fetch that outlives its own render drops its result rather than
    // painting it.
    //
    // The path is read from data-img-ref, not from `src`: the sanitizer moves
    // every fetchable source there and removes `src` outright, so an element
    // that reaches this document has no URL for the engine to load. `src` is
    // only ever written here, and only with a data: URI — which is what makes
    // "the preview issues no network requests" a property of the code rather
    // than of a pattern match over URLs.
    async function resolveImages(generation) {
      if (!images) return;
      const doc = frameDoc();
      if (!doc || typeof doc.querySelectorAll !== 'function') return;
      const pending = Array.prototype.slice.call(doc.querySelectorAll('[data-img-ref]'));
      await Promise.all(pending.map(async (img) => {
        const uri = await images.resolve(
          img.getAttribute('data-img-ref'), source.binding, generation, source.docPath,
        );
        if (uri && generation === images.currentGeneration()) img.setAttribute('src', uri);
      }));
    }

    function renderNow() {
      if (destroyed || !state.showsPreview()) return;
      const target = ensureFrame();
      if (!target) return;
      // Bumped before the parse, not after: a fetch still in flight for the
      // previous document is superseded the moment this render begins.
      const generation = images ? images.nextGeneration() : 0;
      armResolution(generation);
      target.setContent(renderer.render(readDoc()));
    }

    function scheduleRender() {
      if (destroyed || !state.showsPreview()) return;
      if (renderTimer) clearTimeout(renderTimer);
      renderTimer = setTimeout(() => {
        renderTimer = null;
        renderNow();
      }, RENDER_DEBOUNCE_MS);
    }

    function setMode(next) {
      const applied = state.set(next);
      // Rendered on every call, not only on a change: entering split from a
      // pane that was edited while in editor mode has to show the current
      // document, and scheduleRender() deliberately does nothing while the
      // preview is hidden.
      renderNow();
      return applied;
    }

    function cycle() {
      state.cycle();
      renderNow();
      return state.mode();
    }

    // Save As rebinds the pane to a new path. Relative image sources resolve
    // from a new base directory and every cached entry is keyed to the old
    // one, so the cache goes with it. Deliberately does NOT render — the
    // caller follows with setMode, which does.
    function setSource(next) {
      source = normaliseSource(next);
      if (images) images.clear();
    }

    function scrollToLine(line) {
      if (!frame || line < 0 || !state.showsPreview()) return;
      frame.scrollToLine(line);
    }

    // A theme change rewrites the app's --tl-* variables, and the frame
    // snapshots those into its own document on every content write (design
    // tokens do not cascade across documents). Re-rendering is how the
    // preview restyles.
    function refresh() {
      renderNow();
    }

    function destroy() {
      destroyed = true;
      if (renderTimer) clearTimeout(renderTimer);
      renderTimer = null;
      disarmResolution();
      if (images) images.clear();
      if (frame) frame.destroy();
      frame = null;
    }

    return {
      mode: () => state.mode(),
      showsEditor: () => state.showsEditor(),
      showsPreview: () => state.showsPreview(),
      setMode,
      cycle,
      setSource,
      scheduleRender,
      renderNow,
      scrollToLine,
      refresh,
      destroy,
    };
  }

  global.termlabPreviewController = { createController, RENDER_DEBOUNCE_MS };
})(window);
