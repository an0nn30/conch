// Run: node scripts/tests/test_xterm_linefeed_patch.mjs
//
// Guards the TermLab patch applied to vendor/xterm/xterm.min.js: upstream
// 5.5.0's InputHandler.lineFeed dereferences the cursor's buffer line
// unguarded, and after a rapid resize while a fullscreen app is redrawing
// the line store can momentarily be shorter than ybase+y. The throw escapes
// from inside xterm's write loop and wedges the terminal (no rendering, no
// input) until the app restarts. This test recreates that exact buffer
// state and proves lineFeed survives it. If the vendor file is ever
// re-vendored without the patch, this fails.

import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import path from 'node:path';

const require = createRequire(import.meta.url);
const XTERM_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/vendor/xterm/xterm.min.js',
);

const { Terminal } = require(XTERM_PATH);
assert.ok(Terminal, 'vendored xterm must still export Terminal');

const term = new Terminal({ cols: 80, rows: 24, allowProposedApi: true });
const core = term._core;
assert.ok(core, 'xterm core internals must be reachable for this regression test');
const inputHandler = core._inputHandler;
const buffer = core.buffers && core.buffers.active;
assert.ok(inputHandler && typeof inputHandler.lineFeed === 'function', 'lineFeed must exist');
assert.ok(buffer && buffer.lines, 'active buffer line store must exist');

// The corrupt state: cursor mid-screen, but the line the cursor lands on is
// missing from the store — exactly what the resize race leaves behind.
buffer.y = 2;
const originalGet = buffer.lines.get.bind(buffer.lines);
buffer.lines.get = (index) => (index === buffer.ybase + 3 ? undefined : originalGet(index));
assert.equal(
  buffer.lines.get(buffer.ybase + 3),
  undefined,
  'test precondition: the line below the cursor must be missing',
);

assert.doesNotThrow(
  () => inputHandler.lineFeed(),
  'lineFeed must tolerate a missing buffer line instead of wedging the write loop',
);
assert.equal(buffer.y, 3, 'the cursor still advances');

// A healthy line store still gets its wrap flag cleared — the patch must
// not disable the normal behaviour.
const healthy = new Terminal({ cols: 80, rows: 24, allowProposedApi: true });
const healthyBuffer = healthy._core.buffers.active;
const healthyHandler = healthy._core._inputHandler;
healthyBuffer.y = 0;
const nextLine = healthyBuffer.lines.get(healthyBuffer.ybase + 1);
nextLine.isWrapped = true;
healthyHandler.lineFeed();
assert.equal(nextLine.isWrapped, false, 'lineFeed still clears isWrapped on the reached line');

console.log('xterm lineFeed patch: all assertions passed');
