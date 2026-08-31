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
const makeGrammar = () => ({ language: { parser: { parse: () => fakeTree } } });
// A legacy StreamParser-shaped grammar: a plain object, wrapped by StreamLanguage.
const toml = { token: () => null };

// One CM6 key per grammar `languageKeyFor` can name — either directly from a
// fence tag that IS its own extension (go, json), or via fence-highlight.js's
// own FENCE_ALIASES table (python -> py -> 'python', etc.). This is the same
// table the editor's CM6 module namespace provides in the real app.
const CM = {
  rust: makeGrammar,
  toml,
  python: makeGrammar,
  javascript: makeGrammar,
  ruby: makeGrammar,
  shell: makeGrammar,
  powerShell: makeGrammar,
  cpp: makeGrammar,
  perl: makeGrammar,
  go: makeGrammar,
  json: makeGrammar,
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

// --- fence-tag aliases: language name != file extension --------------------
// `go` and `json` already work by luck — the fence tag happens to be the
// extension too. `python`/`javascript`/etc. do NOT share that luck, and are
// exactly the tags a real README uses, so FENCE_ALIASES has to carry them.
for (const tag of [
  'python', 'javascript', 'typescript', 'ruby', 'rust',
  'shell', 'powershell', 'c++', 'perl', 'go', 'json',
]) {
  assert.ok(highlight('x', tag), `fence tag "${tag}" must resolve to a grammar`);
}

// --- "no highlighting" is the correct answer for these ---------------------
assert.strictEqual(highlight('x', 'text'), null, '"text" has no grammar and must stay plain');
assert.strictEqual(highlight('x', 'plain'), null, '"plain" has no grammar and must stay plain');

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
