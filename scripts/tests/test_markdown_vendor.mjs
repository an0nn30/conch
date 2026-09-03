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
