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
import { createRequire } from 'node:module';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const SRC = path.join(ROOT, 'app/features/editor/preview/markdown-renderer.js');

// This test lives under scripts/tests/ at the repo root, but markdown-it,
// dompurify, and jsdom are installed under crates/termlab_tauri/frontend/
// node_modules. A bare `import` would walk up from scripts/tests/ and never
// find that node_modules directory, so resolve explicitly against the
// frontend package instead of duplicating or relocating the dependencies.
const requireFromFrontend = createRequire(path.join(ROOT, 'package.json'));
const MarkdownIt = requireFromFrontend('markdown-it');
const createDOMPurify = requireFromFrontend('dompurify');
const { JSDOM } = requireFromFrontend('jsdom');

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
//
// Three cases below carry an empty forbidden-substring list on purpose:
// `[click](javascript:alert(1))`, its mixed-case twin, and the raw
// `<a href="javascript:alert(1)">` anchor. A plain substring check on these
// is both over- and under-inclusive — it would flag the harmless escaped
// text markdown-it produces when it refuses the link outright (no `<a>`, no
// `href`, nothing executable), and it would MISS a real bypass where an
// entity-encoded href decodes to a javascript:/data:/vbscript: scheme in the
// DOM without ever containing that literal substring in the serialized
// string. Those three, plus the entity-encoded bypass attempts, are asserted
// below against the actual sink instead: parsed anchor elements, not raw text.
const cases = [
  ['<script>alert(1)</script>',                      ['<script', 'alert(1)']],
  ['<img src=x onerror=alert(1)>',                   ['onerror']],
  ['<img src="x" onerror="alert(1)">',               ['onerror']],
  ['[click](javascript:alert(1))',                   []],
  ['[click](JaVaScRiPt:alert(1))',                   []],
  ['<a href="javascript:alert(1)">x</a>',            []],
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

// --- executable-scheme sink check -------------------------------------------
// Parses the sanitized output and returns every anchor whose href, once
// normalised, carries an executable scheme. Parsing (rather than string
// matching) means HTML entities are decoded exactly as a real DOM would
// decode them, so an entity-encoded bypass is caught even though it never
// contains the literal scheme substring. The href is also stripped of all
// whitespace and control characters before the scheme comparison, since a
// tab- or newline-injected scheme (`java\tscript:`) would otherwise dodge a
// naive `startsWith` check while still being resolved as `javascript:` by
// most URL parsers.
function executableSchemeAnchors(html) {
  const parsed = new JSDOM(`<body>${html}</body>`);
  const anchors = [...parsed.window.document.querySelectorAll('a[href]')];
  return anchors.filter((a) => {
    const raw = a.getAttribute('href') || '';
    const normalized = raw.replace(/[\s\x00-\x1f\x7f]+/g, '').toLowerCase();
    return /^(javascript|data|vbscript):/.test(normalized);
  });
}

function assertNoExecutableSchemeAnchor(input) {
  const out = renderer.render(input);
  const bad = executableSchemeAnchors(out);
  assert.strictEqual(
    bad.length, 0,
    `sanitizer produced an executable-scheme anchor\n  input:  ${input}\n  output: ${out}\n  hrefs:  ${bad.map((a) => a.getAttribute('href')).join(', ')}`,
  );
}

const sinkCases = [
  // Re-checked at the sink instead of by substring (see the comment above
  // the `cases` array).
  '[click](javascript:alert(1))',
  '[click](JaVaScRiPt:alert(1))',
  '<a href="javascript:alert(1)">x</a>',
  // Entity-encoded bypass attempts a substring check could not see: these
  // decode to a javascript: scheme only once parsed into the DOM.
  '[x](java&#115;cript:alert(1))',
  '[x](&#106;avascript:alert(1))',
  '<a href="&#106;avascript:alert(1)">x</a>',
  '<a href="&#x6a;avascript:alert(1)">x</a>',
  '[x](vbscript:msgbox(1))',
];

for (const input of sinkCases) {
  assertNoExecutableSchemeAnchor(input);
}

const newBypassCaseCount = 5; // the five entity/vbscript cases above that are not re-checks of existing `cases` entries

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

console.log(`test_markdown_sanitize: ok (${cases.length + newBypassCaseCount} attack cases)`);
