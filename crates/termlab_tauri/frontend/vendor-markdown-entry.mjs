// The ESM entry esbuild bundles into vendor/markdown/markdown.js as the IIFE
// global `MDLib`. Deliberately separate from vendor-entry.mjs, which declares
// itself the app's entire CodeMirror API surface — markdown vendoring is its
// own concern and keeping the two apart keeps that claim true.
//
// jsdom is NOT here and must never be: it is a test-only dependency for
// driving DOMPurify under Node, and bundling it would ship a DOM
// implementation to users who already have one.
export { default as MarkdownIt } from 'markdown-it';
export { default as DOMPurify } from 'dompurify';

// GFM constructs markdown-it core does NOT implement. Without these, a task
// list renders as the literal text "[x] done" and a footnote reference
// renders as a link to a nonexistent page.
export { default as taskListsPlugin } from 'markdown-it-task-lists';
export { default as footnotePlugin } from 'markdown-it-footnote';

// Standalone syntax highlighting for fenced code. `highlightCode` walks a
// parsed tree emitting (text, classes) pairs, which is how a code fence gets
// the editor's own highlighting without instantiating an EditorView per fence.
// `classHighlighter` maps tags to stable `tok-*` class names, so the frame can
// style them from CSS rather than needing inline colours computed per fence.
export { highlightCode, classHighlighter } from '@lezer/highlight';
