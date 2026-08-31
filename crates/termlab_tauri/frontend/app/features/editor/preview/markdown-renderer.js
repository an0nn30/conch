// Markdown source -> sanitized HTML. Pure: no DOM ownership, no Tauri, no I/O.
//
// Dependencies arrive through createRenderer rather than being read off
// globals, which is the whole reason the parser and the sanitizer policy can
// be exercised under Node with no app bootstrap.
//
// The sanitizer runs LAST and unconditionally. Everything upstream — parser
// output, highlighted fences, raw HTML the author embedded — is treated as
// untrusted; a .md file is content the user merely opened, not code they
// chose to enable.
(function initTermLabMarkdownRenderer(global) {
  'use strict';

  // Tags real READMEs use, and nothing that can execute or navigate.
  // <script>, <style>, <iframe>, <object>, <embed>, <form> and every event
  // handler are absent deliberately, not by oversight.
  const ALLOWED_TAGS = [
    'p', 'br', 'hr', 'blockquote', 'pre', 'code', 'span', 'div',
    'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'ul', 'ol', 'li', 'dl', 'dt', 'dd',
    'table', 'thead', 'tbody', 'tr', 'th', 'td',
    'a', 'img', 'em', 'strong', 's', 'del', 'ins', 'sup', 'sub', 'kbd', 'mark',
    'details', 'summary', 'input',
    // markdown-it-footnote wraps the footnote list in <section class="footnotes">
    // with an <hr class="footnotes-sep">. Without `section` here the whole
    // footnote block is silently stripped and footnotes render as dangling refs.
    'section',
  ];

  // No `style` (CSS can exfiltrate via url()), no `on*`, no `srcset`.
  // `data-src-line` is explicit because ALLOW_DATA_ATTR is off.
  const ALLOWED_ATTR = [
    'href', 'title', 'alt', 'src', 'class', 'align', 'colspan', 'rowspan',
    'type', 'checked', 'disabled', 'open', 'id', 'data-src-line', 'data-img-ref',
  ];

  const SAFE_LINK = /^(https?:|mailto:|#|\/|\.\/|\.\.\/)/i;

  function createRenderer(deps) {
    const options = deps || {};
    const MarkdownIt = options.MarkdownIt;
    const DOMPurify = options.DOMPurify;
    const taskListsPlugin = options.taskListsPlugin || null;
    const footnotePlugin = options.footnotePlugin || null;
    const highlight = typeof options.highlight === 'function' ? options.highlight : null;
    if (!MarkdownIt || !DOMPurify) return null;

    const md = new MarkdownIt({
      html: true,        // raw HTML is PARSED, then sanitized below
      linkify: true,
      breaks: false,
      highlight(code, lang) {
        if (!highlight) return '';   // '' => markdown-it escapes it itself
        return highlight(code, lang) || '';
      },
    });

    // Task lists and footnotes are plugins, not core markdown-it. Left off,
    // "- [x] done" renders as the literal text "[x] done".
    //
    // markdown-it-task-lists is used at its DEFAULT, which emits `disabled`
    // checkboxes. Passing { enabled: true } would make them clickable, and a
    // preview is a view of the file, not an editor for it.
    if (taskListsPlugin) md.use(taskListsPlugin);
    if (footnotePlugin) md.use(footnotePlugin);

    // Attach source lines to top-level blocks. markdown-it gives every block
    // token a `.map` of [startLine, endLine]; level 0 keeps this to the
    // outermost blocks so scroll sync has one anchor per visual chunk rather
    // than one per nested list item.
    md.core.ruler.push('termlab_source_lines', (state) => {
      for (const token of state.tokens) {
        if (token.level === 0 && token.map && token.nesting !== -1) {
          token.attrSet('data-src-line', String(token.map[0]));
        }
      }
      return true;
    });

    // Drop remote images at SANITIZE time, not render time. Doing it here
    // means the element never reaches a document, so no request is ever
    // issued — hiding it after the fact would already have leaked.
    DOMPurify.addHook('afterSanitizeAttributes', (node) => {
      if (node.tagName === 'IMG') {
        const src = node.getAttribute('src') || '';
        if (/^https?:/i.test(src)) {
          node.removeAttribute('src');
          node.setAttribute('alt', `[remote image blocked] ${node.getAttribute('alt') || ''}`.trim());
          node.setAttribute('class', 'md-img-blocked');
        }
      }
      if (node.tagName === 'A') {
        const href = node.getAttribute('href') || '';
        if (href && !SAFE_LINK.test(href)) node.removeAttribute('href');
      }
      // Checkboxes come from task lists and must stay inert.
      if (node.tagName === 'INPUT') node.setAttribute('disabled', 'disabled');
    });

    function escapeText(text) {
      return String(text)
        .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }

    function render(markdown) {
      const source = typeof markdown === 'string' ? markdown : '';
      let raw;
      try {
        raw = md.render(source);
      } catch (err) {
        // A parser failure is reported IN the preview rather than as a toast:
        // the preview is the thing that failed, and a toast would fire again
        // on every debounced re-render while the document stays broken.
        return `<div class="md-render-error"><strong>Preview failed to render.</strong>`
          + `<pre>${escapeText(err && err.message ? err.message : err)}</pre></div>`;
      }
      return DOMPurify.sanitize(raw, {
        ALLOWED_TAGS,
        ALLOWED_ATTR,
        ALLOW_DATA_ATTR: false,
        FORBID_TAGS: ['script', 'style', 'iframe', 'object', 'embed', 'form', 'base', 'link'],
        FORBID_ATTR: ['style', 'srcset', 'formaction', 'ping'],
      });
    }

    return { render };
  }

  global.termlabMarkdownRenderer = { createRenderer };
})(window);
