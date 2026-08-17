// Bundles third-party frontend dependencies into a single IIFE global.
//
// The app's own modules are plain IIFE <script> files and are deliberately NOT
// built — this exists only so CodeMirror 6, which is ESM-only, can be consumed
// by a frontend with no module system.
import { build } from 'esbuild';
import { mkdirSync } from 'node:fs';
import path from 'node:path';

const here = import.meta.dirname;
const outDir = path.join(here, 'vendor', 'codemirror');
mkdirSync(outDir, { recursive: true });

await build({
  entryPoints: [path.join(here, 'vendor-entry.mjs')],
  outfile: path.join(outDir, 'codemirror.js'),
  bundle: true,
  format: 'iife',
  globalName: 'CM6',
  minify: true,
  target: 'es2020',
  legalComments: 'none',
});

console.log('vendor: wrote vendor/codemirror/codemirror.js');
