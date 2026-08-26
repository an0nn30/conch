// Run: node scripts/tests/test_tool_zone_markup.mjs
//
// Regression test: a tool zone that never hosts a window must start marked
// `empty`, because updateZone() — the only code that manages that class — runs
// only for zones a window actually touches. The bug: index.html pre-marked
// only the *-bottom zones, so dragging a tool window into left-bottom while
// left-top had never been swept left a stale non-empty left-top rendering as
// an 80px blank block (`.tool-zone:not(.empty) { min-height: 80px }`) above
// the dropped window's header. The boot markup must satisfy the invariant
// "empty class <=> zone has no active window" for EVERY zone; registration
// and activation clear the class for the zones that do receive windows.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';

const INDEX_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/index.html',
);

const html = fs.readFileSync(INDEX_PATH, 'utf8');
const zoneTags = html.match(/<div[^>]*class="[^"]*tool-zone[^"]*"[^>]*>/g) || [];

assert.ok(zoneTags.length >= 5, `expected at least 5 tool zones in index.html, found ${zoneTags.length}`);

for (const tag of zoneTags) {
  const zoneName = (tag.match(/data-zone="([^"]+)"/) || [])[1] || '(unnamed)';
  const classes = ((tag.match(/class="([^"]*)"/) || [])[1] || '').split(/\s+/);
  assert.ok(
    classes.includes('empty'),
    `tool zone "${zoneName}" must boot with the empty class; a zone no window ever touches is never swept by updateZone()`,
  );
}

console.log(`tool zone markup: all ${zoneTags.length} zones boot empty`);

// The bottom zone was split into a left/right pair (bottom-left,
// bottom-right) so it can host two tool windows side by side, mirroring the
// left/right sidebars' top/bottom pairs. The old single `data-zone="bottom"`
// zone must be fully retired from the markup — normalizeZoneName() in
// tool-window-manager.js still *aliases* the legacy name for persisted
// state, but the DOM itself must never contain it again.
assert.ok(
  !html.includes('data-zone="bottom"'),
  'index.html must not contain a lone data-zone="bottom" zone; it was split into bottom-left/bottom-right',
);

for (const zoneName of ['bottom-left', 'bottom-right']) {
  const matches = html.match(new RegExp(`data-zone="${zoneName}"`, 'g')) || [];
  assert.equal(
    matches.length,
    1,
    `expected exactly one data-zone="${zoneName}" in index.html, found ${matches.length}`,
  );

  const zoneStart = html.indexOf(`data-zone="${zoneName}"`);
  const zoneBlock = html.slice(zoneStart, html.indexOf('</div>', zoneStart) + '</div>'.length + 200);
  assert.ok(
    /zone-tab-strip/.test(zoneBlock),
    `data-zone="${zoneName}" must be immediately followed by a .zone-tab-strip child`,
  );
}

const dividerMatches = html.match(/id="bottom-zone-divider"/g) || [];
assert.equal(
  dividerMatches.length,
  1,
  `expected exactly one id="bottom-zone-divider" in index.html, found ${dividerMatches.length}`,
);

console.log('tool zone markup: bottom-left/bottom-right pair and divider present');
