// Server Markdown -> safe segments -> text nodes.
//
// Extracted from lsp-tooltips.js, which is an overlay state machine and had no
// business also being a Markdown normalizer. Pure apart from the DOM nodes it
// builds: no requests, no CodeMirror, no state.
//
// Server text is NEVER injected as markup. A block is normalized to `text` and
// `code` segments and inserted with textContent, which is the same rule
// lsp-completion.js and lsp-diagnostics.js follow, for the same reason: a
// server that sends `<script>` must produce the literal characters `<script>`
// on screen and no element at all.
(function initTermLabLspMarkdown(global) {
  'use strict';

  // A markdown heading marker, and nothing cleverer. Lookbehind is banned
  // repo-wide: a regex literal is validated when the FILE is parsed, so one
  // would stop this module loading at all on an older WKWebView.
  const HEADING_PREFIX = /^\s{0,3}#{1,6}\s+/;

  function doc() {
    return global.document;
  }

  // Not a renderer: a normalizer. Fenced blocks become `code` segments kept
  // verbatim, everything else becomes `text` segments with the handful of
  // inline markers that would otherwise read as noise removed. Nothing here
  // ever produces markup, so a server that sends `<script>` gets a segment
  // whose value is the literal characters `<script>`.

  function isFence(line) {
    return line.trim().indexOf('```') === 0;
  }

  // `__` is deliberately left alone: stripping it would mangle Python dunders,
  // which appear in hover text far more often than underscore emphasis does.
  function plainLine(line) {
    return line.replace(HEADING_PREFIX, '').split('`').join('').split('**').join('');
  }

  function pushText(out, value) {
    const text = String(value).trim();
    if (text) out.push({ type: 'text', value: text });
  }

  function pushCode(out, value) {
    const text = String(value).replace(/\s+$/, '');
    if (text.trim()) out.push({ type: 'code', value: text });
  }

  function markdownSegments(blocks) {
    const out = [];
    for (const block of blocks || []) {
      if (!block || typeof block.value !== 'string') continue;
      if (block.markdown !== true) {
        pushText(out, block.value);
        continue;
      }
      let code = null;
      let text = [];
      for (const line of block.value.split('\n')) {
        if (isFence(line)) {
          if (code === null) {
            pushText(out, text.join('\n'));
            text = [];
            code = [];
          } else {
            pushCode(out, code.join('\n'));
            code = null;
          }
          continue;
        }
        if (code) code.push(line);
        else text.push(plainLine(line));
      }
      if (code) pushCode(out, code.join('\n'));
      pushText(out, text.join('\n'));
    }
    return out;
  }

  // --- rendering ---------------------------------------------------------------
  //
  // Every string goes in through textContent. Nothing here ever assigns
  // markup to an element.

  function block(className, text) {
    const node = doc().createElement('div');
    node.className = className;
    node.textContent = String(text);
    return node;
  }

  function appendSegments(root, segments, textClass, codeClass) {
    for (const segment of segments || []) {
      if (!segment) continue;
      if (segment.type === 'code') {
        const pre = doc().createElement('pre');
        pre.className = codeClass;
        pre.textContent = String(segment.value);
        root.appendChild(pre);
      } else {
        root.appendChild(block(textClass, segment.value));
      }
    }
  }

  global.termlabLspMarkdown = { markdownSegments, appendSegments, block };
})(window);
