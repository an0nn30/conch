// Syntax highlighting for markdown code fences, using the same CodeMirror
// grammars the editor uses. A fence gets the editor's tokens without an
// EditorView being constructed per fence: lezer's highlightCode walks a parsed
// tree and hands back (text, classes) pairs, which become spans here.
//
// Colours are NOT resolved here. classHighlighter yields stable `tok-*` class
// names and preview-frame.js supplies the palette, so the same markup restyles
// on theme change instead of being rebuilt.
(function initTermLabFenceHighlight(global) {
  'use strict';

  function escapeHtml(text) {
    return String(text)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  // A fence language is resolved through the SAME table the editor uses, so a
  // ```rust fence and a .rs file can never disagree about their grammar.
  // languageKeyFor takes a filename, so the fence tag is turned into one.
  function grammarFor(CM, languageMap, lang) {
    if (!CM || !languageMap || !lang) return null;
    const key = languageMap.languageKeyFor(`x.${String(lang).toLowerCase()}`);
    if (!key) return null;
    const entry = CM[key];
    if (!entry) return null;
    // Same two shapes editor-pane.js discriminates between: lang-* packages
    // export a FUNCTION returning a LanguageSupport; legacy modes export a
    // plain StreamParser OBJECT that must be wrapped before it has a parser.
    if (typeof entry === 'function') {
      const support = entry();
      return support && support.language ? support.language.parser : null;
    }
    return CM.StreamLanguage.define(entry).parser;
  }

  function createHighlighter(deps) {
    const options = deps || {};
    const CM = options.CM;
    const highlightCode = options.highlightCode;
    const classHighlighter = options.classHighlighter;
    const languageMap = options.languageMap || global.termlabEditorLanguageMap;
    if (!CM || typeof highlightCode !== 'function') return () => null;

    return function highlight(code, lang) {
      const parser = grammarFor(CM, languageMap, lang);
      if (!parser) return null;

      let tree;
      try {
        tree = parser.parse(String(code));
      } catch (err) {
        // A grammar that throws on malformed input must degrade to plain
        // text, never take the whole preview down with it.
        return null;
      }

      let out = '';
      highlightCode(
        String(code),
        tree,
        classHighlighter,
        (text, classes) => {
          out += classes
            ? `<span class="${escapeHtml(classes)}">${escapeHtml(text)}</span>`
            : escapeHtml(text);
        },
        () => { out += '\n'; },
      );

      const langAttr = escapeHtml(String(lang).toLowerCase());
      return `<pre class="md-code" data-lang="${langAttr}"><code>${out}</code></pre>`;
    };
  }

  global.termlabFenceHighlight = { createHighlighter };
})(window);
