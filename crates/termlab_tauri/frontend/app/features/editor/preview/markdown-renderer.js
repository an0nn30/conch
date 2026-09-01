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

  // A scheme needs at least two characters before its colon, so a Windows
  // drive root (`C:\pics\x.png`) is not read as one. Same rule, for the same
  // reason, as REMOTE_SCHEME in image-resolver.js.
  const URL_SCHEME = /^[a-z][a-z0-9+.-]+:/i;

  // Elements that fetch a subresource from `src`. `input` is here because
  // `input`, `type` and `src` are all allowlisted for task-list checkboxes,
  // and `input[type=image]` issues a request in every engine.
  const FETCHING_TAGS = { IMG: true, INPUT: true };

  // The URL parser DELETES ASCII tab, CR and LF while resolving, so
  // `ht<TAB>tps://evil/x.png` — and its `&Tab;` entity spelling, and a literal
  // newline — resolve and load exactly as the plain form would. Classification
  // therefore runs on a value with all whitespace and control characters
  // removed, never on the raw attribute text.
  function flattenUrl(value) {
    return String(value === null || value === undefined ? '' : value)
      .replace(/[\s\u0000-\u001f\u007f]+/g, '');
  }

  // `//host/x.png` is protocol-relative. On Windows and Android the frontend
  // is served over http from tauri.localhost, so there it is a real outbound
  // request rather than a dead reference — and TermLab ships those targets.
  function isRemoteRef(flat) {
    return flat.startsWith('//') || URL_SCHEME.test(flat);
  }

  // Nothing that can issue a subresource request keeps its `src`: the value is
  // MOVED to data-img-ref, which only preview-controller.js reads, and it
  // writes back a `data:` URI it fetched through Rust — never a URL. The frame
  // therefore makes zero network requests by construction.
  //
  // This is deliberately a rule about SHAPE rather than a blocklist of bad
  // URLs. Every previous spelling of "remote" (tab-split scheme, entity tab,
  // protocol-relative, a tag the check did not look at) got past a pattern
  // that tested for http(s); removing `src` unconditionally cannot be spelled
  // around. Nothing else protects this: the frame's sandbox blocks execution,
  // not subresource loads, and `csp` is null app-wide.
  function neutraliseFetchingSource(node) {
    // Authors do not get to hand the resolver a path directly — the attribute
    // is written here or not at all.
    node.removeAttribute('data-img-ref');
    if (!node.hasAttribute('src')) return;
    const raw = node.getAttribute('src') || '';
    const flat = flattenUrl(raw);
    // Already inline: no request is possible and the resolver has nothing
    // to fetch, so leave it exactly as the author wrote it.
    if (/^data:/i.test(flat)) return;

    node.removeAttribute('src');
    if (isRemoteRef(flat)) {
      node.setAttribute('alt', `[remote image blocked] ${node.getAttribute('alt') || ''}`.trim());
      node.setAttribute('class', 'md-img-blocked');
      return;
    }
    node.setAttribute('data-img-ref', raw);
  }

  // Runs at SANITIZE time, not render time, so a neutralised element never
  // reaches a document — hiding it after the fact would already have leaked.
  function afterSanitizeAttributes(node) {
    if (FETCHING_TAGS[node.tagName]) neutraliseFetchingSource(node);
    if (node.tagName === 'A') {
      const href = node.getAttribute('href') || '';
      if (href && !SAFE_LINK.test(href)) node.removeAttribute('href');
    }
    // Checkboxes come from task lists and must stay inert.
    if (node.tagName === 'INPUT') node.setAttribute('disabled', 'disabled');
  }

  // DOMPurify is a module SINGLETON — `MDLib.DOMPurify` is one object shared by
  // every pane — and addHook APPENDS rather than replaces. createRenderer runs
  // once per preview controller, so hooking there stacked one identical hook
  // per open markdown file, every one of them running on every node of every
  // sanitize, for the life of the process. Hooks are per-instance state, so
  // each distinct instance is hooked exactly once. (Tests build their own
  // instance per suite, which is why this is keyed on the instance rather
  // than being a single module-level boolean.)
  const hookedInstances = new WeakSet();

  function ensureHooks(DOMPurify) {
    if (hookedInstances.has(DOMPurify)) return;
    hookedInstances.add(DOMPurify);
    DOMPurify.addHook('afterSanitizeAttributes', afterSanitizeAttributes);
  }

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

    // markdown-it's fence renderer SHORT-CIRCUITS: when the highlight callback
    // returns a string that already starts with `<pre`, it hands that string
    // back verbatim and never renders the token's own attributes. A highlighted
    // fence therefore lost data-src-line, leaving scroll sync with no anchor
    // for the entire height of a code block — the one construct this feature
    // exists to display well. Re-applying the attribute here (rather than
    // threading the line into the highlighter) is what keeps fence-highlight.js
    // ignorant of markdown-it internals.
    //
    // Guarded rather than assumed: createRenderer is called with stub parsers
    // in tests, and a missing renderer must degrade, not throw.
    const rules = md.renderer && md.renderer.rules ? md.renderer.rules : null;
    const baseFence = rules ? rules.fence : null;
    if (rules && typeof baseFence === 'function') {
      rules.fence = function fenceWithSourceLine(tokens, idx, opts, env, self) {
        const html = baseFence(tokens, idx, opts, env, self);
        const token = tokens[idx];
        const line = token && typeof token.attrGet === 'function'
          ? token.attrGet('data-src-line')
          : null;
        // Only a plain integer is ever written by the rule above, so anything
        // else means the token was not ours to annotate.
        if (!/^\d+$/.test(String(line)) || !/^<pre[\s>]/.test(html)) return html;
        if (/\sdata-src-line=/.test(html)) return html;
        return html.replace(/^<pre/, `<pre data-src-line="${line}"`);
      };
    }

    ensureHooks(DOMPurify);

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
