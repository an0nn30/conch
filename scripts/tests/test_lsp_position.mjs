// Run: node scripts/tests/test_lsp_position.mjs
//
// The one place LSP positions become CodeMirror offsets and back.
//
// This arithmetic used to exist in seven copies (editor-service, completion,
// completion-apply, diagnostics, tooltips, navigation, problems-navigation),
// each with its own idea of what to do with a line past the end of the
// document or a character past the end of a line. Two copies disagreeing is
// how a diagnostic lands on the wrong word, so it lives here now, tested
// against real CodeMirror documents rather than a stub — the clamping rules
// only mean anything against a real `Text`.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const MODULES = path.join(APP, 'features/editor');
const POSITION = path.join(MODULES, 'lsp-position.js');
const INDEX_HTML = path.join(ROOT, 'index.html');
const NODE_MODULES = path.join(ROOT, 'node_modules');

// Every module that used to carry its own copy. If one of them grows a new
// private conversion, this list is where the next reader finds out.
//
// problems-navigation.js is on the list but converts nothing any more: its
// reveal is editor-service's, so it has no positions of its own to place.
const PROBLEMS_NAVIGATION = path.join(APP, 'features/problems/problems-navigation.js');
const CONSUMERS = [
  path.join(MODULES, 'editor-service.js'),
  path.join(MODULES, 'lsp-completion.js'),
  path.join(MODULES, 'lsp-completion-apply.js'),
  path.join(MODULES, 'lsp-diagnostics.js'),
  path.join(MODULES, 'lsp-tooltips.js'),
  path.join(MODULES, 'lsp-navigation.js'),
];

// Objects built inside the vm context belong to another realm, so deepStrictEqual
// refuses them on prototype identity. Every structural comparison here goes
// through one JSON round trip first.
const plain = (value) => JSON.parse(JSON.stringify(value === undefined ? null : value));
function deepEq(actual, expected, message) {
  assert.deepStrictEqual(plain(actual), plain(expected), message);
}

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

const { Text } = await import(
  pathToFileURL(path.join(NODE_MODULES, '@codemirror/state/dist/index.js')).href
);

function load() {
  const sandbox = {
    console, Math, Number, String, Object, Array, JSON, Error, Boolean,
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(POSITION, 'utf8'), sandbox, { filename: POSITION });
  return sandbox.termlabLspPosition;
}

const positions = load();
const doc = Text.of(['const value = 1;', 'let other = 2;', '']);
// A surrogate pair: LSP counts UTF-16 code units and so does CodeMirror, so
// the emoji is TWO columns wide to both of them.
const unicodeDoc = Text.of(['const a = "💡";', 'x']);

// --- position -> offset --------------------------------------------------------

check('a position inside the document converts to its offset', () => {
  assert.strictEqual(positions.offsetAt(doc, { line: 0, character: 6 }), 6);
  assert.strictEqual(positions.offsetAt(doc, { line: 1, character: 4 }), 17 + 4);
});

check('a line past the end of the document clamps to the last line', () => {
  const at = positions.offsetAt(doc, { line: 99, character: 0 });
  assert.strictEqual(at, doc.line(doc.lines).from, 'clamped, not thrown');
  assert.ok(at <= doc.length);
});

check('a character past the end of its line clamps to the line end', () => {
  assert.strictEqual(positions.offsetAt(doc, { line: 0, character: 999 }), doc.line(1).to);
});

check('a negative or missing coordinate is treated as zero, never as a throw', () => {
  assert.strictEqual(positions.offsetAt(doc, { line: -4, character: -9 }), 0);
  assert.strictEqual(positions.offsetAt(doc, { line: 1 }), doc.line(2).from);
  assert.strictEqual(positions.offsetAt(doc, {}), 0);
  assert.strictEqual(positions.offsetAt(doc, null), 0);
  assert.strictEqual(positions.offsetAt(doc, { line: NaN, character: NaN }), 0);
});

check('positions count UTF-16 code units, so a surrogate pair is two columns', () => {
  const afterEmoji = positions.offsetAt(unicodeDoc, { line: 0, character: 13 });
  assert.strictEqual(unicodeDoc.sliceString(0, afterEmoji), 'const a = "💡');
});

// --- offset -> position --------------------------------------------------------

check('an offset converts back to a zero-based line and character', () => {
  deepEq(positions.positionAt(doc, 6), { line: 0, character: 6 });
  deepEq(positions.positionAt(doc, 21), { line: 1, character: 4 });
});

check('the two conversions round-trip', () => {
  for (const offset of [0, 5, 16, 17, 25, doc.length]) {
    const position = positions.positionAt(doc, offset);
    assert.strictEqual(positions.offsetAt(doc, position), offset, `round trip at ${offset}`);
  }
});

check('an offset outside the document is clamped rather than thrown', () => {
  deepEq(positions.positionAt(doc, -3), { line: 0, character: 0 });
  deepEq(
    positions.positionAt(doc, doc.length + 500), positions.positionAt(doc, doc.length),
  );
  deepEq(positions.positionAt(doc, NaN), { line: 0, character: 0 });
});

// --- ranges ---------------------------------------------------------------------

check('a range becomes a span whose end never precedes its start', () => {
  deepEq(
    positions.spanOf(doc, {
      start: { line: 0, character: 6 }, end: { line: 0, character: 11 },
    }),
    { from: 6, to: 11 },
  );
  deepEq(
    positions.spanOf(doc, {
      start: { line: 1, character: 4 }, end: { line: 0, character: 0 },
    }),
    { from: 21, to: 21 },
    'a reversed range collapses instead of selecting backwards',
  );
});

check('a range with no end anchors on its start', () => {
  deepEq(
    positions.spanOf(doc, { start: { line: 0, character: 3 } }), { from: 3, to: 3 },
  );
});

check('a missing range is the start of the document, not a crash', () => {
  deepEq(positions.spanOf(doc, null), { from: 0, to: 0 });
});

// --- line text ---------------------------------------------------------------
//
// The definition chooser previews a target line, which is the same clamping
// rule again: a server may name a line the document no longer has.

check('a line index reads back that line, clamped and trimmed of nothing', () => {
  assert.strictEqual(positions.lineTextAt(doc, 0), 'const value = 1;');
  assert.strictEqual(positions.lineTextAt(doc, 1), 'let other = 2;');
});

check('a line past the end reads the last line rather than throwing', () => {
  assert.strictEqual(positions.lineTextAt(doc, 99), doc.line(doc.lines).text);
  assert.strictEqual(positions.lineTextAt(doc, -2), doc.line(1).text);
});

check('a document that cannot answer yields an empty string', () => {
  assert.strictEqual(positions.lineTextAt(null, 0), '');
  assert.strictEqual(positions.lineTextAt({}, 0), '');
});

// --- wiring and hygiene ------------------------------------------------------------

check('index.html loads lsp-position.js before every module that converts', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const at = (name) => html.indexOf(name);
  assert.ok(at('app/features/editor/lsp-position.js') > 0, 'the module is loaded at all');
  for (const consumer of CONSUMERS) {
    const relative = `app/${path.relative(APP, consumer)}`;
    assert.ok(
      at('app/features/editor/lsp-position.js') < at(relative),
      `${relative} reads termlabLspPosition`,
    );
  }
});

check('no consumer keeps a private copy of the conversion', () => {
  for (const consumer of CONSUMERS.concat([PROBLEMS_NAVIGATION])) {
    const source = fs.readFileSync(consumer, 'utf8');
    if (consumer !== PROBLEMS_NAVIGATION) {
      assert.ok(
        /termlabLspPosition/.test(source),
        `${consumer} must use the shared helper`,
      );
    }
    // The two signatures of a private copy: `number - 1` is how an offset is
    // turned into a zero-based LSP line, and `.lines` is how a line index is
    // clamped to the document. Reading a line's TEXT (lsp-completion's
    // `textBeforeCaret`) is not a position conversion and is left alone.
    assert.ok(
      !/number - 1/.test(source),
      `${consumer} still converts offsets to positions itself`,
    );
    assert.ok(
      !/\.lines\b/.test(source),
      `${consumer} still clamps line numbers itself`,
    );
  }
});

check('the module is pure: no DOM, no CodeMirror, no Tauri', () => {
  const source = fs.readFileSync(POSITION, 'utf8');
  assert.ok(!/document\.|innerHTML/.test(source));
  assert.ok(!/\bCM6\b|__TAURI__|\binvoke\(/.test(source));
});

check('the module uses no regex lookbehind', () => {
  const source = fs.readFileSync(POSITION, 'utf8');
  assert.ok(!/\(\?<[=!]/.test(source), 'a lookbehind costs the whole file on an older WKWebView');
});

check('the module contains no control bytes', () => {
  const bytes = fs.readFileSync(POSITION);
  for (let i = 0; i < bytes.length; i += 1) {
    const byte = bytes[i];
    assert.ok(
      byte >= 0x20 || byte === 0x0a || byte === 0x09,
      `control byte 0x${byte.toString(16)} at offset ${i} — git treats the file as binary`,
    );
  }
});

for (const { name, fn } of queued) {
  ran += 1;
  try {
    await fn();
    console.log(`  ok   ${name}`);
  } catch (error) {
    failures += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${(error && error.stack) || error}`);
  }
}
if (failures) {
  console.log(`lsp position: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`lsp position: all ${ran} checks passed`);
}
