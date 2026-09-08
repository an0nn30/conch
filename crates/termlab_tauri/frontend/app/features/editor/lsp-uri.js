// `file:` URI <-> local path, shared by everything that renders a diagnostic.
//
// Rust speaks URIs because LSP does; every frontend surface — the editor's
// squiggles, the Problems list, the open/focus flow — speaks canonical paths,
// because that is what `editor_reserve_document` and the pane registry are
// keyed by. One conversion, in one place: two copies of this that disagree
// about percent-encoding would silently route a file's diagnostics to nobody.
//
// Deliberately NOT a general URL parser. Anything that is not a `file:` URI
// comes back unchanged so a caller comparing it against a path simply finds
// no match, rather than throwing on a malformed server response.
(function initTermLabLspUri(global) {
  'use strict';

  const FILE_SCHEME = 'file://';

  // Whether a server-supplied URI names a local file at all. Go to Definition
  // needs the question answered before it converts anything: a `jdt://` or
  // `untitled:` target must be reported as unsupported, and `uriToPath` — which
  // deliberately returns unknown schemes unchanged — cannot say so on its own.
  // The scheme test lives here with the rest of the scheme knowledge.
  function isFileUri(uri) {
    const text = String(uri === null || uri === undefined ? '' : uri);
    return text.slice(0, FILE_SCHEME.length) === FILE_SCHEME;
  }

  function uriToPath(uri) {
    const text = String(uri === null || uri === undefined ? '' : uri);
    if (text.slice(0, FILE_SCHEME.length) !== FILE_SCHEME) return text;
    let rest = text.slice(FILE_SCHEME.length);
    // `file://host/path` — drop the authority; `file:///path` has none, and
    // its leading slash is at index 0, so this leaves it alone.
    const slash = rest.indexOf('/');
    if (slash > 0) rest = rest.slice(slash);
    else if (slash < 0) rest = `/${rest}`;
    try {
      return decodeURIComponent(rest);
    } catch (_) {
      // A malformed escape (a lone `%`) is not worth losing the path over.
      return rest;
    }
  }

  // Only the characters that would change the meaning of the URI are escaped;
  // `/` stays a separator. encodeURI is the closest built-in and leaves `#`
  // and `?` alone, which in a path are ordinary characters to the filesystem
  // and delimiters to a URI parser — hence the two explicit replacements.
  function pathToUri(filePath) {
    const text = String(filePath === null || filePath === undefined ? '' : filePath);
    if (!text) return '';
    if (text.slice(0, FILE_SCHEME.length) === FILE_SCHEME) return text;
    const withRoot = text.charAt(0) === '/' ? text : `/${text}`;
    const encoded = encodeURI(withRoot).replace(/#/g, '%23').replace(/\?/g, '%3F');
    return `${FILE_SCHEME}${encoded}`;
  }

  global.termlabLspUri = { uriToPath, pathToUri, isFileUri };
})(window);
