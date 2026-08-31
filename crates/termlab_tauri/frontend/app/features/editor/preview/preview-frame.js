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
(function initTermLabPreviewFrame(global) {
  'use strict';

  const TOKENS = [
    '--tl-bg', '--tl-fg', '--tl-fg-muted', '--tl-accent', '--tl-border',
    '--tl-row-hover', '--tl-danger', '--tl-terminal-bg',
  ];

  // The markdown-preview.css rules, inlined rather than loaded via <link>.
  //
  // A srcdoc document's relative URLs resolve against the PARENT document's
  // base URL (index.html, served from the frontend root), not against this
  // script's own path — so a "../../../styles/..." href written as if it
  // were relative to this file breaks (it walks three directories above the
  // frontend root and 404s), and even a corrected root-relative href would
  // depend on how Tauri's custom asset protocol resolves link fetches inside
  // a sandboxed sub-document, which nothing else in this codebase exercises.
  // Inlining sidesteps both: no network fetch, no protocol-specific
  // behaviour to trust. Keep this in sync with markdown-preview.css by hand
  // — there is no bundler here to do it automatically.
  const PREVIEW_CSS = [
    '.md-preview-body {',
    '  margin: 0;',
    '  padding: 20px 28px;',
    '  background: var(--tl-bg);',
    '  color: var(--tl-fg);',
    '  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;',
    '  font-size: 14px;',
    '  line-height: 1.6;',
    '  overflow-wrap: break-word;',
    '}',
    '.md-preview-body h1, .md-preview-body h2 {',
    '  border-bottom: 1px solid var(--tl-border);',
    '  padding-bottom: 0.3em;',
    '}',
    '.md-preview-body h1 { font-size: 1.9em; margin: 0.6em 0 0.5em; }',
    '.md-preview-body h2 { font-size: 1.5em; margin: 1.2em 0 0.5em; }',
    '.md-preview-body h3 { font-size: 1.25em; margin: 1.1em 0 0.4em; }',
    '.md-preview-body a { color: var(--tl-accent); }',
    '.md-preview-body blockquote {',
    '  margin: 0.8em 0;',
    '  padding: 0.2em 1em;',
    '  border-left: 3px solid var(--tl-border);',
    '  color: var(--tl-fg-muted);',
    '}',
    '.md-preview-body table { border-collapse: collapse; margin: 1em 0; display: block; overflow-x: auto; }',
    '.md-preview-body th, .md-preview-body td { border: 1px solid var(--tl-border); padding: 6px 12px; }',
    '.md-preview-body th { background: var(--tl-row-hover); font-weight: 600; }',
    '.md-preview-body tr:nth-child(2n) td { background: var(--tl-row-hover); }',
    '.md-preview-body img { max-width: 100%; }',
    '.md-preview-body hr { border: 0; border-top: 1px solid var(--tl-border); margin: 1.5em 0; }',
    '.md-preview-body code {',
    '  font-family: ui-monospace, "JetBrains Mono", Menlo, Consolas, monospace;',
    '  font-size: 0.9em;',
    '  background: var(--tl-row-hover);',
    '  padding: 0.15em 0.4em;',
    '  border-radius: 3px;',
    '}',
    '.md-preview-body pre.md-code {',
    '  background: var(--tl-terminal-bg, var(--tl-bg));',
    '  border: 1px solid var(--tl-border);',
    '  border-radius: 4px;',
    '  padding: 12px 14px;',
    '  overflow-x: auto;',
    '}',
    '.md-preview-body pre.md-code code { background: none; padding: 0; font-size: 0.875em; }',
    '.md-preview-body .md-img-blocked {',
    '  display: inline-block;',
    '  padding: 2px 8px;',
    '  border: 1px dashed var(--tl-border);',
    '  border-radius: 3px;',
    '  color: var(--tl-fg-muted);',
    '  font-size: 0.85em;',
    '}',
    '.md-preview-body .tok-keyword { color: var(--tl-accent); }',
    '.md-preview-body .tok-string { color: var(--green, var(--tl-accent)); }',
    '.md-preview-body .tok-comment { color: var(--tl-fg-muted); font-style: italic; }',
    '.md-preview-body .tok-number,',
    '.md-preview-body .tok-bool { color: var(--yellow, var(--tl-accent)); }',
    '.md-preview-body .tok-variableName { color: var(--tl-fg); }',
    '.md-preview-body .tok-function { color: var(--blue, var(--tl-fg)); }',
    '.md-preview-body .tok-typeName,',
    '.md-preview-body .tok-className { color: var(--cyan, var(--tl-accent)); }',
    '.md-preview-body .tok-operator { color: var(--tl-fg-muted); }',
    '.md-preview-body .tok-invalid { color: var(--tl-danger); }',
  ].join('\n');

  function defaultReadToken(name) {
    const styles = global.getComputedStyle(global.document.documentElement);
    return styles.getPropertyValue(name).trim();
  }

  function paletteCss(readToken) {
    const lines = TOKENS
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

    const doc = global.document;
    const iframe = doc.createElement('iframe');
    iframe.setAttribute('sandbox', 'allow-same-origin');
    iframe.setAttribute('class', 'md-preview-frame');
    iframe.setAttribute('title', 'Markdown preview');
    hostEl.appendChild(iframe);

    function frameDoc() {
      return iframe.contentDocument || null;
    }

    function setContent(html) {
      const shell = [
        '<!doctype html><html><head><meta charset="utf-8">',
        `<style>${paletteCss(readToken)}\n${PREVIEW_CSS}</style>`,
        '</head><body class="md-preview-body">',
        typeof html === 'string' ? html : '',
        '</body></html>',
      ].join('');
      // srcdoc rather than document.write: the content is replaced atomically
      // and the sandbox attribute is re-applied to the new document.
      iframe.setAttribute('srcdoc', shell);
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
      if (typeof iframe.remove === 'function') iframe.remove();
      else if (hostEl.removeChild) hostEl.removeChild(iframe);
    }

    return { setContent, scrollToLine, destroy, element: iframe };
  }

  global.termlabPreviewFrame = { createFrame };
})(window);
