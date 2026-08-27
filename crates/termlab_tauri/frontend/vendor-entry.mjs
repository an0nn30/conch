// The ESM entry esbuild bundles into vendor/codemirror/codemirror.js as the
// IIFE global `CM6`. This file is the app's entire CodeMirror API surface:
// if a module needs something not re-exported here, add it here first.
export { EditorState, Compartment, Prec } from '@codemirror/state';
export {
  EditorView, keymap, lineNumbers, highlightActiveLine,
  highlightActiveLineGutter, drawSelection, rectangularSelection,
  highlightSpecialChars,
} from '@codemirror/view';
export { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
export {
  syntaxHighlighting, HighlightStyle, StreamLanguage,
  bracketMatching, indentOnInput, foldGutter, foldKeymap,
} from '@codemirror/language';
export { searchKeymap, highlightSelectionMatches } from '@codemirror/search';
export { tags } from '@lezer/highlight';

// LSP completion (features/editor/lsp-completion.js). Deliberately narrow:
// `autocompletion` is the extension, the commands are what the dedicated
// completion keymap binds (list navigation included — with `defaultKeymap`
// off, nothing else binds the arrows), `completionStatus` is how every one of
// those bindings decides whether to consume the key or hand it back to vim,
// and `snippet` is the applier a snippet item's expansion goes through.
//
// `completionKeymap` is NOT re-exported: autocompletion() is configured with
// `defaultKeymap: false` precisely so that one keymap owns these keys.
// @codemirror/lint is a declared dependency but exports nothing yet — the
// diagnostics task adds its names.
export {
  autocompletion, startCompletion, closeCompletion, acceptCompletion,
  moveCompletionSelection, completionStatus, snippet, snippetCompletion,
} from '@codemirror/autocomplete';

// Optional vim keybindings ([editor] vim_mode). `vim()` returns the extension,
// `Vim` is the engine object that owns defineEx (how :w/:q are bound to this
// app's own save and close paths). `getCM` (an EditorView -> CM5-style
// adapter) has no consumer here and is deliberately not re-exported.
export { vim, Vim } from '@replit/codemirror-vim';

export { javascript } from '@codemirror/lang-javascript';
export { json } from '@codemirror/lang-json';
export { python } from '@codemirror/lang-python';
export { markdown } from '@codemirror/lang-markdown';
export { rust } from '@codemirror/lang-rust';
export { html } from '@codemirror/lang-html';
export { css } from '@codemirror/lang-css';
export { xml } from '@codemirror/lang-xml';
export { yaml } from '@codemirror/lang-yaml';
export { sql } from '@codemirror/lang-sql';
export { java } from '@codemirror/lang-java';
export { cpp } from '@codemirror/lang-cpp';
export { go } from '@codemirror/lang-go';
export { php } from '@codemirror/lang-php';

export { shell } from '@codemirror/legacy-modes/mode/shell';
export { toml } from '@codemirror/legacy-modes/mode/toml';
export { dockerFile } from '@codemirror/legacy-modes/mode/dockerfile';
export { lua } from '@codemirror/legacy-modes/mode/lua';
export { ruby } from '@codemirror/legacy-modes/mode/ruby';
export { perl } from '@codemirror/legacy-modes/mode/perl';
export { powerShell } from '@codemirror/legacy-modes/mode/powershell';
export { nginx } from '@codemirror/legacy-modes/mode/nginx';
export { properties } from '@codemirror/legacy-modes/mode/properties';
export { diff } from '@codemirror/legacy-modes/mode/diff';
