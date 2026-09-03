// Bundles third-party frontend dependencies into a single IIFE global.
//
// The app's own modules are plain IIFE <script> files and are deliberately NOT
// built — this exists only so CodeMirror 6, which is ESM-only, can be consumed
// by a frontend with no module system.
import { build } from 'esbuild';
import { mkdirSync } from 'node:fs';
import path from 'node:path';

const here = import.meta.dirname;

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
