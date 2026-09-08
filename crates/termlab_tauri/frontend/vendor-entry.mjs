// The ESM entry esbuild bundles into vendor/codemirror/codemirror.js as the
// IIFE global `CM6`. This file is the app's entire CodeMirror API surface:
// if a module needs something not re-exported here, add it here first.
// `Prec` raises the completion key handler above vim's view plugin;
// `ChangeSet` and `StateEffect` are what let a completion's additional edits
// be folded into the snippet's own transaction with CodeMirror's own position
// mapping rather than hand-rolled arithmetic (features/editor/
// lsp-completion-apply.js).
// `StateField` and `showTooltip` are the hover/signature overlay
// (features/editor/lsp-tooltips.js): the field holds the one overlay a view
// may show, and the facet is how CodeMirror places it and remaps it as the
// document changes, instead of this app recomputing coordinates on scroll.
// `hoverTooltip` is deliberately NOT re-exported: its dwell source cannot be
// invoked manually, and the command palette's Show Hover action has to reach
// the same overlay the pointer does.
export { EditorState, Compartment, Prec, ChangeSet, StateEffect, StateField } from '@codemirror/state';
export {
  EditorView, keymap, lineNumbers, highlightActiveLine,
  highlightActiveLineGutter, drawSelection, rectangularSelection,
  highlightSpecialChars, showTooltip,
} from '@codemirror/view';
export { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
export {
  syntaxHighlighting, HighlightStyle, StreamLanguage,
  bracketMatching, indentOnInput, foldGutter, foldKeymap,
} from '@codemirror/language';
export { searchKeymap, highlightSelectionMatches } from '@codemirror/search';
export { tags } from '@lezer/highlight';

// LSP completion (features/editor/lsp-completion.js). Deliberately narrow:
// `autocompletion` is the extension, the commands are what the completion key
// handler runs (list navigation included — with `defaultKeymap` off, nothing
// else binds the arrows), `completionStatus` is how every one of those
// decides whether to consume the key or hand it back to vim,
// `insertCompletionText` builds the plain insertion (caret placement and the
// `input.complete` user event included), and `snippet` is the applier a
// snippet item's expansion goes through.
//
// `completionKeymap` is NOT re-exported: autocompletion() is configured with
// `defaultKeymap: false` precisely so that one handler owns these keys.
export {
  autocompletion, startCompletion, closeCompletion, acceptCompletion,
  moveCompletionSelection, completionStatus, insertCompletionText, snippet,
} from '@codemirror/autocomplete';

// LSP diagnostics (features/editor/lsp-diagnostics.js). `setDiagnostics` is
// the whole write path: it returns a transaction spec and appends the lint
// state field itself the first time, so a pane needs no diagnostics
// configuration until a server actually publishes something. `linter` is
// mounted with a null source — this app never lints locally, it only renders
// what Rust normalized — purely so the lint config (delay, tooltip filter)
// has somewhere to live; `lintGutter` adds the severity markers next to the
// line numbers. `diagnosticCount` and `forEachDiagnostic` are the read side,
// which is how a test can assert what actually landed in a view's state
// rather than what we believe we dispatched.
//
// `lintKeymap`, the panel commands (`openLintPanel`/`closeLintPanel`) and
// `nextDiagnostic`/`previousDiagnostic` are deliberately NOT re-exported: F8
// traverses the workspace-wide Problems snapshot, not one document's marks,
// and CodeMirror's own lint panel would be a second, competing problems list.
export {
  linter, setDiagnostics, lintGutter, diagnosticCount, forEachDiagnostic,
} from '@codemirror/lint';

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
