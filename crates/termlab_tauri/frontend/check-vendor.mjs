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

const missing = expected.filter((name) => globalThis.CM6[name] === undefined);
if (!globalThis.CM6) throw new Error('CM6 global not defined — check globalName');
if (missing.length) throw new Error(`missing from bundle: ${missing.join(', ')}`);
console.log(`vendor check: ${expected.length} exports present`);
