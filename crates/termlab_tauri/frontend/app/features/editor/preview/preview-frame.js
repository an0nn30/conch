// The sandboxed surface the rendered markdown is displayed in.
//
// sandbox="allow-same-origin" WITHOUT allow-scripts is the security design:
//
//   - no allow-scripts  => nothing in the document executes. A <script> that
//                          survived sanitizing is inert, and so is every
//                          inline handler.
//   - allow-same-origin => the PARENT can reach into the frame's DOM, which
//                          is how scroll sync and link interception work with
//                          no message protocol at all.
//
// A postMessage design would need a script INSIDE the frame to receive
// messages, which would require allow-scripts and undo the first property.
// Do not add it.
//
// Design tokens do not cascade across documents, so the palette is snapshotted
// from the parent's computed style and written into the frame as literal
// values on every setContent — which is also how a theme change restyles it.
//
// The stylesheet itself is fetched by the PARENT, not linked from inside the
// frame. Two reasons: a srcdoc document's base URL for resolving relative
// URLs is inherited from the parent document (index.html, served from the
// frontend root), not from this script's own path, so a <link> written as if
// relative to this file resolves against the wrong base; and even a
// corrected relative <link> would depend on how Tauri's custom asset
// protocol handles a subresource fetch issued from inside a sandboxed
// sub-document, which nothing else in this codebase exercises or can verify.
// The parent document is ordinary same-origin app code with no sandbox
// restrictions, so it can just read its own asset directly with fetch() and
// hand the frame the text — the frame never resolves anything itself. This
// also keeps markdown-preview.css the one editable copy of these rules
// instead of a second, JS-embedded copy that would silently drift out of
// sync with it.
(function initTermLabPreviewFrame(global) {
  'use strict';

  const BASE_TOKENS = [
    '--tl-bg', '--tl-fg', '--tl-fg-muted', '--tl-accent', '--tl-border',
    '--tl-row-hover', '--tl-danger', '--tl-terminal-bg',
  ];

  // The ANSI syntax-accent vars the stylesheet's .tok-* rules read for fence
  // highlighting (`var(--green, var(--tl-accent))` etc). Snapshotted ONLY
  // when the appearance is not light, mirroring the `syntax` branch in
  // features/editor/theme.js: under Light that file ignores the ANSI vars
  // entirely and uses the plain app tokens instead, because the ANSI palette
  // stays tuned for a dark canvas and would look wrong pasted onto a light
  // document. Omitting these vars under Light lets the stylesheet's own
  // fallbacks apply, and those fallbacks were chosen to match theme.js's
  // light-branch choices exactly (string/number/bool/typeName/className ->
  // --tl-accent, function -> --tl-fg) — so a change to either side of that
  // correspondence needs a matching change to the other.
  const SYNTAX_TOKENS = ['--green', '--yellow', '--blue', '--cyan'];

  // Same appearance check as features/editor/theme.js's isLightAppearance():
  // a missing global.termlabAppearance (or a stub without current()) is
  // treated as not-light, matching that file's own default.
  function isLightAppearance() {
    const appearance = global.termlabAppearance;
    return !!(appearance
      && typeof appearance.current === 'function'
      && appearance.current() === 'light');
  }

  function tokenList() {
    return isLightAppearance() ? BASE_TOKENS : BASE_TOKENS.concat(SYNTAX_TOKENS);
  }

  const CSS_PATH = 'styles/design-system/components/markdown-preview.css';

  // The last text the shared load produced, good or empty. Frames created
  // after the load settles read this synchronously instead of waiting again.
  let sharedCssText = '';

  // Fetches markdown-preview.css once and caches the settled promise
  // module-wide, so every frame shares one load instead of issuing its own.
  // Missing `fetch` (as in the test harness's minimal sandbox) and a failed
  // request are both handled the same way: warn and resolve to '' — the
  // preview must still render its content, just unstyled, rather than never
  // rendering at all.
  function loadSharedCss() {
    if (typeof global.fetch !== 'function') {
      if (global.console && typeof global.console.warn === 'function') {
        global.console.warn('termlabPreviewFrame: fetch unavailable, markdown-preview.css not loaded');
      }
      return Promise.resolve('');
    }
    let promise;
    try {
      promise = global.fetch(CSS_PATH).then((res) => {
        if (!res || !res.ok) {
          throw new Error(`markdown-preview.css: ${res ? res.status : 'no response'}`);
        }
        return res.text();
      });
    } catch (err) {
      promise = Promise.reject(err);
    }
    return promise
      .then((text) => {
        sharedCssText = text;
        return text;
      })
      .catch((err) => {
        if (global.console && typeof global.console.warn === 'function') {
          global.console.warn('termlabPreviewFrame: failed to load markdown-preview.css', err);
        }
        sharedCssText = '';
        return '';
      });
  }

  // Kicked off once at module init so the fetch is already in flight (or
  // already settled) by the time the first frame needs it.
  const sharedCssReady = loadSharedCss();

  function defaultReadToken(name) {
    const styles = global.getComputedStyle(global.document.documentElement);
    return styles.getPropertyValue(name).trim();
  }

  function paletteCss(readToken) {
    const lines = tokenList()
      .map((name) => {
        const value = readToken(name);
        return value ? `  ${name}: ${value};` : '';
      })
      .filter(Boolean);
    return `:root {\n${lines.join('\n')}\n}`;
  }

  function createFrame(hostEl, deps) {
    if (!hostEl) return null;
    const options = deps || {};
    const readToken = typeof options.readToken === 'function' ? options.readToken : defaultReadToken;
    const onLinkClick = typeof options.onLinkClick === 'function' ? options.onLinkClick : null;
    // The injection seam: a caller (namely the test suite) can supply the
    // stylesheet text directly via deps.css, bypassing the shared fetch
    // entirely so the caller never touches the network.
    const explicitCss = typeof options.css === 'string';

    const doc = global.document;
    const iframe = doc.createElement('iframe');
    iframe.setAttribute('sandbox', 'allow-same-origin');
    iframe.setAttribute('class', 'md-preview-frame');
    iframe.setAttribute('title', 'Markdown preview');
    hostEl.appendChild(iframe);

    // Best text available right now: the explicit override, or whatever the
    // shared load has produced so far (possibly still '' if it hasn't
    // settled yet). `render` is called again once it does.
    let css = explicitCss ? options.css : sharedCssText;
    let lastHtml = null;
    let destroyed = false;
    // Monotonic, and written into the shell below purely so that two
    // consecutive renders can never produce a byte-identical srcdoc.
    //
    // Whether a webview re-navigates when srcdoc is assigned the value it
    // already holds is engine-dependent, and the parent DEPENDS on the load
    // event firing — it is what tells it the images of the new document exist
    // and need resolving. Concrete failure without this: Save As from
    // `~/a/notes.md` to `~/b/notes.md` with identical bytes. The caller clears
    // its image cache and repoints its base directory, then this writes the
    // same shell; a frame that does not re-navigate keeps `~/a`'s already
    // resolved images and shows the wrong ones when both directories hold a
    // same-named file. A token removes the question instead of betting on the
    // answer.
    let renderToken = 0;

    function frameDoc() {
      return iframe.contentDocument || null;
    }

    function render() {
      renderToken += 1;
      const shell = [
        // The token lives INSIDE <head>, not ahead of the doctype: a comment
        // before the doctype puts the document into quirks mode.
        '<!doctype html><html><head><meta charset="utf-8">',
        `<!-- termlab-render ${renderToken} -->`,
        `<style>${paletteCss(readToken)}\n${css}</style>`,
        '</head><body class="md-preview-body">',
        lastHtml || '',
        '</body></html>',
      ].join('');
      // srcdoc rather than document.write: the content is replaced atomically
      // and the sandbox attribute is re-applied to the new document.
      iframe.setAttribute('srcdoc', shell);
    }

    function setContent(html) {
      lastHtml = typeof html === 'string' ? html : '';
      render();
    }

    if (!explicitCss) {
      // The shared load may still be in flight, or may already have settled
      // (successfully or not) before this frame existed. Either way, once it
      // resolves, pick up the result and re-render whatever the most recent
      // setContent call was — an unstyled first paint that corrects itself is
      // fine; a frame that never gets styled because it missed the load is
      // not.
      sharedCssReady.then((text) => {
        // The frame may have been torn down before the fetch settled — a
        // detached iframe object is harmless to keep writing to, but there's
        // no reason to touch it once its own destroy() has run.
        if (destroyed) return;
        css = text;
        if (lastHtml !== null) render();
      });
    }

    // Links are intercepted in the PARENT because the frame cannot run script.
    // Without this an external link would try to navigate the frame itself,
    // replacing the preview with a web page inside the app.
    function bindLinks() {
      const d = frameDoc();
      if (!d || !onLinkClick) return;
      d.addEventListener('click', (event) => {
        let node = event.target;
        while (node && node.tagName !== 'A') node = node.parentNode;
        if (!node) return;
        const href = node.getAttribute('href');
        if (!href) return;
        event.preventDefault();
        onLinkClick(href);
      });
    }

    iframe.addEventListener('load', bindLinks);

    // Scroll sync: find the last mapped block at or above `line` and bring it
    // to the top. One-directional by design — the preview never scrolls the
    // editor, so there is no feedback loop to guard against.
    function scrollToLine(line) {
      const d = frameDoc();
      if (!d || typeof d.querySelectorAll !== 'function') return;
      const blocks = d.querySelectorAll('[data-src-line]');
      let target = null;
      for (const el of blocks) {
        if (Number(el.getAttribute('data-src-line')) <= line) target = el;
        else break;
      }
      if (target && typeof target.scrollIntoView === 'function') {
        target.scrollIntoView({ block: 'start' });
      }
    }

    function destroy() {
      destroyed = true;
      if (typeof iframe.remove === 'function') iframe.remove();
      else if (hostEl.removeChild) hostEl.removeChild(iframe);
    }

    return { setContent, scrollToLine, destroy, element: iframe };
  }

  global.termlabPreviewFrame = { createFrame };
})(window);
