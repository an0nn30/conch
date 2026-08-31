// Run: node scripts/tests/test_markdown_render.mjs
//
// The renderer is a pure string->string function by design, so this needs no
// DOM for the parsing half. DOMPurify does need one, so jsdom supplies it —
// the single test-only exception recorded in the design doc.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { createRequire } from 'node:module';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const SRC = path.join(ROOT, 'app/features/editor/preview/markdown-renderer.js');

// This test lives under scripts/tests/ at the repo root, but markdown-it,
// dompurify, markdown-it-task-lists, markdown-it-footnote, and jsdom are
// installed under crates/termlab_tauri/frontend/node_modules (they're the
// same packages Task 1's vendor bundle pulls in). A bare `import` would walk
// up from scripts/tests/ and never find that node_modules directory, so
// resolve explicitly against the frontend package instead of duplicating or
// relocating the dependencies.
const requireFromFrontend = createRequire(path.join(ROOT, 'package.json'));
const MarkdownIt = requireFromFrontend('markdown-it');
const createDOMPurify = requireFromFrontend('dompurify');
const taskListsPlugin = requireFromFrontend('markdown-it-task-lists');
const footnotePlugin = requireFromFrontend('markdown-it-footnote');
const { JSDOM } = requireFromFrontend('jsdom');

const sandbox = { window: {} };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(SRC, 'utf8'), sandbox);

const dom = new JSDOM('');
const renderer = sandbox.termlabMarkdownRenderer.createRenderer({
  MarkdownIt,
  DOMPurify: createDOMPurify(dom.window),
  taskListsPlugin,
  footnotePlugin,
});

// --- GFM constructs --------------------------------------------------------
const table = renderer.render('| a | b |\n|---|---|\n| 1 | 2 |');
// No closing `>` in the pattern: the source-line mapping rule below tags
// every top-level block, table included, with a data-src-line attribute
// (the same reason the footnote assertion below matches `<section` rather
// than `<section>`).
assert.match(table, /<table/, 'GFM tables must render');
assert.match(table, /<td>1<\/td>/, 'table cells must render');

const strike = renderer.render('~~gone~~');
assert.match(strike, /<s>gone<\/s>/, 'strikethrough must render');

const task = renderer.render('- [x] done\n- [ ] todo');
assert.match(task, /type="checkbox"/, 'task lists must render as checkboxes');
assert.match(task, /disabled/, 'task list checkboxes must not be interactive');

const footnote = renderer.render('text[^1]\n\n[^1]: the note');
assert.match(footnote, /footnote-ref/, 'footnote references must render');
assert.match(footnote, /<section/, 'the footnote section must survive sanitizing');
assert.match(footnote, /the note/, 'footnote body text must render');

// --- source mapping --------------------------------------------------------
// Scroll sync reads these back off the rendered elements, so every top-level
// block needs one and they must ascend with the source.
const mapped = renderer.render('# One\n\nPara two\n\n## Three');
const lines = [...mapped.matchAll(/data-src-line="(\d+)"/g)].map((m) => Number(m[1]));
assert.ok(lines.length >= 3, `expected >=3 mapped blocks, got ${lines.length}`);
assert.deepStrictEqual(
  lines, [...lines].sort((a, b) => a - b),
  'data-src-line values must ascend with source order',
);
assert.strictEqual(lines[0], 0, 'first block maps to source line 0');

// --- inline code is not a fence -------------------------------------------
assert.match(renderer.render('`x`'), /<code>x<\/code>/, 'inline code must render');

console.log('test_markdown_render: ok');
