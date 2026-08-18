// Run: node scripts/tests/test_window_size.mjs
//
// The measurement behind "windows open at the right size": pure arithmetic
// that turns what the terminal reports into the cell/chrome metrics Rust uses
// to size the NEXT window. Nothing here resizes anything — the previous
// design corrected a visible window after show, and every variant of it
// animated. The DOM measurement that feeds this is verified by using the app.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/core/window-size.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { metricsFor, rendererCellSize } = sandbox.termlabWindowSize;

// 80x24 in an 800x480 host inside a 840x560 window: 10x20 cells, 40x80 chrome.
{
  const m = metricsFor(
    { cols: 80, rows: 24, width: 800, height: 480 },
    { width: 840, height: 560 },
  );
  assert.ok(m, 'expected metrics');
  assert.strictEqual(m.cellWidth, 10, 'cellWidth');
  assert.strictEqual(m.cellHeight, 20, 'cellHeight');
  assert.strictEqual(m.chromeWidth, 40, 'chromeWidth');
  assert.strictEqual(m.chromeHeight, 80, 'chromeHeight');
}

// When the renderer's exact cell size is supplied it wins over division —
// division inflates the cell by the partial cell the host happens to hold.
{
  const m = metricsFor(
    { cols: 80, rows: 24, width: 805, height: 484, cellWidth: 10, cellHeight: 20 },
    { width: 845, height: 564 },
  );
  assert.strictEqual(m.cellWidth, 10, 'renderer cell width wins');
  assert.strictEqual(m.cellHeight, 20, 'renderer cell height wins');
  // Chrome accounts for the partial cell: 845 - 80*10 = 45.
  assert.strictEqual(m.chromeWidth, 45, 'chrome absorbs the partial cell');
  assert.strictEqual(m.chromeHeight, 84);
}

// Fractional cells survive rather than rounding to a lie.
{
  const m = metricsFor(
    { cols: 90, rows: 30, width: 864, height: 570 },
    { width: 900, height: 640 },
  );
  assert.ok(Math.abs(m.cellWidth - 9.6) < 1e-9, 'fractional cellWidth');
  assert.ok(Math.abs(m.cellHeight - 19) < 1e-9, 'fractional cellHeight');
}

// An unfitted terminal (0 cols) must yield null, never Infinity — persisting
// Infinity would make every future launch open at a garbage size.
assert.strictEqual(metricsFor({ cols: 0, rows: 24, width: 800, height: 480 }, { width: 840, height: 560 }), null);
assert.strictEqual(metricsFor({ cols: 80, rows: 0, width: 800, height: 480 }, { width: 840, height: 560 }), null);
assert.strictEqual(metricsFor({ cols: 80, rows: 24, width: 0, height: 480 }, { width: 840, height: 560 }), null);
assert.strictEqual(metricsFor({ cols: 80, rows: 24, width: 800, height: 480 }, { width: 0, height: 560 }), null);

// A terminal reported larger than its window is a mid-layout read, not a
// measurement.
assert.strictEqual(metricsFor({ cols: 80, rows: 24, width: 900, height: 480 }, { width: 840, height: 560 }), null);

// A renderer cell that would put the grid outside the window is refused too.
assert.strictEqual(
  metricsFor(
    { cols: 80, rows: 24, width: 800, height: 480, cellWidth: 11, cellHeight: 20 },
    { width: 840, height: 560 },
  ),
  null,
  '80 x 11px = 880 grid in an 840 window is not a measurement',
);

// Garbage in does not throw.
assert.strictEqual(metricsFor(null, { width: 840, height: 560 }), null);
assert.strictEqual(metricsFor({ cols: 80, rows: 24, width: 800, height: 480 }, null), null);

// rendererCellSize: reads xterm's private dimensions when present, null on
// anything else — it must never throw on a missing or reshaped internal.
{
  const term = {
    _core: { _renderService: { dimensions: { css: { cell: { width: 8.4, height: 20 } } } } },
  };
  // Field-by-field: deepStrictEqual fails on vm-sandbox objects (cross-realm
  // prototypes).
  const cell = rendererCellSize(term);
  assert.strictEqual(cell.width, 8.4);
  assert.strictEqual(cell.height, 20);
  assert.strictEqual(rendererCellSize({}), null);
  assert.strictEqual(rendererCellSize(null), null);
  assert.strictEqual(
    rendererCellSize({ _core: { _renderService: { dimensions: { css: { cell: { width: 0, height: 20 } } } } } }),
    null,
    'a zero cell is not a measurement',
  );
}

console.log('window size arithmetic: all assertions passed');
