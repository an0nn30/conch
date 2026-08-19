// The CodeMirror theme app/features/editor/theme.js built under the DARK
// appearance at commit b043fbf — the last commit BEFORE the fix round that
// taught theme.js to branch on appearance (F2).
//
// This file is a PIN, not a convenience: `feat/termlab-light` is merged on
// the promise that a dark install sees no change whatsoever, and the editor
// theme is the one place in that branch where a light-appearance code path
// sits inside a function the dark path also runs through. Regenerating this
// snapshot to make a test pass would retire the only automated statement of
// that promise, so treat a mismatch as a defect in theme.js until proven
// otherwise.
//
// Captured by test_appearance.mjs:captureEditorTheme() with DARK_VARS and
// no __termlabTermFontFamily set: the serialized {styles, opts, specs}
// triple handed to CM.EditorView.theme / CM.HighlightStyle.define, with tag
// functions replaced by their names so the structure serializes.

export const EDITOR_THEME_DARK_AT_BASE = String.raw`{
  "styles": {
    "&": {
      "backgroundColor": "rgb(7, 10, 14)",
      "color": "rgb(191, 198, 206)",
      "height": "100%"
    },
    ".cm-content": {
      "caretColor": "rgb(191, 198, 206)"
    },
    ".cm-cursor, .cm-dropCursor": {
      "borderLeftColor": "rgb(191, 198, 206)"
    },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
      "backgroundColor": "rgb(33, 66, 131)"
    },
    ".cm-gutters": {
      "backgroundColor": "rgb(7, 10, 14)",
      "color": "rgb(128, 134, 142)",
      "borderRight": "1px solid rgb(45, 49, 55)"
    },
    ".cm-activeLine": {
      "backgroundColor": "rgb(43, 45, 48)"
    },
    ".cm-activeLineGutter": {
      "backgroundColor": "rgb(43, 45, 48)",
      "color": "rgb(191, 198, 206)"
    },
    ".cm-selectionMatch": {
      "backgroundColor": "rgb(43, 45, 48)"
    },
    ".cm-scroller": {
      "fontFamily": "\"JetBrains Mono\", \"Fira Code\", \"Cascadia Code\", \"Symbols Nerd Font Mono\", \"Symbols Nerd Font\", \"Menlo\", \"DejaVu Sans Mono\", \"Consolas\", \"Liberation Mono\", monospace"
    },
    ".cm-panels": {
      "backgroundColor": "rgb(7, 10, 14)",
      "color": "rgb(191, 198, 206)",
      "fontFamily": "\"JetBrains Mono\", \"Fira Code\", \"Cascadia Code\", \"Symbols Nerd Font Mono\", \"Symbols Nerd Font\", \"Menlo\", \"DejaVu Sans Mono\", \"Consolas\", \"Liberation Mono\", monospace"
    },
    ".cm-panels.cm-panels-bottom": {
      "borderTop": "1px solid rgb(45, 49, 55)"
    },
    ".cm-panels.cm-panels-top": {
      "borderBottom": "1px solid rgb(45, 49, 55)"
    },
    ".cm-vim-panel": {
      "fontFamily": "\"JetBrains Mono\", \"Fira Code\", \"Cascadia Code\", \"Symbols Nerd Font Mono\", \"Symbols Nerd Font\", \"Menlo\", \"DejaVu Sans Mono\", \"Consolas\", \"Liberation Mono\", monospace"
    },
    ".cm-vim-panel input": {
      "color": "rgb(191, 198, 206)",
      "fontFamily": "\"JetBrains Mono\", \"Fira Code\", \"Cascadia Code\", \"Symbols Nerd Font Mono\", \"Symbols Nerd Font\", \"Menlo\", \"DejaVu Sans Mono\", \"Consolas\", \"Liberation Mono\", monospace"
    }
  },
  "opts": {
    "dark": true
  },
  "specs": [
    {
      "tag": [
        "keyword",
        "controlKeyword",
        "moduleKeyword"
      ],
      "color": "rgb(53, 116, 240)"
    },
    {
      "tag": [
        "string",
        {
          "tag": "special",
          "arg": "string"
        }
      ],
      "color": "rgb(152, 195, 121)"
    },
    {
      "tag": [
        "comment",
        "lineComment",
        "blockComment"
      ],
      "color": "rgb(128, 134, 142)",
      "fontStyle": "italic"
    },
    {
      "tag": [
        "number",
        "bool",
        "null"
      ],
      "color": "rgb(229, 192, 123)"
    },
    {
      "tag": [
        {
          "tag": "function",
          "arg": "variableName"
        },
        {
          "tag": "definition",
          "arg": "variableName"
        }
      ],
      "color": "rgb(97, 175, 239)"
    },
    {
      "tag": [
        "typeName",
        "className"
      ],
      "color": "rgb(86, 182, 194)"
    },
    {
      "tag": "propertyName",
      "color": "rgb(191, 198, 206)"
    },
    {
      "tag": "operator",
      "color": "rgb(128, 134, 142)"
    },
    {
      "tag": "invalid",
      "color": "rgb(199, 84, 80)"
    },
    {
      "tag": [
        "heading",
        "strong"
      ],
      "color": "rgb(191, 198, 206)",
      "fontWeight": "bold"
    },
    {
      "tag": "emphasis",
      "fontStyle": "italic"
    },
    {
      "tag": "link",
      "color": "rgb(53, 116, 240)",
      "textDecoration": "underline"
    }
  ]
}`;
