// LSP position <-> CodeMirror offset, shared by everything that places a
// server-supplied range in an open buffer.
//
// The arithmetic is trivial — LSP positions and CodeMirror offsets both count
// UTF-16 code units, so this is addition — and the CLAMPING is not. A server
// that has raced ahead of the document can name a line past its end, and
// CodeMirror throws on an out-of-range line rather than saturating; a
// character past the end of a line has to stop at the line end, not spill into
// the next one. Seven copies of those rules, each subtly its own, is how a
// diagnostic ends up underlining the wrong word in one surface and the right
// one in another. One conversion, in one place, for the same reason `file:`
// URI conversion lives only in lsp-uri.js.
//
// Deliberately pure: no DOM, no CodeMirror import, no Tauri. It takes a
// CodeMirror `Text` (anything with `lines`, `line(n)`, `lineAt(offset)` and
// `length`) and plain LSP position objects, and it never throws on a malformed
// one — a missing or NaN coordinate reads as zero, which puts the caret at the
// start of the document rather than taking a surface down.
(function initTermLabLspPosition(global) {
  'use strict';

  function clamp(value, low, high) {
    return Math.min(Math.max(value, low), high);
  }

  function finiteOr(value, fallback) {
    const number = Number(value);
    return Number.isFinite(number) ? number : fallback;
  }

  // LSP position (zero-based line, UTF-16 character) -> document offset.
  // `text` is a CodeMirror `Text`, never a DOM node — this module touches no
  // DOM at all.
  function offsetAt(text, position) {
    if (!text || typeof text.line !== 'function') return 0;
    const line = finiteOr(position && position.line, 0);
    const character = finiteOr(position && position.character, 0);
    const entry = text.line(clamp(Math.floor(line) + 1, 1, text.lines));
    const column = character > 0 ? Math.floor(character) : 0;
    return Math.min(entry.from + column, entry.to);
  }

  // Document offset -> LSP position.
  function positionAt(text, offset) {
    if (!text || typeof text.lineAt !== 'function') return { line: 0, character: 0 };
    const at = clamp(finiteOr(offset, 0), 0, text.length);
    const line = text.lineAt(at);
    return { line: line.number - 1, character: at - line.from };
  }

  // LSP range -> a `{from, to}` document span. `to` never precedes `from`: a
  // reversed or partly stale range collapses rather than selecting backwards,
  // which is what every caller wants when it turns this into a selection.
  function spanOf(text, range) {
    const from = offsetAt(text, range && range.start);
    const to = offsetAt(text, (range && range.end) || (range && range.start));
    return { from, to: Math.max(to, from) };
  }

  // The text of a zero-based LSP line, clamped the same way. The definition
  // chooser previews a target line with it, and a server naming a line the
  // document no longer has must not be a thrown exception inside a render.
  function lineTextAt(text, line) {
    if (!text || typeof text.line !== 'function') return '';
    const index = clamp(Math.floor(finiteOr(line, 0)) + 1, 1, text.lines);
    const entry = text.line(index);
    return String((entry && entry.text) || '');
  }

  global.termlabLspPosition = {
    offsetAt, positionAt, spanOf, lineTextAt,
  };
})(window);
