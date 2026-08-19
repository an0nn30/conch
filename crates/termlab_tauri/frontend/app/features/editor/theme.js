// A CodeMirror theme built from the app's design tokens.
//
// Reads the same --tl-* variables every other component uses, so the editor
// follows the active theme and light/dark without a second palette to keep in
// sync. No literal colours live here: an unset token yields an empty string
// and CodeMirror falls back to its own default rather than to a wrong
// hardcode.
(function initTermLabEditorTheme(global) {
  'use strict';

  function token(name, fallbackToken) {
    const styles = getComputedStyle(document.documentElement);
    const value = styles.getPropertyValue(name).trim();
    if (value) return value;
    return fallbackToken ? styles.getPropertyValue(fallbackToken).trim() : '';
  }

  // Whether the app is running under the LIGHT appearance, asked of the one
  // owner of that answer (app/core/appearance.js). Not inferred from a token:
  // the two colour sources this branches between (app tokens vs the terminal
  // palette) can disagree, which is exactly the case being handled.
  function isLightAppearance() {
    const appearance = global.termlabAppearance;
    return !!(appearance
      && typeof appearance.current === 'function'
      && appearance.current() === 'light');
  }

  function buildTheme() {
    const CM = global.CM6;
    if (!CM) return [];

    // Under LIGHT the editor is a document surface and follows the app
    // appearance: every colour comes from the app's own --tl-* tokens.
    //
    // Under DARK nothing changes. The editor keeps matching the terminal
    // beside it: the background prefers --tl-terminal-bg and the syntax
    // accents come from the terminal's ANSI vars.
    //
    // The two sources have to be branched rather than merged because
    // config-service.js:16-27 writes --tl-terminal-bg and --red/--green/...
    // as INLINE root styles from the terminal colour scheme, which the app
    // deliberately keeps dark under both appearances. Preferring them under
    // Light paints a light-appearance editor (light selection bands, light
    // gutter rules, #1F2933 text) onto a near-black background.
    const light = isLightAppearance();

    const bg = light ? token('--tl-bg') : token('--tl-terminal-bg', '--tl-bg');
    const fg = token('--tl-fg');
    const muted = token('--tl-fg-muted');
    const accent = token('--tl-accent');
    const border = token('--tl-border');
    const selection = token('--tl-selection-bg', '--tl-accent');
    const rowHover = token('--tl-row-hover');

    // Syntax accents. Under Dark: the terminal's ANSI var, falling back to an
    // app token when the palette does not define it — verbatim what this file
    // has always done. Under Light: the app token directly, because the ANSI
    // vars are terminal-owned and stay tuned for a dark canvas.
    const syntax = light
      ? (ansiVar, appToken) => token(appToken)
      : (ansiVar, appToken) => token(ansiVar, appToken);

    // The terminal's stack, not the UI font — an editor beside a terminal
    // shares its typeface. main-runtime keeps this global current; the literal
    // fallback is only for the pre-init window and must stay in step with
    // FONT_FALLBACKS in main-runtime.js:58.
    const fontStack = global.__termlabTermFontFamily
      || '"JetBrains Mono", "Fira Code", "Cascadia Code", "Symbols Nerd Font Mono", "Symbols Nerd Font", "Menlo", "DejaVu Sans Mono", "Consolas", "Liberation Mono", monospace';

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
      '.cm-scroller': { fontFamily: fontStack },
      // Panels: vim's `:` command line and the search bar. Both would
      // otherwise take CodeMirror's own grey panel default and read as a
      // foreign strip pasted under the editor. The vim package's base theme
      // hardcodes `monospace` on its panel, which is why the font is restated
      // here — an EditorView.theme rule outranks a baseTheme one.
      '.cm-panels': { backgroundColor: bg, color: fg, fontFamily: fontStack },
      '.cm-panels.cm-panels-bottom': { borderTop: `1px solid ${border}` },
      '.cm-panels.cm-panels-top': { borderBottom: `1px solid ${border}` },
      '.cm-vim-panel': { fontFamily: fontStack },
      '.cm-vim-panel input': { color: fg, fontFamily: fontStack },
      // Under Light the flag is asserted rather than inferred: the app
      // appearance IS the answer, and bg above now comes from --tl-bg, the
      // very token isDarkTheme() measures.
    }, { dark: light ? false : isDarkTheme() });

    const t = CM.tags;
    const highlight = CM.HighlightStyle.define([
      { tag: [t.keyword, t.controlKeyword, t.moduleKeyword], color: accent },
      { tag: [t.string, t.special(t.string)], color: syntax('--green', '--tl-accent') },
      { tag: [t.comment, t.lineComment, t.blockComment], color: muted, fontStyle: 'italic' },
      { tag: [t.number, t.bool, t.null], color: syntax('--yellow', '--tl-accent') },
      { tag: [t.function(t.variableName), t.definition(t.variableName)], color: syntax('--blue', '--tl-fg') },
      { tag: [t.typeName, t.className], color: syntax('--cyan', '--tl-accent') },
      { tag: t.propertyName, color: fg },
      { tag: t.operator, color: muted },
      { tag: t.invalid, color: token('--tl-danger') },
      { tag: [t.heading, t.strong], color: fg, fontWeight: 'bold' },
      { tag: t.emphasis, fontStyle: 'italic' },
      { tag: t.link, color: accent, textDecoration: 'underline' },
    ]);

    return [theme, CM.syntaxHighlighting(highlight)];
  }

  // Whether the active theme is dark, inferred from the perceived lightness of
  // --tl-bg. There is no declared light/dark flag to read: the token pipeline
  // emits colour values only, and nothing in the app sets a theme attribute or
  // class on the document element. So the background colour itself is the
  // signal, and inference is the mechanism rather than a fallback for one.
  //
  // Unresolvable (--tl-bg unset, or not an rgb() value) is treated as dark,
  // which is the default theme.
  function isDarkTheme() {
    const bg = token('--tl-bg');
    const m = /rgb[a]?\((\d+),\s*(\d+),\s*(\d+)/.exec(bg);
    if (!m) return true;
    const luma = (0.299 * +m[1] + 0.587 * +m[2] + 0.114 * +m[3]) / 255;
    return luma < 0.5;
  }

  global.termlabEditorTheme = { buildTheme };
})(window);
