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
    // A crash inside the vendor bundle reaches us as one line in the status
    // banner and one line in frontend.log, and with plain `minify: true` that
    // line reads `at Bl.scanTile` — or, worse, names nothing at all. Both
    // settings below exist to make that line legible; neither changes what the
    // bundle DOES.
    //
    //   keepNames: esbuild keeps every function and class NAME, so a stack
    //     frame says `InlineCoordsScan.scanTile` instead of `Bl.scanTile`.
    //     This is the one that survives everywhere — it is baked into the .js
    //     itself, so it works in the built app's WKWebView with no devtools
    //     attached and nothing else to serve.
    //   sourcemap 'linked': emits codemirror.js.map beside the bundle and adds
    //     the //# sourceMappingURL comment. OPT-IN, because tauri.conf.json's
    //     `frontendDist: "frontend"` embeds this whole directory in the app —
    //     the two maps are ~4.4MB that every installer would carry for a file
    //     only devtools can read, and the owner hits this bug in a webview
    //     with no devtools attached. Set TERMLAB_VENDOR_SOURCEMAP=1 before
    //     `npm run build:vendor` when you are debugging the bundle in a
    //     browser. keepNames is deliberately NOT behind the same switch: it is
    //     what makes the shipped stack readable.
    keepNames: true,
    sourcemap: process.env.TERMLAB_VENDOR_SOURCEMAP === '1' ? 'linked' : false,
    target: 'es2020',
    legalComments: 'none',
  });
  console.log(`vendor: wrote ${b.outDir.join('/')}/${b.out}`);
}
