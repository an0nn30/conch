/**
 * Sanitizer for plugin-supplied HTML (the `html` widget).
 *
 * Plugin markup is untrusted. A plugin holding only `ui.panel` must not be
 * able to reach the host JS realm, because that realm owns the Tauri `invoke`
 * bridge and every capability behind it. Shadow DOM provides style
 * encapsulation, not script isolation, so markup is parsed into an inert
 * document and rebuilt node by node against an allowlist before it touches
 * the live DOM.
 *
 * Rebuilding rather than scrubbing in place is deliberate: only text and
 * vetted attributes cross over, and nothing is re-serialized, which is what
 * makes mutation-XSS round-trips possible in string-based sanitizers.
 */
(function () {
  'use strict';

  /** Elements that may appear in plugin markup. Everything else is dropped. */
  const ALLOWED_TAGS = new Set([
    'div', 'span', 'p', 'br', 'hr', 'a', 'b', 'strong', 'i', 'em', 'u', 's',
    'small', 'sub', 'sup', 'code', 'pre', 'kbd', 'samp', 'mark', 'del', 'ins',
    'abbr', 'time', 'wbr',
    'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'ul', 'ol', 'li', 'dl', 'dt', 'dd', 'blockquote',
    'table', 'caption', 'thead', 'tbody', 'tfoot', 'tr', 'th', 'td',
    'img', 'figure', 'figcaption',
    'section', 'article', 'header', 'footer', 'aside', 'nav', 'label',
    // Buttons carry no behaviour of their own here — plugin clicks are routed
    // by delegated data-action listeners — but panels are built out of them.
    'button',
  ]);

  const SVG_NS = 'http://www.w3.org/2000/svg';

  /**
   * SVG elements permitted inline. Deliberately excludes `script`,
   * `foreignObject`, `use` and the animation elements, which can execute
   * script or pull in external references.
   */
  const SVG_TAGS = new Set([
    'svg', 'g', 'path', 'circle', 'ellipse', 'rect', 'line', 'polyline',
    'polygon', 'title', 'desc',
  ]);

  /** Presentation attributes permitted on SVG elements. No URL-bearing ones. */
  const SVG_ATTRS = new Set([
    'viewbox', 'xmlns', 'width', 'height', 'd', 'fill', 'fill-rule',
    'fill-opacity', 'clip-rule', 'stroke', 'stroke-width', 'stroke-linecap',
    'stroke-linejoin', 'stroke-dasharray', 'stroke-opacity', 'opacity',
    'cx', 'cy', 'r', 'rx', 'ry', 'x', 'y', 'x1', 'y1', 'x2', 'y2', 'points',
    'transform', 'preserveaspectratio', 'vector-effect', 'class', 'style',
  ]);

  /** Attributes allowed on any permitted element. */
  const GLOBAL_ATTRS = new Set(['class', 'id', 'title', 'style', 'dir', 'lang', 'role']);

  /** Additional attributes allowed per element. */
  const TAG_ATTRS = {
    a: new Set(['href', 'target', 'rel']),
    img: new Set(['src', 'alt', 'width', 'height', 'loading']),
    td: new Set(['colspan', 'rowspan', 'headers']),
    th: new Set(['colspan', 'rowspan', 'headers', 'scope']),
    ol: new Set(['start', 'type', 'reversed']),
    li: new Set(['value']),
    time: new Set(['datetime']),
  };

  /** Image data URIs that cannot carry script. `svg+xml` is excluded. */
  const DATA_IMAGE = /^data:image\/(?:png|jpeg|jpg|gif|webp);base64,[a-z0-9+/=]+$/;

  /**
   * Decide whether a URL-bearing attribute value is safe to keep.
   *
   * Relative URLs are allowed. Absolute URLs must use a known-inert scheme —
   * `javascript:`, `vbscript:`, and general `data:` are rejected.
   */
  function isSafeUrl(raw, allowDataImage) {
    if (raw == null) return false;
    // Browsers ignore control characters and whitespace when resolving a
    // scheme, so "java\nscript:" is "javascript:". Strip them before testing.
    const v = String(raw).replace(/[\u0000-\u0020]/g, '').toLowerCase();
    if (v === '') return false;
    const scheme = v.match(/^([a-z0-9+.-]+):/);
    if (!scheme) return true; // relative
    const s = scheme[1];
    if (s === 'http' || s === 'https' || s === 'mailto') return true;
    if (allowDataImage && DATA_IMAGE.test(v)) return true;
    return false;
  }

  /** Copy allowed attributes from `src` onto `dest`. */
  function copyAttributes(src, dest, tag, isSvg) {
    const extra = TAG_ATTRS[tag];
    for (const attribute of Array.from(src.attributes)) {
      const name = attribute.name.toLowerCase();
      const value = attribute.value;

      // Event handlers are never allowed, under any element.
      if (name.startsWith('on')) continue;

      const allowed =
        name.startsWith('data-') ||
        name.startsWith('aria-') ||
        (isSvg
          ? SVG_ATTRS.has(name)
          : GLOBAL_ATTRS.has(name) || (extra != null && extra.has(name)));
      if (!allowed) continue;

      if ((name === 'href' || name === 'src') && !isSafeUrl(value, tag === 'img')) {
        continue;
      }

      // Set under the original name: SVG attributes such as `viewBox` are
      // case-sensitive and are ignored if written lowercase.
      dest.setAttribute(attribute.name, value);
      // A link that opens elsewhere must not hand the opener to the target.
      if (name === 'target') dest.setAttribute('rel', 'noopener noreferrer');
    }
  }

  /**
   * Rebuild one node into `doc`, or return null if it is not allowed.
   *
   * Disallowed elements are dropped along with their subtree; comments and
   * processing instructions are dropped outright.
   */
  function cleanNode(node, doc) {
    if (node.nodeType === 3) return doc.createTextNode(node.nodeValue);
    if (node.nodeType !== 1) return null;

    // Namespace comes from the parsed node rather than a traversal flag, so a
    // nested subtree can never be misclassified.
    const isSvg = node.namespaceURI === SVG_NS;
    const tag = node.localName.toLowerCase();
    if (isSvg ? !SVG_TAGS.has(tag) : !ALLOWED_TAGS.has(tag)) return null;

    const el = isSvg
      ? doc.createElementNS(SVG_NS, node.localName)
      : doc.createElement(tag);
    copyAttributes(node, el, tag, isSvg);
    for (const child of Array.from(node.childNodes)) {
      const clean = cleanNode(child, doc);
      if (clean) el.appendChild(clean);
    }
    return el;
  }

  /**
   * Parse untrusted HTML and return a sanitized DocumentFragment.
   *
   * @param {string} html Untrusted markup from a plugin.
   * @param {Document} [doc] Document that owns the resulting nodes.
   * @returns {DocumentFragment} Safe to append to the live DOM.
   */
  function sanitizeToFragment(html, doc) {
    const targetDoc = doc || document;
    const frag = targetDoc.createDocumentFragment();
    if (html == null || html === '') return frag;

    // DOMParser produces an inert document: no scripts run, no resources load.
    const parsed = new DOMParser().parseFromString(String(html), 'text/html');
    for (const child of Array.from(parsed.body.childNodes)) {
      const clean = cleanNode(child, targetDoc);
      if (clean) frag.appendChild(clean);
    }
    return frag;
  }

  window.htmlSanitizer = { sanitizeToFragment, isSafeUrl };
})();
