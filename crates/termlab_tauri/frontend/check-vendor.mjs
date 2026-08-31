// Asserts every name vendor-entry.mjs claims to export actually resolves in
// the built bundle. A missing name is otherwise silent: the language just
// never highlights.
import fs from 'node:fs';
import path from 'node:path';

const here = import.meta.dirname;
const entry = fs.readFileSync(path.join(here, 'vendor-entry.mjs'), 'utf8');
const expected = [...entry.matchAll(/export\s*\{([^}]*)\}/g)]
  .flatMap((m) => m[1].split(','))
  .map((s) => s.trim().split(/\s+as\s+/).pop().trim())
  .filter(Boolean);

globalThis.window = globalThis;
// Indirect eval (not `new Function`) so the bundle's top-level `var CM6 = ...`
// lands on globalThis, matching how a <script> tag runs it in a browser.
// `new Function(code)()` executes in the function's own scope, so the `var`
// stays local and globalThis.CM6 is never set.
(0, eval)(fs.readFileSync(path.join(here, 'vendor', 'codemirror', 'codemirror.js'), 'utf8'));

// Order matters: dereferencing CM6 before checking it exists turns a missing
// global into a TypeError instead of the message that names the cause.
if (!globalThis.CM6) throw new Error('CM6 global not defined — check globalName');
const missing = expected.filter((name) => globalThis.CM6[name] === undefined);
if (missing.length) throw new Error(`missing from bundle: ${missing.join(', ')}`);
console.log(`vendor check: ${expected.length} exports present`);

// Same check for the markdown bundle (vendor-markdown-entry.mjs -> MDLib).
// Kept as a parallel block rather than folding both bundles into a loop, so
// each bundle's check reads top-to-bottom on its own.
const mdEntry = fs.readFileSync(path.join(here, 'vendor-markdown-entry.mjs'), 'utf8');
const mdExpected = [...mdEntry.matchAll(/export\s*\{([^}]*)\}/g)]
  .flatMap((m) => m[1].split(','))
  .map((s) => s.trim().split(/\s+as\s+/).pop().trim())
  .filter(Boolean);

(0, eval)(fs.readFileSync(path.join(here, 'vendor', 'markdown', 'markdown.js'), 'utf8'));

if (!globalThis.MDLib) throw new Error('MDLib global not defined — check globalName');
const mdMissing = mdExpected.filter((name) => globalThis.MDLib[name] === undefined);
if (mdMissing.length) throw new Error(`missing from markdown bundle: ${mdMissing.join(', ')}`);
console.log(`vendor check: ${mdExpected.length} markdown exports present`);
