// Run: node scripts/tests/test_dock_highlight.mjs
//
// IntelliJ-style drag guides: while dragging a tool-window button, the only
// visual is a flat translucent region over the REAL dock area the hovered
// zone maps to. This suite pins the pure zone→rect math; the manager feeds
// it live layout metrics.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/layout/dock-highlight.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { computeDockHighlightRect } = sandbox.termlabDockHighlight;
const plain = (x) => JSON.parse(JSON.stringify(x));

// Window: main area 1000x600 at (0, 40); strips 24px; panels 280/300 wide;
// bottom bar hidden, default height 312.
const M = {
  main: { left: 0, top: 40, width: 1000, height: 600 },
  bottomRect: null,
  leftStripWidth: 24,
  rightStripWidth: 24,
  leftPanelWidth: 280,
  rightPanelWidth: 300,
  bottomHeight: 312,
};

// --- left band: full height next to the strip, split into halves -------------
assert.deepStrictEqual(
  plain(computeDockHighlightRect('left-top', M)),
  { left: 24, top: 40, width: 280, height: 300 },
  'left-top is the top half of the left band',
);
assert.deepStrictEqual(
  plain(computeDockHighlightRect('left-bottom', M)),
  { left: 24, top: 340, width: 280, height: 300 },
  'left-bottom is the bottom half',
);

// --- right band hugs the right strip -----------------------------------------
assert.deepStrictEqual(
  plain(computeDockHighlightRect('right-top', M)),
  { left: 1000 - 24 - 300, top: 40, width: 300, height: 300 },
);
assert.deepStrictEqual(
  plain(computeDockHighlightRect('right-bottom', M)),
  { left: 676, top: 340, width: 300, height: 300 },
);

// --- bottom band: hidden bar falls back to default height over the main area --
assert.deepStrictEqual(
  plain(computeDockHighlightRect('bottom-left', M)),
  { left: 0, top: 40 + 600 - 312, width: 500, height: 312 },
  'bottom-left is the left half of the fallback band',
);
assert.deepStrictEqual(
  plain(computeDockHighlightRect('bottom-right', M)),
  { left: 500, top: 328, width: 500, height: 312 },
);

// --- a visible bottom bar wins over the fallback ------------------------------
{
  const withBar = { ...M, bottomRect: { left: 0, top: 560, width: 1000, height: 200 } };
  assert.deepStrictEqual(
    plain(computeDockHighlightRect('bottom-right', withBar)),
    { left: 500, top: 560, width: 500, height: 200 },
    'live bottom bar rect drives the band',
  );
}

// --- junk zone → null ---------------------------------------------------------
assert.strictEqual(computeDockHighlightRect('attic', M), null);

console.log('test_dock_highlight: all assertions passed');
