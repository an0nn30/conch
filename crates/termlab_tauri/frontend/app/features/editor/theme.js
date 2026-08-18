// A CodeMirror theme built from the app's design tokens.
//
// Reads the same --tl-* variables every other component uses, so the editor
// follows skins and light/dark without a second palette to keep in sync. No
// literal colours live here: an unset token yields an empty string and
// CodeMirror falls back to its own default rather than to a wrong hardcode.
(function initTermLabEditorTheme(global) {
  'use strict';

  function token(name, fallbackToken) {
    const styles = getComputedStyle(document.documentElement);
    const value = styles.getPropertyValue(name).trim();
    if (value) return value;
    return fallbackToken ? styles.getPropertyValue(fallbackToken).trim() : '';
  }

  function buildTheme() {
    const CM = global.CM6;
    if (!CM) return [];

    const bg = token('--tl-terminal-bg', '--tl-bg');
    const fg = token('--tl-fg');
    const muted = token('--tl-fg-muted');
    const accent = token('--tl-accent');
    const border = token('--tl-border');
    const selection = token('--tl-selection-bg', '--tl-accent');
    const rowHover = token('--tl-row-hover');

    const theme = CM.EditorView.theme({
      '&': { backgroundColor: bg, color: fg, height: '100%' },
      '.cm-content': { caretColor: fg },
      '.cm-cursor, .cm-dropCursor': { borderLeftColor: fg },
      '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection':
        { backgroundColor: selection },
      '.cm-gutters': { backgroundColor: bg, color: muted, borderRight: `1px solid ${border}` },
      '.cm-activeLine': { backgroundColor: rowHover },
      '.cm-activeLineGutter': { backgroundColor: rowHover, color: fg },
      '.cm-selectionMatch': { backgroundColor: rowHover },
      '.cm-scroller': {
        // The terminal's stack, not the UI font — an editor beside a terminal
        // shares its typeface. main-runtime keeps this global current; the
        // literal fallback below is only for the pre-init window and must
        // stay in step with FONT_FALLBACKS in main-runtime.js:58-59.
        fontFamily: global.__termlabTermFontFamily
          || '"JetBrains Mono", "Fira Code", "Cascadia Code", "Symbols Nerd Font Mono", "Symbols Nerd Font", "Menlo", "DejaVu Sans Mono", "Consolas", "Liberation Mono", monospace',
      },
    }, { dark: isDarkTheme() });

    const t = CM.tags;
    const highlight = CM.HighlightStyle.define([
      { tag: [t.keyword, t.controlKeyword, t.moduleKeyword], color: accent },
      { tag: [t.string, t.special(t.string)], color: token('--green', '--tl-accent') },
      { tag: [t.comment, t.lineComment, t.blockComment], color: muted, fontStyle: 'italic' },
      { tag: [t.number, t.bool, t.null], color: token('--yellow', '--tl-accent') },
      { tag: [t.function(t.variableName), t.definition(t.variableName)], color: token('--blue', '--tl-fg') },
      { tag: [t.typeName, t.className], color: token('--cyan', '--tl-accent') },
      { tag: t.propertyName, color: fg },
      { tag: t.operator, color: muted },
      { tag: t.invalid, color: token('--tl-danger') },
      { tag: [t.heading, t.strong], color: fg, fontWeight: 'bold' },
      { tag: t.emphasis, fontStyle: 'italic' },
      { tag: t.link, color: accent, textDecoration: 'underline' },
    ]);

    return [theme, CM.syntaxHighlighting(highlight)];
  }

  // Whether the current skin is dark, inferred from the perceived lightness of
  // --tl-bg. There is no declared light/dark flag to read: the token pipeline
  // emits colour values only, and nothing in the app sets a theme attribute or
  // class on the document element. So the background colour itself is the
  // signal, and inference is the mechanism rather than a fallback for one.
  //
  // Unresolvable (--tl-bg unset, or not an rgb() value) is treated as dark,
  // which is the default skin.
  function isDarkTheme() {
    const bg = token('--tl-bg');
    const m = /rgb[a]?\((\d+),\s*(\d+),\s*(\d+)/.exec(bg);
    if (!m) return true;
    const luma = (0.299 * +m[1] + 0.587 * +m[2] + 0.114 * +m[3]) / 255;
    return luma < 0.5;
  }

  global.termlabEditorTheme = { buildTheme };
})(window);
