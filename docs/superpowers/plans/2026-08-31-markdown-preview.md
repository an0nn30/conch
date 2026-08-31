# Markdown Preview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render markdown files in the light editor with three per-pane view modes (Editor / Split / Preview), matching the VS Code and IntelliJ workflow.

**Architecture:** `markdown-it` parses GFM into HTML carrying source-line attributes; fenced code is highlighted by reusing the CodeMirror grammars already vendored; DOMPurify strips the result to an allowlist; the HTML is injected into an `<iframe sandbox="allow-same-origin">` with `allow-scripts` deliberately absent, so nothing in a markdown file can execute. Images are inlined as `data:` URIs — local ones through a new scoped Rust command, remote ones over the pane's existing SFTP session. Scroll sync is one-directional, editor to preview.

**Tech Stack:** Vanilla IIFE JavaScript (no bundler for app code), esbuild for vendor bundles, CodeMirror 6, markdown-it, DOMPurify, Rust/Tauri v2, jsdom (tests only).

**Spec:** `docs/superpowers/specs/2026-08-31-markdown-preview-design.md`

## Global Constraints

- **Never commit to `main`.** All work goes on `feat/markdown-preview`.
- **No Co-Authored-By lines** in commit messages. Imperative mood.
- App JavaScript is **plain IIFE `<script>` files, not ES modules**. Each file exposes one global (e.g. `window.termlabMarkdownRenderer`). Script order in `index.html` matters — add tags after dependencies.
- **Only vendor bundles are built.** App modules are never bundled or transpiled.
- The iframe MUST be `sandbox="allow-same-origin"` with **no `allow-scripts`**. Adding `allow-scripts` defeats the entire security design.
- `http(s)` image sources are dropped at the **sanitizer**, never merely hidden at render time — a blocked image must never issue a network request.
- CSS uses `--tl-*` design tokens. **Never hardcode hex colors.** Resolved token values are snapshotted into the iframe because custom properties do not cascade across documents.
- New config fields use `#[serde(default)]` for backward compatibility.
- Rust: prefer `pub(crate)`, avoid `.unwrap()`, use `log::warn!`/`log::error!` over panics.
- Every behavior change needs tests. Run `cargo test --workspace`, `cargo clippy --all-targets`, `cargo fmt -- --check` before declaring done.
- `jsdom` is **devDependencies only** — never re-exported from a vendor entry, never shipped.

## File Structure

**New — frontend modules** (`crates/termlab_tauri/frontend/`)

| File | Responsibility |
|---|---|
| `vendor-markdown-entry.mjs` | ESM entry bundling markdown-it + DOMPurify + lezer highlight helpers into the `MDLib` global |
| `app/features/editor/preview/markdown-renderer.js` | **Pure.** markdown string → `{ html }`. Parse, highlight fences, sanitize. No DOM ownership, no I/O, no Tauri |
| `app/features/editor/preview/preview-frame.js` | Owns the sandboxed iframe: content injection, theme snapshot, scroll-to-line, link interception |
| `app/features/editor/preview/image-resolver.js` | Image path → `data:` URI, with cache and generation-based cancellation |
| `app/features/editor/preview/preview-mode.js` | Three-mode state machine and internal layout for one pane |
| `styles/design-system/components/markdown-preview.css` | Preview typography and `.tok-*` syntax classes |

**New — tests** (`scripts/tests/`)

`test_markdown_render.mjs`, `test_markdown_sanitize.mjs`, `test_markdown_highlight.mjs`, `test_markdown_image_resolve.mjs`, `test_preview_mode.mjs`

**Modified**

`frontend/package.json`, `frontend/build-vendor.mjs`, `frontend/index.html`, `frontend/check-vendor.mjs`, `app/features/editor/language-map.js`, `app/features/editor/editor-pane.js`, `app/features/editor/editor-service.js`, `crates/termlab_tauri/src/editor_fs.rs`, `crates/termlab_tauri/src/lib.rs`, `crates/termlab_core/src/config/editor.rs`, `crates/termlab_core/src/config/termlab.rs`, `config.example.toml`, `README.md`, `CLAUDE.md`

---

### Task 1: Vendor markdown-it, DOMPurify, and jsdom

Adds a **second** vendor bundle rather than extending `vendor-entry.mjs`, whose header declares it "the app's entire CodeMirror API surface". Keeping markdown vendoring separate preserves that contract.

**Files:**
- Create: `crates/termlab_tauri/frontend/vendor-markdown-entry.mjs`
- Modify: `crates/termlab_tauri/frontend/package.json`
- Modify: `crates/termlab_tauri/frontend/build-vendor.mjs`
- Modify: `crates/termlab_tauri/frontend/check-vendor.mjs`
- Test: `scripts/tests/test_markdown_vendor.mjs`

**Interfaces:**
- Consumes: nothing (first task)
- Produces: global `MDLib` with `{ MarkdownIt, DOMPurify, taskListsPlugin, footnotePlugin, highlightCode, classHighlighter }`, loaded from `vendor/markdown/markdown.js`

- [ ] **Step 1: Install the dependencies**

```bash
cd crates/termlab_tauri/frontend
npm install --save-dev markdown-it@^14.1.0 markdown-it-task-lists@^2.1.1 \
  markdown-it-footnote@^4.0.0 dompurify@^3.2.0 jsdom@^25.0.0
```

`markdown-it`, its two plugins, and `dompurify` are bundled into the vendor
output. `jsdom` is **only** for `scripts/tests` and must never appear in a
vendor entry.

The plugins are required, not optional: markdown-it core renders `- [x] done`
as the literal text `[x] done` and `[^1]` as a broken link. Verified against
markdown-it 14.3.1 — task lists and footnotes do **not** work without them.

- [ ] **Step 2: Write the failing test**

Create `scripts/tests/test_markdown_vendor.mjs`:

```javascript
// Run: node scripts/tests/test_markdown_vendor.mjs
//
// Pins the markdown vendor bundle's contract: the four names the app's own
// modules resolve against MDLib. A missing export here fails as a confusing
// "undefined is not a function" deep inside the renderer, so it is asserted
// at the boundary instead.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const BUNDLE = path.join(ROOT, 'vendor/markdown/markdown.js');

assert.ok(fs.existsSync(BUNDLE), `vendor bundle missing: run "npm run build:vendor" in ${ROOT}`);

const sandbox = { window: {}, globalThis: {}, self: {} };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(BUNDLE, 'utf8'), sandbox);

const MDLib = sandbox.MDLib;
assert.ok(MDLib, 'bundle must define the MDLib global');
assert.strictEqual(typeof MDLib.MarkdownIt, 'function', 'MDLib.MarkdownIt must be a constructor');
assert.strictEqual(typeof MDLib.DOMPurify, 'function', 'MDLib.DOMPurify must be a factory');
assert.strictEqual(typeof MDLib.taskListsPlugin, 'function', 'MDLib.taskListsPlugin must be a function');
assert.strictEqual(typeof MDLib.footnotePlugin, 'function', 'MDLib.footnotePlugin must be a function');
assert.strictEqual(typeof MDLib.highlightCode, 'function', 'MDLib.highlightCode must be a function');
assert.ok(MDLib.classHighlighter, 'MDLib.classHighlighter must be present');

console.log('test_markdown_vendor: ok');
```

- [ ] **Step 3: Run it and watch it fail**

Run: `node scripts/tests/test_markdown_vendor.mjs`
Expected: FAIL — `vendor bundle missing: run "npm run build:vendor" ...`

- [ ] **Step 4: Create the vendor entry**

Create `crates/termlab_tauri/frontend/vendor-markdown-entry.mjs`:

```javascript
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
```

- [ ] **Step 5: Teach the build about the second bundle**

In `crates/termlab_tauri/frontend/build-vendor.mjs`, replace the single `build()` call with a loop over both entries:

```javascript
const bundles = [
  { entry: 'vendor-entry.mjs', outDir: ['vendor', 'codemirror'], out: 'codemirror.js', globalName: 'CM6' },
  { entry: 'vendor-markdown-entry.mjs', outDir: ['vendor', 'markdown'], out: 'markdown.js', globalName: 'MDLib' },
];

for (const b of bundles) {
  const dir = path.join(here, ...b.outDir);
  mkdirSync(dir, { recursive: true });
  await build({
    entryPoints: [path.join(here, b.entry)],
    outfile: path.join(dir, b.out),
    bundle: true,
    format: 'iife',
    globalName: b.globalName,
    minify: true,
    target: 'es2020',
    legalComments: 'none',
  });
  console.log(`vendor: wrote ${b.outDir.join('/')}/${b.out}`);
}
```

- [ ] **Step 6: Gitignore the new bundle**

Built bundles are not committed — `.gitignore:56` already excludes
`crates/termlab_tauri/frontend/vendor/codemirror/`. Add the sibling line so the
markdown bundle is treated the same way:

```
crates/termlab_tauri/frontend/vendor/markdown/
```

- [ ] **Step 7: Add the bundle to the vendor check**

In `check-vendor.mjs`, add `vendor/markdown/markdown.js` alongside the existing CodeMirror bundle assertion, matching whatever shape that file already uses.

- [ ] **Step 8: Build and verify the test passes**

```bash
cd crates/termlab_tauri/frontend && npm run build:vendor
cd - && node scripts/tests/test_markdown_vendor.mjs
```
Expected: PASS — `test_markdown_vendor: ok`

- [ ] **Step 9: Commit**

```bash
# NOTE: the built bundle itself is gitignored and must NOT be added.
git add .gitignore \
        crates/termlab_tauri/frontend/package.json \
        crates/termlab_tauri/frontend/package-lock.json \
        crates/termlab_tauri/frontend/vendor-markdown-entry.mjs \
        crates/termlab_tauri/frontend/build-vendor.mjs \
        crates/termlab_tauri/frontend/check-vendor.mjs \
        scripts/tests/test_markdown_vendor.mjs
git commit -m "Vendor markdown-it and DOMPurify as a second bundle"
```

---

### Task 2: Markdown renderer — GFM to HTML with source lines

The renderer is a **pure function**: markdown in, HTML string out. It never touches the document, never calls Tauri, and never fetches anything. That is what makes it and the sanitizer testable under plain Node.

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/preview/markdown-renderer.js`
- Test: `scripts/tests/test_markdown_render.mjs`

**Interfaces:**
- Consumes: `MDLib.MarkdownIt`, `MDLib.DOMPurify` (Task 1)
- Produces: `window.termlabMarkdownRenderer` with:
  - `createRenderer(deps) -> { render(markdown: string) -> string }` where `deps` is `{ MarkdownIt, DOMPurify, highlight? }`. `highlight` is an optional `(code: string, lang: string) => string|null` added in Task 4; when absent or returning null, fences render as plain escaped code.

- [ ] **Step 1: Write the failing test**

Create `scripts/tests/test_markdown_render.mjs`:

```javascript
// Run: node scripts/tests/test_markdown_render.mjs
//
// The renderer is a pure string->string function by design, so this needs no
// DOM for the parsing half. DOMPurify does need one, so jsdom supplies it —
// the single test-only exception recorded in the design doc.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { JSDOM } from 'jsdom';
import MarkdownIt from 'markdown-it';
import createDOMPurify from 'dompurify';
import taskListsPlugin from 'markdown-it-task-lists';
import footnotePlugin from 'markdown-it-footnote';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const SRC = path.join(ROOT, 'app/features/editor/preview/markdown-renderer.js');

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
assert.match(table, /<table>/, 'GFM tables must render');
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
```

- [ ] **Step 2: Run it and watch it fail**

Run: `node scripts/tests/test_markdown_render.mjs`
Expected: FAIL — `Cannot read properties of undefined (reading 'createRenderer')`

- [ ] **Step 3: Write the renderer**

Create `crates/termlab_tauri/frontend/app/features/editor/preview/markdown-renderer.js`:

```javascript
// Markdown source -> sanitized HTML. Pure: no DOM ownership, no Tauri, no I/O.
//
// Dependencies arrive through createRenderer rather than being read off
// globals, which is the whole reason the parser and the sanitizer policy can
// be exercised under Node with no app bootstrap.
//
// The sanitizer runs LAST and unconditionally. Everything upstream — parser
// output, highlighted fences, raw HTML the author embedded — is treated as
// untrusted; a .md file is content the user merely opened, not code they
// chose to enable.
(function initTermLabMarkdownRenderer(global) {
  'use strict';

  // Tags real READMEs use, and nothing that can execute or navigate.
  // <script>, <style>, <iframe>, <object>, <embed>, <form> and every event
  // handler are absent deliberately, not by oversight.
  const ALLOWED_TAGS = [
    'p', 'br', 'hr', 'blockquote', 'pre', 'code', 'span', 'div',
    'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'ul', 'ol', 'li', 'dl', 'dt', 'dd',
    'table', 'thead', 'tbody', 'tr', 'th', 'td',
    'a', 'img', 'em', 'strong', 's', 'del', 'ins', 'sup', 'sub', 'kbd', 'mark',
    'details', 'summary', 'input',
    // markdown-it-footnote wraps the footnote list in <section class="footnotes">
    // with an <hr class="footnotes-sep">. Without `section` here the whole
    // footnote block is silently stripped and footnotes render as dangling refs.
    'section',
  ];

  // No `style` (CSS can exfiltrate via url()), no `on*`, no `srcset`.
  // `data-src-line` is explicit because ALLOW_DATA_ATTR is off.
  const ALLOWED_ATTR = [
    'href', 'title', 'alt', 'src', 'class', 'align', 'colspan', 'rowspan',
    'type', 'checked', 'disabled', 'open', 'id', 'data-src-line', 'data-img-ref',
  ];

  const SAFE_LINK = /^(https?:|mailto:|#|\/|\.\/|\.\.\/)/i;

  function createRenderer(deps) {
    const options = deps || {};
    const MarkdownIt = options.MarkdownIt;
    const DOMPurify = options.DOMPurify;
    const taskListsPlugin = options.taskListsPlugin || null;
    const footnotePlugin = options.footnotePlugin || null;
    const highlight = typeof options.highlight === 'function' ? options.highlight : null;
    if (!MarkdownIt || !DOMPurify) return null;

    const md = new MarkdownIt({
      html: true,        // raw HTML is PARSED, then sanitized below
      linkify: true,
      breaks: false,
      highlight(code, lang) {
        if (!highlight) return '';   // '' => markdown-it escapes it itself
        return highlight(code, lang) || '';
      },
    });

    // Task lists and footnotes are plugins, not core markdown-it. Left off,
    // "- [x] done" renders as the literal text "[x] done".
    //
    // markdown-it-task-lists is used at its DEFAULT, which emits `disabled`
    // checkboxes. Passing { enabled: true } would make them clickable, and a
    // preview is a view of the file, not an editor for it.
    if (taskListsPlugin) md.use(taskListsPlugin);
    if (footnotePlugin) md.use(footnotePlugin);

    // Attach source lines to top-level blocks. markdown-it gives every block
    // token a `.map` of [startLine, endLine]; level 0 keeps this to the
    // outermost blocks so scroll sync has one anchor per visual chunk rather
    // than one per nested list item.
    md.core.ruler.push('termlab_source_lines', (state) => {
      for (const token of state.tokens) {
        if (token.level === 0 && token.map && token.nesting !== -1) {
          token.attrSet('data-src-line', String(token.map[0]));
        }
      }
      return true;
    });

    // Drop remote images at SANITIZE time, not render time. Doing it here
    // means the element never reaches a document, so no request is ever
    // issued — hiding it after the fact would already have leaked.
    DOMPurify.addHook('afterSanitizeAttributes', (node) => {
      if (node.tagName === 'IMG') {
        const src = node.getAttribute('src') || '';
        if (/^https?:/i.test(src)) {
          node.removeAttribute('src');
          node.setAttribute('alt', `[remote image blocked] ${node.getAttribute('alt') || ''}`.trim());
          node.setAttribute('class', 'md-img-blocked');
        }
      }
      if (node.tagName === 'A') {
        const href = node.getAttribute('href') || '';
        if (href && !SAFE_LINK.test(href)) node.removeAttribute('href');
      }
      // Checkboxes come from task lists and must stay inert.
      if (node.tagName === 'INPUT') node.setAttribute('disabled', 'disabled');
    });

    function escapeText(text) {
      return String(text)
        .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }

    function render(markdown) {
      const source = typeof markdown === 'string' ? markdown : '';
      let raw;
      try {
        raw = md.render(source);
      } catch (err) {
        // A parser failure is reported IN the preview rather than as a toast:
        // the preview is the thing that failed, and a toast would fire again
        // on every debounced re-render while the document stays broken.
        return `<div class="md-render-error"><strong>Preview failed to render.</strong>`
          + `<pre>${escapeText(err && err.message ? err.message : err)}</pre></div>`;
      }
      return DOMPurify.sanitize(raw, {
        ALLOWED_TAGS,
        ALLOWED_ATTR,
        ALLOW_DATA_ATTR: false,
        FORBID_TAGS: ['script', 'style', 'iframe', 'object', 'embed', 'form', 'base', 'link'],
        FORBID_ATTR: ['style', 'srcset', 'formaction', 'ping'],
      });
    }

    return { render };
  }

  global.termlabMarkdownRenderer = { createRenderer };
})(window);
```

- [ ] **Step 4: Run the test and verify it passes**

Run: `node scripts/tests/test_markdown_render.mjs`
Expected: PASS — `test_markdown_render: ok`

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor/preview/markdown-renderer.js \
        scripts/tests/test_markdown_render.mjs
git commit -m "Add pure markdown renderer with GFM and source-line mapping"
```

---

### Task 3: Sanitizer XSS corpus

The renderer already sanitizes; this task proves it. Treat the corpus as permanently open — any bypass found later gets a case here rather than a patch elsewhere.

**Files:**
- Test: `scripts/tests/test_markdown_sanitize.mjs`
- Modify (only if a case fails): `app/features/editor/preview/markdown-renderer.js`

**Interfaces:**
- Consumes: `createRenderer` from Task 2
- Produces: nothing new — this is a verification task

- [ ] **Step 1: Write the corpus**

Create `scripts/tests/test_markdown_sanitize.mjs`:

```javascript
// Run: node scripts/tests/test_markdown_sanitize.mjs
//
// The security test for markdown preview. tauri.conf.json sets csp to null and
// the webview holds __TAURI__.invoke, so script execution in this document
// means file, SSH and vault access. The sandboxed iframe is the hard barrier;
// this pins the first one.
//
// Every case asserts on the SANITIZED STRING, before it reaches any document.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { JSDOM } from 'jsdom';
import MarkdownIt from 'markdown-it';
import createDOMPurify from 'dompurify';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const SRC = path.join(ROOT, 'app/features/editor/preview/markdown-renderer.js');

const sandbox = { window: {} };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(SRC, 'utf8'), sandbox);

const dom = new JSDOM('');
const renderer = sandbox.termlabMarkdownRenderer.createRenderer({
  MarkdownIt,
  DOMPurify: createDOMPurify(dom.window),
});

// Each case: markdown in, and substrings that must NOT survive.
const cases = [
  ['<script>alert(1)</script>',                      ['<script', 'alert(1)']],
  ['<img src=x onerror=alert(1)>',                   ['onerror']],
  ['<img src="x" onerror="alert(1)">',               ['onerror']],
  ['[click](javascript:alert(1))',                   ['javascript:']],
  ['[click](JaVaScRiPt:alert(1))',                   ['avascript:']],
  ['<a href="javascript:alert(1)">x</a>',            ['javascript:']],
  ['<iframe src="evil.html"></iframe>',              ['<iframe']],
  ['<object data="evil.swf"></object>',              ['<object']],
  ['<embed src="evil">',                             ['<embed']],
  ['<form action="/x"><input name="p"></form>',      ['<form', 'action']],
  ['<svg><script>alert(1)</script></svg>',           ['<script']],
  ['<svg onload="alert(1)"></svg>',                  ['onload']],
  ['<body onload="alert(1)">',                       ['onload']],
  ['<div style="background:url(http://e.com/x)">x</div>', ['style=']],
  ['<base href="http://evil.com/">',                 ['<base']],
  ['<link rel="stylesheet" href="http://e/x.css">',  ['<link']],
  ['<a href="data:text/html;base64,PHNjcmlwdD4=">x</a>', ['data:text/html']],
  ['<details open ontoggle="alert(1)">x</details>',  ['ontoggle']],
  ['<math><mtext><script>alert(1)</script></mtext></math>', ['<script']],
];

for (const [input, forbidden] of cases) {
  const out = renderer.render(input);
  for (const bad of forbidden) {
    assert.ok(
      !out.toLowerCase().includes(bad.toLowerCase()),
      `sanitizer leaked ${JSON.stringify(bad)}\n  input:  ${input}\n  output: ${out}`,
    );
  }
}

// --- remote images are dropped, local ones survive -------------------------
const remote = renderer.render('![b](https://img.shields.io/badge.svg)');
assert.ok(!remote.includes('https://img.shields.io'), 'remote image src must be dropped');
assert.match(remote, /md-img-blocked/, 'blocked images must be marked for the placeholder style');

const local = renderer.render('![logo](./img/logo.png)');
assert.match(local, /src="\.\/img\/logo\.png"/, 'local image paths must survive to the resolver');

// --- legitimate README HTML survives ---------------------------------------
const details = renderer.render('<details><summary>More</summary>\n\nhidden\n\n</details>');
assert.match(details, /<details>/, '<details> must survive');
assert.match(details, /<summary>/, '<summary> must survive');
assert.match(renderer.render('press <kbd>Cmd</kbd>'), /<kbd>/, '<kbd> must survive');

// --- scroll-sync attribute must not be stripped ----------------------------
assert.match(renderer.render('# h'), /data-src-line="0"/, 'data-src-line must survive sanitizing');

// --- a parser failure is reported, not thrown ------------------------------
// The renderer is called on every debounced keystroke; throwing would take the
// pane down mid-edit, so a failure has to become content.
{
  const exploding = sandbox.termlabMarkdownRenderer.createRenderer({
    MarkdownIt: function Broken() { return { render() { throw new Error('boom'); }, core: { ruler: { push() {} } } }; },
    DOMPurify: createDOMPurify(dom.window),
  });
  const out = exploding.render('# x');
  assert.match(out, /md-render-error/, 'a parse failure renders an inline error block');
  assert.match(out, /boom/, 'the error message is shown to the reader');
}

console.log(`test_markdown_sanitize: ok (${cases.length} attack cases)`);
```

- [ ] **Step 2: Run it**

Run: `node scripts/tests/test_markdown_sanitize.mjs`
Expected: PASS — `test_markdown_sanitize: ok (19 attack cases)`

If any case fails, fix `markdown-renderer.js` — tighten `ALLOWED_TAGS`, `ALLOWED_ATTR`, `FORBID_*`, or the `afterSanitizeAttributes` hook — and re-run. **Do not weaken an assertion to make it pass.**

- [ ] **Step 3: Commit**

```bash
git add scripts/tests/test_markdown_sanitize.mjs \
        crates/termlab_tauri/frontend/app/features/editor/preview/markdown-renderer.js
git commit -m "Add XSS corpus for the markdown sanitizer"
```

---

### Task 4: Fenced code highlighting via CodeMirror grammars

Reuses the grammars already re-exported from `vendor-entry.mjs`, so a `rust` fence highlights with the same tokens as the editor. No second grammar set and no `highlight.js`.

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/preview/fence-highlight.js`
- Test: `scripts/tests/test_markdown_highlight.mjs`

**Interfaces:**
- Consumes: `window.termlabEditorLanguageMap.languageKeyFor` (existing), `MDLib.highlightCode`, `MDLib.classHighlighter`, `CM6` grammars
- Produces: `window.termlabFenceHighlight` with `createHighlighter(deps) -> (code, lang) => string|null`, where `deps` is `{ CM, highlightCode, classHighlighter }`. Returns a full `<pre><code>...</code></pre>` string, or `null` for an unknown language so markdown-it falls back to its own escaping.

- [ ] **Step 1: Write the failing test**

Create `scripts/tests/test_markdown_highlight.mjs`:

```javascript
// Run: node scripts/tests/test_markdown_highlight.mjs
//
// Fence highlighting reuses the editor's own grammars. The real CM6 bundle
// needs a DOM, so — like test_editor_vim_glue.mjs — the two shapes this module
// actually touches are stubbed: a grammar exposing `.parser.parse()`, and
// lezer's highlightCode walker. What is pinned is the RESOLUTION logic (which
// grammar a fence language maps to, and what happens when it maps to nothing)
// plus the escaping, which is the security-relevant half.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const SRC = path.join(ROOT, 'app/features/editor/preview/fence-highlight.js');
const LANGUAGE_MAP = path.join(ROOT, 'app/features/editor/language-map.js');

const sandbox = { window: {} };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(LANGUAGE_MAP, 'utf8'), sandbox);
vm.runInContext(fs.readFileSync(SRC, 'utf8'), sandbox);

const fakeTree = { __tree: true };
// A LanguageSupport-shaped grammar: a factory returning { language: { parser } }.
const rust = () => ({ language: { parser: { parse: () => fakeTree } } });
// A legacy StreamParser-shaped grammar: a plain object, wrapped by StreamLanguage.
const toml = { token: () => null };

const CM = {
  rust,
  toml,
  StreamLanguage: { define: () => ({ parser: { parse: () => fakeTree } }) },
};

// Stand-in for lezer's walker: emits one classed span per whitespace-delimited
// word so the output shape can be asserted without a real grammar.
function highlightCode(code, _tree, _highlighter, putText, putBreak) {
  code.split('\n').forEach((line, i) => {
    if (i > 0) putBreak();
    line.split(/(\s+)/).forEach((chunk) => {
      putText(chunk, /^\s*$/.test(chunk) ? '' : 'tok-keyword');
    });
  });
}

const highlight = sandbox.termlabFenceHighlight.createHighlighter({
  CM, highlightCode, classHighlighter: {},
});

// --- known language --------------------------------------------------------
const out = highlight('fn main', 'rust');
assert.match(out, /^<pre class="md-code"/, 'must emit a pre wrapper');
assert.match(out, /<code/, 'must emit a code element');
assert.match(out, /class="tok-keyword"/, 'must emit highlight classes');
assert.match(out, /data-lang="rust"/, 'must record the language for styling');

// --- legacy stream grammar -------------------------------------------------
assert.ok(highlight('a = 1', 'toml'), 'legacy StreamParser grammars must work');

// --- unknown / absent language --------------------------------------------
assert.strictEqual(highlight('x', 'klingon'), null, 'unknown language returns null');
assert.strictEqual(highlight('x', ''), null, 'absent language returns null');

// --- escaping: the security-relevant half ---------------------------------
// A fence body is untrusted. Even though the sanitizer runs after this, the
// highlighter must not be the thing that introduces markup.
const nasty = highlight('</code><script>alert(1)</script>', 'rust');
assert.ok(!nasty.includes('<script>'), 'fence content must be escaped, not injected');
assert.match(nasty, /&lt;script&gt;/, 'angle brackets must be entity-escaped');

console.log('test_markdown_highlight: ok');
```

- [ ] **Step 2: Run it and watch it fail**

Run: `node scripts/tests/test_markdown_highlight.mjs`
Expected: FAIL — `Cannot read properties of undefined (reading 'createHighlighter')`

- [ ] **Step 3: Write the highlighter**

Create `crates/termlab_tauri/frontend/app/features/editor/preview/fence-highlight.js`:

```javascript
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
```

- [ ] **Step 4: Run the test and verify it passes**

Run: `node scripts/tests/test_markdown_highlight.mjs`
Expected: PASS — `test_markdown_highlight: ok`

- [ ] **Step 5: Confirm the renderer still passes with a highlighter attached**

Run: `node scripts/tests/test_markdown_render.mjs && node scripts/tests/test_markdown_sanitize.mjs`
Expected: both PASS. The `<pre class="md-code">` wrapper must survive sanitizing — `pre`, `code`, `span`, and `class` are all in the allowlist, and `data-lang` is not, so it is stripped harmlessly. If styling later needs `data-lang`, add it to `ALLOWED_ATTR` in Task 2 and re-run the corpus.

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor/preview/fence-highlight.js \
        scripts/tests/test_markdown_highlight.mjs
git commit -m "Highlight markdown code fences with the editor's grammars"
```

---

### Task 5: Rust — local image reads and config fields

**Files:**
- Modify: `crates/termlab_tauri/Cargo.toml`
- Modify: `crates/termlab_tauri/src/editor_fs.rs`
- Modify: `crates/termlab_tauri/src/lib.rs`
- Modify: `crates/termlab_core/src/config/editor.rs`
- Modify: `crates/termlab_core/src/config/termlab.rs`
- Modify: `config.example.toml`

**Interfaces:**
- Consumes: nothing from earlier tasks
- Produces:
  - Tauri command `editor_read_image_base64(path: String) -> Result<String, String>` returning a bare base64 payload (no `data:` prefix)
  - `EditorConfig::preview_default_mode: String` (`"editor"` | `"split"` | `"preview"`, default `"editor"`)
  - `KeyboardConfig::toggle_preview: String` (default `"cmd+shift+p"`)

- [ ] **Step 1: Write the failing Rust tests**

Append to the existing `#[cfg(test)] mod tests` in `crates/termlab_tauri/src/editor_fs.rs`:

```rust
#[test]
fn image_extensions_are_recognised() {
    for name in ["a.png", "b.JPG", "c.jpeg", "d.gif", "e.webp", "f.svg", "g.bmp"] {
        assert!(is_image_name(name), "{name} must be treated as an image");
    }
}

#[test]
fn non_image_extensions_are_rejected() {
    for name in ["a.txt", "b.rs", "c.md", "d", "e.png.exe"] {
        assert!(!is_image_name(name), "{name} must not be treated as an image");
    }
}

#[test]
fn image_mime_matches_extension() {
    assert_eq!(image_mime("a.png"), "image/png");
    assert_eq!(image_mime("a.JPG"), "image/jpeg");
    assert_eq!(image_mime("a.svg"), "image/svg+xml");
    assert_eq!(image_mime("a.unknown"), "application/octet-stream");
}

#[test]
fn oversized_images_are_refused() {
    assert!(
        check_image_size(MAX_IMAGE_BYTES + 1).is_err(),
        "an image over the cap must be refused rather than inlined"
    );
    assert!(check_image_size(1024).is_ok());
}
```

And in `crates/termlab_core/src/config/editor.rs`:

```rust
#[test]
fn preview_default_mode_defaults_to_editor() {
    assert_eq!(
        EditorConfig::default().preview_default_mode,
        "editor",
        "preview must be opt-in, never the default view"
    );
}

#[test]
fn preview_default_mode_round_trips() {
    let parsed: EditorConfig = toml::from_str(r#"preview_default_mode = "split""#).unwrap();
    assert_eq!(parsed.preview_default_mode, "split");
    let text = toml::to_string(&parsed).unwrap();
    let back: EditorConfig = toml::from_str(&text).unwrap();
    assert_eq!(back.preview_default_mode, "split");
}

#[test]
fn editor_config_without_preview_key_still_parses() {
    // Backward compatibility: configs written before this field existed.
    let parsed: EditorConfig = toml::from_str("vim_mode = true").unwrap();
    assert!(parsed.vim_mode);
    assert_eq!(parsed.preview_default_mode, "editor");
}
```

And in `crates/termlab_core/src/config/termlab.rs`:

```rust
#[test]
fn toggle_preview_has_a_default_binding() {
    assert_eq!(KeyboardConfig::default().toggle_preview, "cmd+shift+p");
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: FAIL — `cannot find function is_image_name`, `no field preview_default_mode`, `no field toggle_preview`

- [ ] **Step 3: Add the base64 dependency**

`base64` is already a workspace dependency (`Cargo.toml:45`) but is not yet
used by `termlab_tauri`. Add it to `crates/termlab_tauri/Cargo.toml` using the
same form `termlab_remote` uses:

```toml
base64 = { workspace = true }
```

- [ ] **Step 4: Implement the image command**

Add to `crates/termlab_tauri/src/editor_fs.rs`:

```rust
/// Largest image inlined into a preview. Images become base64 `data:` URIs, so
/// the cost is ~4/3 of this in the webview per image; a cap keeps one oversized
/// asset from stalling a render.
pub(crate) const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

fn image_extension(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let dot = base.rfind('.')?;
    if dot == 0 {
        return None;
    }
    Some(base[dot + 1..].to_ascii_lowercase())
}

pub(crate) fn is_image_name(name: &str) -> bool {
    matches!(
        image_extension(name).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico")
    )
}

pub(crate) fn image_mime(name: &str) -> &'static str {
    match image_extension(name).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

pub(crate) fn check_image_size(bytes: u64) -> Result<(), String> {
    if bytes > MAX_IMAGE_BYTES {
        return Err(format!(
            "image is {bytes} bytes, over the {MAX_IMAGE_BYTES} byte preview limit"
        ));
    }
    Ok(())
}

/// Read a local image for the markdown preview, base64-encoded.
///
/// Returns the payload only — the caller builds the `data:` URI, because it
/// already knows the MIME type from the same filename.
#[tauri::command]
pub(crate) fn editor_read_image_base64(path: String) -> Result<String, String> {
    use base64::Engine;

    if !is_image_name(&path) {
        return Err(format!("not a recognised image file: {path}"));
    }
    let meta = std::fs::metadata(&path).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path}: not a file"));
    }
    check_image_size(meta.len())?;

    let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}
```

Register it in `crates/termlab_tauri/src/lib.rs` alongside the other `editor_fs::` commands in the `invoke_handler` list:

```rust
editor_fs::editor_read_image_base64,
```

- [ ] **Step 5: Add the config fields**

In `crates/termlab_core/src/config/editor.rs`, `EditorConfig` currently derives `Default`. Adding a non-empty string default means writing `Default` out by hand:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Route the editor's keystrokes through vim's modal keymap.
    pub vim_mode: bool,

    /// Which view mode a markdown file opens in: "editor", "split" or
    /// "preview". Defaults to "editor" so nothing changes for anyone who has
    /// not asked for a preview. Unrecognised values are treated as "editor"
    /// by the frontend rather than rejected, so a typo degrades to today's
    /// behaviour instead of failing the whole config load.
    pub preview_default_mode: String,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            vim_mode: false,
            preview_default_mode: "editor".to_string(),
        }
    }
}
```

In `crates/termlab_core/src/config/termlab.rs`, add the field to `KeyboardConfig` and its `Default`:

```rust
    pub toggle_preview: String,
```
```rust
            toggle_preview: "cmd+shift+p".into(),
```

Document both in `config.example.toml` under the existing `[editor]` and `[termlab.keyboard]` sections:

```toml
# Which view mode a markdown file opens in: "editor", "split" or "preview".
preview_default_mode = "editor"
```
```toml
toggle_preview = "cmd+shift+p"   # cycle editor -> split -> preview
```

- [ ] **Step 6: Verify**

```bash
cargo test --workspace
cargo clippy --all-targets
cargo fmt -- --check
```
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/termlab_tauri/Cargo.toml crates/termlab_tauri/src/editor_fs.rs crates/termlab_tauri/src/lib.rs \
        crates/termlab_core/src/config/editor.rs \
        crates/termlab_core/src/config/termlab.rs config.example.toml
git commit -m "Add local image reads and preview config for markdown preview"
```

---

### Task 6: Image resolver

Turns the `src` values the sanitizer let through into `data:` URIs. The cache and the generation counter are the whole point: without them the 150ms debounce would re-fetch every image on every keystroke, and over SFTP that is a stalled connection.

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/preview/image-resolver.js`
- Test: `scripts/tests/test_markdown_image_resolve.mjs`

**Interfaces:**
- Consumes: `editor_read_image_base64` (Task 5), `sftp_read_file` (existing)
- Produces: `window.termlabPreviewImages` with `createResolver(deps) -> { resolve(src, binding, forGeneration, docPath), currentGeneration(), nextGeneration(), clear() }`
  - `deps` is `{ invoke, mimeFor }`
  - `binding` is `null` for a local file, or `{ paneId, remotePath }` for a remote one
  - `docPath` is the local document path, used to resolve relative sources when `binding` is null
  - `resolve` returns `Promise<string|null>` — a `data:` URI, or `null` if the fetch failed or its generation went stale

- [ ] **Step 1: Write the failing test**

Create `scripts/tests/test_markdown_image_resolve.mjs`:

```javascript
// Run: node scripts/tests/test_markdown_image_resolve.mjs
//
// What this pins is the traffic behaviour, not the pixels: an image is fetched
// ONCE per pane, a debounced re-render costs nothing, and a fetch that lands
// after its render was superseded is discarded rather than written into a
// frame that has moved on. Over SFTP those three properties are the difference
// between a preview and a stalled connection.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const SRC = path.join(ROOT, 'app/features/editor/preview/image-resolver.js');

const sandbox = { window: {} };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(SRC, 'utf8'), sandbox);

function makeResolver() {
  const calls = [];
  const invoke = async (command, args) => {
    calls.push({ command, args });
    if (command === 'editor_read_image_base64') return 'TE9DQUw=';
    if (command === 'sftp_read_file') {
      // One short read: 4 base64 chars, fewer bytes than requested => done.
      return { data: 'UkVNT1RF', bytes_read: 6 };
    }
    throw new Error(`unexpected command ${command}`);
  };
  const resolver = sandbox.termlabPreviewImages.createResolver({
    invoke,
    mimeFor: () => 'image/png',
  });
  return { resolver, calls };
}

// --- local resolution ------------------------------------------------------
{
  const { resolver, calls } = makeResolver();
  const uri = await resolver.resolve('./img/a.png', null, resolver.currentGeneration(), '/home/u/doc.md');
  assert.match(uri, /^data:image\/png;base64,TE9DQUw=$/, 'local image becomes a data URI');
  assert.strictEqual(calls[0].command, 'editor_read_image_base64');
  assert.strictEqual(
    calls[0].args.path, '/home/u/img/a.png',
    'relative paths resolve against the document directory, not the cwd',
  );
}

// --- cache: the second resolve costs no I/O -------------------------------
{
  const { resolver, calls } = makeResolver();
  const gen = resolver.currentGeneration();
  await resolver.resolve('./a.png', null, gen, '/d/doc.md');
  await resolver.resolve('./a.png', null, gen, '/d/doc.md');
  assert.strictEqual(calls.length, 1, 'a cached image must not be fetched twice');
}

// --- cancellation: a stale generation discards its result -----------------
{
  const { resolver } = makeResolver();
  const stale = resolver.currentGeneration();
  resolver.nextGeneration();
  const out = await resolver.resolve('./a.png', null, stale, '/d/doc.md');
  assert.strictEqual(out, null, 'a result from a superseded render must be discarded');
}

// --- remote resolution -----------------------------------------------------
{
  const { resolver, calls } = makeResolver();
  const uri = await resolver.resolve(
    './img/b.png',
    { paneId: 7, remotePath: '/srv/docs/readme.md' },
    resolver.currentGeneration(),
    null,
  );
  assert.match(uri, /^data:image\/png;base64,UkVNT1RF$/, 'remote image becomes a data URI');
  assert.strictEqual(calls[0].command, 'sftp_read_file');
  assert.strictEqual(calls[0].args.paneId, 7, 'must reuse the pane SSH session');
  assert.strictEqual(
    calls[0].args.path, '/srv/docs/img/b.png',
    'remote relative paths resolve against the remote directory',
  );
}

// --- a failed fetch degrades, never throws --------------------------------
{
  const resolver = sandbox.termlabPreviewImages.createResolver({
    invoke: async () => { throw new Error('ENOENT'); },
    mimeFor: () => 'image/png',
  });
  const out = await resolver.resolve('./missing.png', null, resolver.currentGeneration(), '/d/doc.md');
  assert.strictEqual(out, null, 'a failed image resolves to null, it does not reject');
}

// --- absolute and remote-scheme sources -----------------------------------
{
  const { resolver, calls } = makeResolver();
  await resolver.resolve('/abs/x.png', null, resolver.currentGeneration(), '/d/doc.md');
  assert.strictEqual(calls[0].args.path, '/abs/x.png', 'absolute paths pass through unchanged');

  const skipped = await resolver.resolve('https://e.com/x.png', null, resolver.currentGeneration(), '/d/doc.md');
  assert.strictEqual(skipped, null, 'http(s) sources are never fetched');
}

console.log('test_markdown_image_resolve: ok');
```

- [ ] **Step 2: Run it and watch it fail**

Run: `node scripts/tests/test_markdown_image_resolve.mjs`
Expected: FAIL — `Cannot read properties of undefined (reading 'createResolver')`

- [ ] **Step 3: Write the resolver**

Create `crates/termlab_tauri/frontend/app/features/editor/preview/image-resolver.js`:

```javascript
// Image src -> data: URI, for the markdown preview.
//
// Two mechanisms carry this module, and both exist because the preview
// re-renders on a 150ms debounce while you type:
//
//   cache      — an image is fetched once per pane. After the first render,
//                further renders cost no I/O at all.
//   generation — every render bumps a counter. A fetch that returns against a
//                superseded generation drops its result instead of writing
//                into a frame that has moved on. Without it, an edit burst
//                queues a round-trip per keystroke per image, which over SFTP
//                is a stalled connection rather than a slow one.
//
// http(s) sources never reach here — the sanitizer drops them so no request is
// ever issued — but the guard is repeated below because this module must be
// safe to call directly.
(function initTermLabPreviewImages(global) {
  'use strict';

  const REMOTE_SCHEME = /^[a-z][a-z0-9+.-]*:/i;
  const SFTP_CHUNK = 1024 * 1024; // sftp_read_file caps each call at 1MB
  const MAX_REMOTE_CHUNKS = 8;    // ceiling of 8MB, matching MAX_IMAGE_BYTES

  function dirnameOf(filePath) {
    if (typeof filePath !== 'string' || !filePath) return '';
    const at = Math.max(filePath.lastIndexOf('/'), filePath.lastIndexOf('\\'));
    return at <= 0 ? '/' : filePath.slice(0, at);
  }

  // Join and flatten `.`/`..` without a URL constructor, which would need a
  // base origin these paths do not have.
  function joinPath(dir, rel) {
    if (rel.startsWith('/')) return rel;
    const parts = `${dir}/${rel}`.split(/[\\/]+/);
    const out = [];
    for (const part of parts) {
      if (part === '.' || part === '') continue;
      if (part === '..') { out.pop(); continue; }
      out.push(part);
    }
    return `/${out.join('/')}`;
  }

  function createResolver(deps) {
    const options = deps || {};
    const invoke = options.invoke;
    const mimeFor = typeof options.mimeFor === 'function'
      ? options.mimeFor
      : () => 'application/octet-stream';
    if (typeof invoke !== 'function') return null;

    const cache = new Map();
    let generation = 0;

    async function fetchLocal(absPath) {
      const b64 = await invoke('editor_read_image_base64', { path: absPath });
      return typeof b64 === 'string' ? b64 : null;
    }

    async function fetchRemote(paneId, absPath) {
      let out = '';
      let offset = 0;
      for (let i = 0; i < MAX_REMOTE_CHUNKS; i += 1) {
        const res = await invoke('sftp_read_file', {
          paneId, path: absPath, offset, length: SFTP_CHUNK,
        });
        if (!res || typeof res.data !== 'string') break;
        out += res.data;
        const read = Number(res.bytes_read) || 0;
        offset += read;
        // A short read means end of file. Anything else would loop forever on
        // a zero-byte response.
        if (read < SFTP_CHUNK) break;
      }
      return out || null;
    }

    // `docPath` is the local document path; `binding` is the remote one.
    async function resolve(src, binding, forGeneration, docPath) {
      if (typeof src !== 'string' || !src) return null;
      // Already inline, or a scheme this preview refuses to fetch.
      if (src.startsWith('data:') || REMOTE_SCHEME.test(src)) return null;

      const remote = binding && binding.remotePath ? binding : null;
      const baseDir = remote ? dirnameOf(remote.remotePath) : dirnameOf(docPath);
      const absPath = joinPath(baseDir, src);
      const key = `${remote ? remote.paneId : 'local'}:${absPath}`;

      if (cache.has(key)) return cache.get(key);

      let payload = null;
      try {
        payload = remote
          ? await fetchRemote(remote.paneId, absPath)
          : await fetchLocal(absPath);
      } catch (err) {
        // A missing or unreadable image is a placeholder, never a toast and
        // never a rejected render.
        payload = null;
      }

      // The render that asked for this has been superseded; drop the result
      // rather than caching work for a document state that no longer exists.
      if (forGeneration !== generation) return null;
      if (!payload) return null;

      const uri = `data:${mimeFor(absPath)};base64,${payload}`;
      cache.set(key, uri);
      return uri;
    }

    return {
      resolve,
      currentGeneration: () => generation,
      nextGeneration: () => { generation += 1; return generation; },
      clear: () => cache.clear(),
    };
  }

  global.termlabPreviewImages = { createResolver };
})(window);
```

- [ ] **Step 4: Run the test and verify it passes**

Run: `node scripts/tests/test_markdown_image_resolve.mjs`
Expected: PASS — `test_markdown_image_resolve: ok`

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor/preview/image-resolver.js \
        scripts/tests/test_markdown_image_resolve.mjs
git commit -m "Resolve preview images to data URIs with caching and cancellation"
```

---

### Task 7: Sandboxed preview frame and styles

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/preview/preview-frame.js`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/markdown-preview.css`
- Test: `scripts/tests/test_preview_frame.mjs`

**Interfaces:**
- Consumes: `termlabPreviewImages` (Task 6)
- Produces: `window.termlabPreviewFrame` with `createFrame(hostEl, deps) -> { setContent(html), scrollToLine(n), destroy() }`, where `deps` is `{ onLinkClick, readToken }`

- [ ] **Step 1: Write the failing test**

Create `scripts/tests/test_preview_frame.mjs`:

```javascript
// Run: node scripts/tests/test_preview_frame.mjs
//
// The sandbox attribute is the whole security design, so it is asserted as a
// literal rather than checked loosely: `allow-same-origin` lets the PARENT
// drive the frame's DOM (scroll sync, link interception) while the ABSENCE of
// allow-scripts means nothing inside the frame can execute. Adding
// allow-scripts would silently undo the feature's entire threat model, so this
// test exists to make that change fail loudly.
//
// No jsdom here: the frame's own contract is attribute-level, and the minimal
// element factory is the established idiom for this kind of test.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const SRC = path.join(ROOT, 'app/features/editor/preview/preview-frame.js');

function makeEl(tag) {
  return {
    tagName: String(tag).toUpperCase(),
    children: [], attrs: {}, style: {}, listeners: {},
    setAttribute(k, v) { this.attrs[k] = String(v); },
    getAttribute(k) { return Object.prototype.hasOwnProperty.call(this.attrs, k) ? this.attrs[k] : null; },
    appendChild(c) { this.children.push(c); return c; },
    removeChild(c) { this.children = this.children.filter((x) => x !== c); },
    addEventListener(t, fn) { (this.listeners[t] ||= []).push(fn); },
    remove() {},
  };
}

const sandbox = {
  window: {},
  document: { createElement: makeEl, documentElement: {} },
  getComputedStyle: () => ({ getPropertyValue: () => '#111111' }),
};
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(SRC, 'utf8'), sandbox);

const host = makeEl('div');
const frame = sandbox.termlabPreviewFrame.createFrame(host, { readToken: () => '#111111' });
assert.ok(frame, 'createFrame must return a handle');

const iframe = host.children.find((c) => c.tagName === 'IFRAME');
assert.ok(iframe, 'the frame must mount an iframe');

const sandboxAttr = iframe.getAttribute('sandbox');
assert.strictEqual(
  sandboxAttr, 'allow-same-origin',
  'sandbox must be exactly "allow-same-origin" — adding allow-scripts defeats the design',
);
assert.ok(
  !/allow-scripts/.test(sandboxAttr),
  'allow-scripts must never be present on the preview frame',
);

console.log('test_preview_frame: ok');
```

- [ ] **Step 2: Run it and watch it fail**

Run: `node scripts/tests/test_preview_frame.mjs`
Expected: FAIL — `Cannot read properties of undefined (reading 'createFrame')`

- [ ] **Step 3: Write the frame**

Create `crates/termlab_tauri/frontend/app/features/editor/preview/preview-frame.js`:

```javascript
// The sandboxed surface the rendered markdown is displayed in.
//
// sandbox="allow-same-origin" WITHOUT allow-scripts is the security design:
//
//   - no allow-scripts  => nothing in the document executes. A <script> that
//                          survived sanitizing is inert, and so is every
//                          inline handler.
//   - allow-same-origin => the PARENT can reach into the frame's DOM, which
//                          is how scroll sync and link interception work with
//                          no message protocol at all.
//
// A postMessage design would need a script INSIDE the frame to receive
// messages, which would require allow-scripts and undo the first property.
// Do not add it.
//
// Design tokens do not cascade across documents, so the palette is snapshotted
// from the parent's computed style and written into the frame as literal
// values on every setContent — which is also how a theme change restyles it.
(function initTermLabPreviewFrame(global) {
  'use strict';

  const TOKENS = [
    '--tl-bg', '--tl-fg', '--tl-fg-muted', '--tl-accent', '--tl-border',
    '--tl-row-hover', '--tl-danger', '--tl-terminal-bg',
  ];

  function defaultReadToken(name) {
    const styles = global.getComputedStyle(global.document.documentElement);
    return styles.getPropertyValue(name).trim();
  }

  function paletteCss(readToken) {
    const lines = TOKENS
      .map((name) => {
        const value = readToken(name);
        return value ? `  ${name}: ${value};` : '';
      })
      .filter(Boolean);
    return `:root {\n${lines.join('\n')}\n}`;
  }

  function createFrame(hostEl, deps) {
    if (!hostEl) return null;
    const options = deps || {};
    const readToken = typeof options.readToken === 'function' ? options.readToken : defaultReadToken;
    const onLinkClick = typeof options.onLinkClick === 'function' ? options.onLinkClick : null;

    const doc = global.document;
    const iframe = doc.createElement('iframe');
    iframe.setAttribute('sandbox', 'allow-same-origin');
    iframe.setAttribute('class', 'md-preview-frame');
    iframe.setAttribute('title', 'Markdown preview');
    hostEl.appendChild(iframe);

    function frameDoc() {
      return iframe.contentDocument || null;
    }

    function setContent(html) {
      const shell = [
        '<!doctype html><html><head><meta charset="utf-8">',
        `<style>${paletteCss(readToken)}</style>`,
        `<link rel="stylesheet" href="../../../styles/design-system/components/markdown-preview.css">`,
        '</head><body class="md-preview-body">',
        typeof html === 'string' ? html : '',
        '</body></html>',
      ].join('');
      // srcdoc rather than document.write: the content is replaced atomically
      // and the sandbox attribute is re-applied to the new document.
      iframe.setAttribute('srcdoc', shell);
    }

    // Links are intercepted in the PARENT because the frame cannot run script.
    // Without this an external link would try to navigate the frame itself,
    // replacing the preview with a web page inside the app.
    function bindLinks() {
      const d = frameDoc();
      if (!d || !onLinkClick) return;
      d.addEventListener('click', (event) => {
        let node = event.target;
        while (node && node.tagName !== 'A') node = node.parentNode;
        if (!node) return;
        const href = node.getAttribute('href');
        if (!href) return;
        event.preventDefault();
        onLinkClick(href);
      });
    }

    iframe.addEventListener('load', bindLinks);

    // Scroll sync: find the last mapped block at or above `line` and bring it
    // to the top. One-directional by design — the preview never scrolls the
    // editor, so there is no feedback loop to guard against.
    function scrollToLine(line) {
      const d = frameDoc();
      if (!d || typeof d.querySelectorAll !== 'function') return;
      const blocks = d.querySelectorAll('[data-src-line]');
      let target = null;
      for (const el of blocks) {
        if (Number(el.getAttribute('data-src-line')) <= line) target = el;
        else break;
      }
      if (target && typeof target.scrollIntoView === 'function') {
        target.scrollIntoView({ block: 'start' });
      }
    }

    function destroy() {
      if (typeof iframe.remove === 'function') iframe.remove();
      else if (hostEl.removeChild) hostEl.removeChild(iframe);
    }

    return { setContent, scrollToLine, destroy, element: iframe };
  }

  global.termlabPreviewFrame = { createFrame };
})(window);
```

- [ ] **Step 4: Write the stylesheet**

Create `crates/termlab_tauri/frontend/styles/design-system/components/markdown-preview.css`. Every colour reads a `--tl-*` token — the frame injects resolved values for these, so the same rules work in light and dark. No hex literals.

```css
/* Markdown preview. Loaded INSIDE the sandboxed frame, where design tokens do
   not cascade in from the parent — preview-frame.js writes resolved values
   into a :root block ahead of this sheet. */
.md-preview-body {
  margin: 0;
  padding: 20px 28px;
  background: var(--tl-bg);
  color: var(--tl-fg);
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
  font-size: 14px;
  line-height: 1.6;
  overflow-wrap: break-word;
}
.md-preview-body h1, .md-preview-body h2 {
  border-bottom: 1px solid var(--tl-border);
  padding-bottom: 0.3em;
}
.md-preview-body h1 { font-size: 1.9em; margin: 0.6em 0 0.5em; }
.md-preview-body h2 { font-size: 1.5em; margin: 1.2em 0 0.5em; }
.md-preview-body h3 { font-size: 1.25em; margin: 1.1em 0 0.4em; }
.md-preview-body a { color: var(--tl-accent); }
.md-preview-body blockquote {
  margin: 0.8em 0;
  padding: 0.2em 1em;
  border-left: 3px solid var(--tl-border);
  color: var(--tl-fg-muted);
}
.md-preview-body table { border-collapse: collapse; margin: 1em 0; display: block; overflow-x: auto; }
.md-preview-body th, .md-preview-body td { border: 1px solid var(--tl-border); padding: 6px 12px; }
.md-preview-body th { background: var(--tl-row-hover); font-weight: 600; }
.md-preview-body tr:nth-child(2n) td { background: var(--tl-row-hover); }
.md-preview-body img { max-width: 100%; }
.md-preview-body hr { border: 0; border-top: 1px solid var(--tl-border); margin: 1.5em 0; }
.md-preview-body code {
  font-family: ui-monospace, "JetBrains Mono", Menlo, Consolas, monospace;
  font-size: 0.9em;
  background: var(--tl-row-hover);
  padding: 0.15em 0.4em;
  border-radius: 3px;
}
.md-preview-body pre.md-code {
  background: var(--tl-terminal-bg, var(--tl-bg));
  border: 1px solid var(--tl-border);
  border-radius: 4px;
  padding: 12px 14px;
  overflow-x: auto;
}
.md-preview-body pre.md-code code { background: none; padding: 0; font-size: 0.875em; }

/* An image the sanitizer refused to load. Named rather than silent so the
   reader can tell a blocked remote image from a broken local path. */
.md-preview-body .md-img-blocked {
  display: inline-block;
  padding: 2px 8px;
  border: 1px dashed var(--tl-border);
  border-radius: 3px;
  color: var(--tl-fg-muted);
  font-size: 0.85em;
}

/* Syntax classes emitted by lezer's classHighlighter. Mirrors the tag->colour
   choices in features/editor/theme.js so a fence and the editor agree. */
.md-preview-body .tok-keyword { color: var(--tl-accent); }
.md-preview-body .tok-string { color: var(--green, var(--tl-accent)); }
.md-preview-body .tok-comment { color: var(--tl-fg-muted); font-style: italic; }
.md-preview-body .tok-number,
.md-preview-body .tok-bool { color: var(--yellow, var(--tl-accent)); }
.md-preview-body .tok-variableName { color: var(--tl-fg); }
.md-preview-body .tok-function { color: var(--blue, var(--tl-fg)); }
.md-preview-body .tok-typeName,
.md-preview-body .tok-className { color: var(--cyan, var(--tl-accent)); }
.md-preview-body .tok-operator { color: var(--tl-fg-muted); }
.md-preview-body .tok-invalid { color: var(--tl-danger); }
```

- [ ] **Step 5: Run the test and verify it passes**

Run: `node scripts/tests/test_preview_frame.mjs`
Expected: PASS — `test_preview_frame: ok`

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor/preview/preview-frame.js \
        crates/termlab_tauri/frontend/styles/design-system/components/markdown-preview.css \
        scripts/tests/test_preview_frame.mjs
git commit -m "Add sandboxed preview frame and markdown preview styles"
```

---

### Task 8: Mode state machine

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/preview/preview-mode.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/language-map.js`
- Test: `scripts/tests/test_preview_mode.mjs`

**Interfaces:**
- Consumes: `termlabPreviewFrame` (Task 7), `termlabMarkdownRenderer` (Task 2)
- Produces:
  - `window.termlabEditorLanguageMap.isMarkdown(filename) -> boolean`
  - `window.termlabPreviewMode` with `createModeState(initial) -> { mode(), cycle(), set(mode), showsEditor(), showsPreview() }`. Valid modes: `'editor'`, `'split'`, `'preview'`; cycle order is editor → split → preview → editor.

- [ ] **Step 1: Write the failing test**

Create `scripts/tests/test_preview_mode.mjs`:

```javascript
// Run: node scripts/tests/test_preview_mode.mjs
//
// The mode machine is pure state — no DOM, no CodeMirror — so it is tested
// directly. What matters is that an unknown mode degrades to 'editor' rather
// than throwing: preview_default_mode comes from a hand-edited config file, and
// a typo there must not stop files from opening.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const sandbox = { window: {} };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(path.join(ROOT, 'app/features/editor/language-map.js'), 'utf8'), sandbox);
vm.runInContext(fs.readFileSync(path.join(ROOT, 'app/features/editor/preview/preview-mode.js'), 'utf8'), sandbox);

// --- markdown detection ----------------------------------------------------
const { isMarkdown } = sandbox.termlabEditorLanguageMap;
for (const name of ['a.md', 'B.MD', 'notes.markdown', '/p/to/README.md']) {
  assert.ok(isMarkdown(name), `${name} must be markdown`);
}
for (const name of ['a.txt', 'a.rs', 'markdown', '', null, undefined]) {
  assert.ok(!isMarkdown(name), `${JSON.stringify(name)} must not be markdown`);
}

// --- cycle order -----------------------------------------------------------
const { createModeState } = sandbox.termlabPreviewMode;
{
  const s = createModeState('editor');
  assert.strictEqual(s.mode(), 'editor');
  assert.strictEqual(s.cycle(), 'split');
  assert.strictEqual(s.cycle(), 'preview');
  assert.strictEqual(s.cycle(), 'editor', 'cycle must wrap back to editor');
}

// --- visibility derives from mode -----------------------------------------
{
  const s = createModeState('split');
  assert.ok(s.showsEditor() && s.showsPreview(), 'split shows both');
  s.set('preview');
  assert.ok(!s.showsEditor() && s.showsPreview(), 'preview hides the editor');
  s.set('editor');
  assert.ok(s.showsEditor() && !s.showsPreview(), 'editor hides the preview');
}

// --- bad input degrades ----------------------------------------------------
{
  assert.strictEqual(createModeState('nonsense').mode(), 'editor', 'unknown initial mode falls back');
  assert.strictEqual(createModeState(undefined).mode(), 'editor', 'absent initial mode falls back');
  const s = createModeState('editor');
  s.set('nope');
  assert.strictEqual(s.mode(), 'editor', 'set() ignores an unknown mode');
}

console.log('test_preview_mode: ok');
```

- [ ] **Step 2: Run it and watch it fail**

Run: `node scripts/tests/test_preview_mode.mjs`
Expected: FAIL — `isMarkdown is not a function`

- [ ] **Step 3: Add `isMarkdown` to the language map**

In `app/features/editor/language-map.js`, add alongside `languageKeyFor` and include it in the exported object:

```javascript
  // Whether a file gets the markdown preview. Derived from the SAME table that
  // drives highlighting, so a file that highlights as markdown can always be
  // previewed as markdown — the two can never drift apart.
  function isMarkdown(filename) {
    return languageKeyFor(filename) === 'markdown';
  }
```
```javascript
  global.termlabEditorLanguageMap = { languageKeyFor, isMarkdown };
```

- [ ] **Step 4: Write the mode state**

Create `crates/termlab_tauri/frontend/app/features/editor/preview/preview-mode.js`:

```javascript
// The Editor / Split / Preview state for one editor pane.
//
// Pure state, deliberately: layout and rendering read from it, nothing is
// stored here about the DOM. The mode is per-pane runtime state and is not
// persisted — `[editor] preview_default_mode` only seeds it.
(function initTermLabPreviewMode(global) {
  'use strict';

  const MODES = ['editor', 'split', 'preview'];

  function normalise(mode) {
    // An unknown value degrades to 'editor' rather than throwing.
    // preview_default_mode is hand-edited in config.toml, and a typo there
    // must not stop markdown files from opening.
    return MODES.includes(mode) ? mode : 'editor';
  }

  function createModeState(initial) {
    let mode = normalise(initial);
    return {
      mode: () => mode,
      set(next) { mode = normalise(next); return mode; },
      cycle() {
        mode = MODES[(MODES.indexOf(mode) + 1) % MODES.length];
        return mode;
      },
      showsEditor: () => mode !== 'preview',
      showsPreview: () => mode !== 'editor',
    };
  }

  global.termlabPreviewMode = { createModeState, MODES };
})(window);
```

- [ ] **Step 5: Run the test and verify it passes**

Run: `node scripts/tests/test_preview_mode.mjs`
Expected: PASS — `test_preview_mode: ok`

- [ ] **Step 6: Confirm nothing else regressed**

The language map is shared with the editor, so its own suite must still pass.

Run: `node scripts/tests/test_editor_vim_glue.mjs`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor/preview/preview-mode.js \
        crates/termlab_tauri/frontend/app/features/editor/language-map.js \
        scripts/tests/test_preview_mode.mjs
git commit -m "Add markdown preview mode state and isMarkdown helper"
```

---

### Task 9: Wire the preview into the editor

The integration task: mount the frame beside the CodeMirror view, drive re-render and scroll sync, bind the shortcut, load the scripts, and update the docs.

**Files:**
- Modify: `crates/termlab_tauri/frontend/index.html`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-pane.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-service.js`
- Modify: `crates/termlab_tauri/frontend/app/shortcut-runtime.js`
- Modify: `README.md`, `CLAUDE.md`

**Interfaces:**
- Consumes: everything from Tasks 1–8
- Produces: `termlabEditorPane.setPreviewMode(view, mode)` and `termlabEditorPane.togglePreview(view)`

- [ ] **Step 1: Load the new scripts**

In `index.html`, add after the existing CodeMirror vendor tag and the editor module tags. **Order matters** — `language-map.js` must already be loaded, and the vendor bundle before the modules that read `MDLib`:

```html
<script src="vendor/markdown/markdown.js"></script>
<script src="app/features/editor/preview/markdown-renderer.js"></script>
<script src="app/features/editor/preview/fence-highlight.js"></script>
<script src="app/features/editor/preview/image-resolver.js"></script>
<script src="app/features/editor/preview/preview-frame.js"></script>
<script src="app/features/editor/preview/preview-mode.js"></script>
```

- [ ] **Step 2: Mount the preview in the pane**

In `editor-pane.js`, add a `previews` `WeakMap` beside the existing compartment maps, and a `setPreviewMode(view, mode)` that:

1. Returns early unless `termlabEditorLanguageMap.isMarkdown(filename)`.
1. Returns early — leaving the pane in `editor` mode with the toggle inert — if
   `window.MDLib` is absent. The vendor bundle is built by `make frontend-vendor`
   and is gitignored, so a fresh checkout run with plain `cargo run` will not
   have it. Mirror the existing `bundleMissing()` treatment in
   `editor-service.js`: degrade quietly, never throw.
2. On first entry to `split`/`preview`, creates the host element and calls `termlabPreviewFrame.createFrame`.
3. Applies CSS classes for the layout (`md-mode-editor` / `md-mode-split` / `md-mode-preview`) on the pane's container.
4. Calls `renderNow(view)` after any mode change.

Keep this to mounting and layout only — the pane must not learn how markdown is parsed. Follow the existing `WeakMap` + compartment idiom rather than adding pane state elsewhere.

- [ ] **Step 3: Wire debounced re-render and scroll sync**

Extend the existing `dirtyWatcher` `updateListener` in `createEditorView`. It already fires on `update.docChanged`; add the debounce and the scroll hook:

```javascript
// Re-render the preview ~150ms after typing stops. Parsing is sub-millisecond;
// the debounce is there so image resolution and frame layout do not run on
// every keystroke — over SFTP that is the difference between one fetch and one
// per character.
let renderTimer = null;
const PREVIEW_DEBOUNCE_MS = 150;

const previewWatcher = CM.EditorView.updateListener.of((update) => {
  const preview = previews.get(view);
  if (!preview) return;
  if (update.docChanged) {
    if (renderTimer) clearTimeout(renderTimer);
    renderTimer = setTimeout(() => renderNow(view), PREVIEW_DEBOUNCE_MS);
  }
  // Editor drives preview; the preview never scrolls the editor, so there is
  // no loop to guard.
  if (update.geometryChanged || update.selectionSet) {
    const top = view.lineBlockAtHeight(view.scrollDOM.scrollTop).from;
    preview.frame.scrollToLine(view.state.doc.lineAt(top).number - 1);
  }
});
```

`renderNow` bumps the image generation, renders, injects, then resolves images and swaps them in:

```javascript
async function renderNow(view) {
  const preview = previews.get(view);
  if (!preview) return;
  const generation = preview.images.nextGeneration();
  const html = preview.renderer.render(view.state.doc.toString());
  preview.frame.setContent(html);
  // Images are resolved AFTER injection so text appears immediately; each
  // result is dropped if its generation was superseded meanwhile.
  await resolveImagesInto(preview, generation);
}

// Walk the freshly-injected frame and swap each unresolved <img> for a data:
// URI. Runs after setContent so text is readable while images are still in
// flight, and every result is generation-checked so a superseded render can
// never paint into the current frame.
async function resolveImagesInto(preview, generation) {
  const doc = preview.frame.element.contentDocument;
  if (!doc) return;
  const pending = [...doc.querySelectorAll('img[src]')].filter(
    (img) => !img.getAttribute('src').startsWith('data:'),
  );
  await Promise.all(pending.map(async (img) => {
    const uri = await preview.images.resolve(
      img.getAttribute('src'), preview.binding, generation, preview.docPath,
    );
    // Re-check: the frame may have been replaced while this was in flight.
    if (uri && generation === preview.images.currentGeneration()) img.setAttribute('src', uri);
  }));
}
```

- [ ] **Step 4: Offer the toggle only for markdown**

In `editor-service.js`, when a pane is created or renamed via Save As, call `setPreviewMode` with `config.editor.preview_default_mode` if `isMarkdown(filename)`, and otherwise force `'editor'`. Save As from `notes.md` to `notes.txt` must drop out of preview — reuse the existing `setPaneLanguage` call site, which already runs on exactly that event.

- [ ] **Step 5: Bind the shortcut**

In `shortcut-runtime.js`, register `toggle_preview` next to the other editor bindings. It must call `termlabEditorPane.togglePreview` on the focused pane and no-op when the focused pane is not a markdown editor.

- [ ] **Step 6: Verify the whole suite**

```bash
cargo test --workspace
cargo clippy --all-targets
cargo fmt -- --check
for t in scripts/tests/test_markdown_*.mjs scripts/tests/test_preview_*.mjs scripts/tests/test_editor_*.mjs; do
  node "$t" || echo "FAILED: $t"
done
```
Expected: all pass, no `FAILED:` lines.

- [ ] **Step 7: Run the app and walk the manual checklist**

```bash
make run
```

Work through **Manual verification** in the design doc — all seven items. Item 6 (open a file containing `<script>alert(1)</script>`, `<img src=x onerror=alert(1)>`, and an `https://` image) is the one that must not be skipped: confirm nothing executes and that the DevTools network tab shows **no request** for the remote image.

- [ ] **Step 8: Update the docs**

In `README.md`, extend the Built-in Editor feature paragraph with the preview, and add to the shortcut table:

```
| `Cmd+Shift+P` | Toggle markdown preview (editor / split / preview) |
```

In `CLAUDE.md`, add `preview/` to the `app/features/` line of the frontend tree, and add a Key Patterns bullet:

```
- **Markdown preview**: rendered HTML is sanitized and displayed in an iframe
  sandboxed WITHOUT `allow-scripts` — never add it. `csp` is null app-wide, so
  the sandbox is what keeps an opened `.md` file from reaching `__TAURI__`.
```

- [ ] **Step 9: Commit**

```bash
git add crates/termlab_tauri/frontend/index.html \
        crates/termlab_tauri/frontend/app/features/editor/ \
        crates/termlab_tauri/frontend/app/shortcut-runtime.js \
        README.md CLAUDE.md
git commit -m "Wire markdown preview into the light editor"
```

---

## Done when

- `cargo test --workspace`, `cargo clippy --all-targets`, `cargo fmt -- --check` all pass.
- Every `scripts/tests/test_markdown_*.mjs` and `test_preview_*.mjs` passes, and the pre-existing editor suites still pass.
- All seven manual verification items in the design doc have been walked, including the hostile-file case.
- The preview frame's `sandbox` attribute is exactly `allow-same-origin`.
