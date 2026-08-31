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
      const el = frame && frame.element;
      if (el && typeof el.addEventListener === 'function') {
        // srcdoc parses ASYNCHRONOUSLY. The <img> elements of a freshly
        // injected document do not exist until the frame has loaded, so
        // walking it straight after setContent() would query the PREVIOUS
        // document and silently resolve nothing. Hanging resolution off the
        // load event is also what makes it happen after injection, so the
        // text is readable while the images are still in flight.
        el.addEventListener('load', () => {
          resolveImages(images ? images.currentGeneration() : 0);
        });
      }
      return frame;
    }

    // Swap each unresolved <img> for a data: URI. Every result is
    // generation-checked twice — once inside the resolver and once here on
    // return — so a render that has been superseded can never paint into the
    // frame that replaced it.
    async function resolveImages(generation) {
      if (!images) return;
      const doc = frameDoc();
      if (!doc || typeof doc.querySelectorAll !== 'function') return;
      const pending = Array.prototype.slice.call(doc.querySelectorAll('img[src]'))
        .filter((img) => !String(img.getAttribute('src') || '').startsWith('data:'));
      await Promise.all(pending.map(async (img) => {
        const uri = await images.resolve(
          img.getAttribute('src'), source.binding, generation, source.docPath,
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
      if (images) images.nextGeneration();
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
