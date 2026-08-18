// Run: node scripts/tests/test_window_size.mjs
//
// The cell→pixel arithmetic behind the "default window size in columns × lines"
// setting. Pure, so it is tested directly; the DOM measurement that feeds it is
// verified by using the app (no jsdom in this repo — see test_tl_dialog.mjs).
import assert from 'node:assert';

// deepStrictEqual compares prototypes, and objects built inside the vm sandbox
// carry that realm's Object — so a structurally identical result fails. Compare
// the fields instead, which also gives a clearer message on failure.
function assertDelta(actual, dw, dh, what) {
  assert.ok(actual, `${what}: expected a delta, got ${actual}`);
  assert.strictEqual(actual.dw, dw, `${what}: dw`);
  assert.strictEqual(actual.dh, dh, `${what}: dh`);
}
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

const { sizeDelta } = sandbox.termlabWindowSize;

// A terminal measuring 80x24 in an 800x480 box has 10x20 cells. Growing it to
// 102x46 needs 22 more columns and 22 more rows.
const current = { cols: 80, rows: 24, width: 800, height: 480 };
// 10x20 cells: 102.5 cells wide = 1025px (from 800), 46.5 tall = 930px (from 480).
assertDelta(sizeDelta(current, { cols: 102, rows: 46 }), 225, 450, 'grow to 102x46');

// Shrinking yields negatives rather than clamping to zero.
assertDelta(sizeDelta(current, { cols: 60, rows: 20 }), -195, -70, 'shrink');

// Already the right size: no movement, but still a delta rather than null, so
// callers can distinguish "nothing to do" from "cannot compute".
assertDelta(sizeDelta(current, { cols: 80, rows: 24 }), 0, 0, 'already correct');

// Fractional cell sizes round rather than accumulating error.
assertDelta(
  sizeDelta({ cols: 80, rows: 24, width: 803, height: 484 }, { cols: 81, rows: 25 }),
  15, 30, 'fractional cells round',
);

// The bug this bias exists for: one column short, with a fractional cell size.
// Aiming at the boundary yields a delta that floors back to the same count, so
// the loop sticks. The delta must clear the boundary instead.
const oneShort = sizeDelta({ cols: 89, rows: 30, width: 748, height: 528 }, { cols: 90, rows: 30 });
assert.ok(oneShort.dw >= Math.ceil(748 / 89), 'must add at least a full cell to gain a column');

// 0 columns/lines is the documented "leave the window alone" escape hatch.
assert.strictEqual(sizeDelta(current, { cols: 0, rows: 46 }), null);
assert.strictEqual(sizeDelta(current, { cols: 102, rows: 0 }), null);
assert.strictEqual(sizeDelta(current, { cols: 0, rows: 0 }), null);

// An unfitted terminal reports 0 cols; dividing by it would produce Infinity
// and resize the window off-screen, so these must refuse instead.
assert.strictEqual(sizeDelta({ cols: 0, rows: 24, width: 800, height: 480 }, { cols: 102, rows: 46 }), null);
assert.strictEqual(sizeDelta({ cols: 80, rows: 0, width: 800, height: 480 }, { cols: 102, rows: 46 }), null);
assert.strictEqual(sizeDelta({ cols: 80, rows: 24, width: 0, height: 480 }, { cols: 102, rows: 46 }), null);
assert.strictEqual(sizeDelta({ cols: 80, rows: 24, width: 800, height: 0 }, { cols: 102, rows: 46 }), null);

// Garbage in does not throw.
assert.strictEqual(sizeDelta(null, { cols: 102, rows: 46 }), null);
assert.strictEqual(sizeDelta(current, null), null);
assert.strictEqual(sizeDelta({}, {}), null);

// --- metricsFor: the measurement persisted for the next launch ---

const { metricsFor } = sandbox.termlabWindowSize;

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

// Garbage in does not throw.
assert.strictEqual(metricsFor(null, { width: 840, height: 560 }), null);
assert.strictEqual(metricsFor({ cols: 80, rows: 24, width: 800, height: 480 }, null), null);

console.log('window size arithmetic: all assertions passed');
