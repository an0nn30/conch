# Bottom Zone Split — Design

**Date:** 2026-08-26
**Status:** Approved (design reviewed in chat; spec pending user review)
**Owner request:** IntelliJ-style bottom panel with left and right sections:
tools mount to bottom-left or bottom-right, and two tools can be open side by
side in the bottom bar.

## Summary

Replace the single special-cased `bottom` tool-window zone with a
`bottom-left` / `bottom-right` pair that behaves exactly like the existing
`left-top`/`left-bottom` and `right-top`/`right-bottom` sidebar pairs, rotated
90°: one active window per zone, a draggable divider when both zones hold an
open window, full width for a lone window, and IntelliJ-style split ends on
the horizontal strip. The design removes the "bottom is special" case from the
tool-window manager rather than adding a new concept.

## Current state (what changes)

- `tool-window-manager.js` runs five zones:
  `['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom']`.
  Sidebars pair two zones with `updateSidebar(side)` (divider when both
  active, lone zone takes all space, remembered ratio in
  `lastSplitRatios[side]`) and `initZoneDivider(side)` (vertical drag).
  The bottom is a single zone handled by `updateBottomZone()` and is
  special-cased in ~a dozen places (`sideForZone`, `panelSideHasWindows`,
  strip rendering, dock targets, context menu).
- Markup (`index.html`): `#bottom-zone-wrap` holds one
  `.tool-zone[data-zone="bottom"]` plus the `#bottom-zone-resize` height
  handle; `#bottom-strip` is the horizontal rail.
- Persistence (`commands.rs` / termlab_core state): `tool_window_zones`
  (window id → zone name, opaque strings), `active_zone_windows`
  (zone name → active window id, `""` = zone closed), `SplitRatios { left,
  right }` ↔ `left_split_ratio` / `right_split_ratio`,
  `bottom_panel_visible`, `bottom_panel_height`.
- Registrations defaulting to the bottom: `file-explorer`,
  `transfer-center` (`defaultZone: 'bottom'`), plugin panels with
  `location: 'bottom'`.

## Design

### Zones

`ZONE_IDS` becomes
`['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right']`.
`sideForZone('bottom-left' | 'bottom-right')` → `'bottom'`. Every site that
special-cases the `bottom` zone name generalizes to the pair; sites keyed on
the *side* `'bottom'` (panel visibility, height resize, zen hiding, toggle
shortcut) are unchanged.

### Layout & divider

`#bottom-zone-wrap` becomes:

```
#bottom-zone-wrap
  #bottom-zone-resize            (existing height drag handle, unchanged)
  .bottom-zone-row               (horizontal flex row)
    .tool-zone[data-zone="bottom-left"]
    #bottom-zone-divider         (vertical divider, col-resize)
    .tool-zone[data-zone="bottom-right"]
```

`updateBottomZone()` adopts the sidebar pair logic:

- Bar visible iff `panelState.bottom.visible` and at least one of the two
  zones has an active window (or a popped-out summonable one, matching
  today's behavior for the single zone).
- Divider visible only when **both** zones have an active window.
- One active zone → `flex: 1`, the other `flex: 0` (lone tool takes the full
  width).
- Both active → restore `lastSplitRatios.bottom` (default 0.5).

`initZoneDivider` is generalized: for `left`/`right` it drags on `clientY`
between `-top`/`-bottom` zones as today; for `bottom` it drags on `clientX`
between `bottom-left`/`bottom-right` with the same 0.15–0.85 clamp,
`lastSplitRatios.bottom` update, terminal refit during drag, and
`triggerSave()` on release. The height handle (`#bottom-zone-resize`) and
`bottom_panel_height` persistence are untouched — both sections share one bar
height, like IntelliJ.

### Strip (split ends)

The bottom strip renders two `strip-section`s justified to opposite ends
(`justify-content: space-between` on `#bottom-strip`): left end =
`bottom-left` windows, right end = `bottom-right` windows — the horizontal
analogue of the vertical strips' top/bottom sections. The strip hides only
when *both* zones have no windows. `makeStripBtn` is reused unchanged (it
already takes the zone).

### Moving tools

- Context menu targets become: Left (Top), Left (Bottom), Right (Top),
  Right (Bottom), **Bottom (Left)**, **Bottom (Right)** — the single
  "Bottom" entry is replaced by the pair.
- Drag-docking: the dock-target rect computation replaces the one full-width
  bottom rect with two half-width rects (left half → `bottom-left`, right
  half → `bottom-right`), same vertical band as today.
- `moveTo(windowId, zone)` already works by zone name; no changes beyond the
  new names being valid.

### Compatibility & migration

`'bottom'` remains accepted as an input alias for `'bottom-left'` forever:

- `normalizeZoneName(name)`: `'bottom'` → `'bottom-left'`; unknown names fall
  back to the caller's default (current behavior for bad persisted values).
- Applied at every zone-name entry point: `register()` (`defaultZone`),
  persisted `tool_window_zones` values, plugin `location` mapping, and
  `moveTo()`.
- Persisted `active_zone_windows`: a `'bottom'` key is read as
  `'bottom-left'` on load; saves emit only the new keys. `getActiveZoneAssignments`
  iterates the new `ZONE_IDS`, so the closed-zone `""` marker semantics carry
  over per section.
- First boot after upgrade: every tool that lived in `bottom` appears in
  `bottom-left` with the same active/closed state; `bottom-right` starts
  empty. No visual change until the user moves something right.

### Rust persistence (additive only)

- `SplitRatios { left, right }` gains `bottom: Option<f64>`.
- The persisted state struct gains `bottom_split_ratio` with
  `#[serde(default = ...)]` (default 0.5); `LoadedLayout` returns it.
- `tool_window_zones` / `active_zone_windows` need no schema change (opaque
  string maps). Old `state.toml` files load unchanged; new fields default.

### Files touched

- `crates/termlab_tauri/frontend/app/layout/tool-window-manager.js` — zone
  list, `normalizeZoneName`, `updateBottomZone` pair logic, generalized
  `initZoneDivider`, strip sections, context menu, dock targets,
  `getSplitRatios`/`setSplitRatio` bottom arm.
- `crates/termlab_tauri/frontend/index.html` — bottom zone pair markup.
- `crates/termlab_tauri/frontend/styles/tool-windows.css` — owns
  `.bottom-zone-wrap` and `.zone-divider` today; gains `.bottom-zone-row`,
  the vertical divider variant, and strip `space-between`.
- `crates/termlab_tauri/frontend/app/tool-window-runtime.js` — untouched:
  `defaultZone: 'bottom'` registrations and plugin `location: 'bottom'`
  keep working through the alias; no registration churn.
- `crates/termlab_tauri/src/commands.rs` — `SplitRatios.bottom`,
  `bottom_split_ratio` field plumbing.
- `crates/termlab_core` state struct (wherever `left_split_ratio` lives) —
  additive `bottom_split_ratio` with serde default.

### Out of scope

- Per-section heights (IntelliJ shares one bar height; so do we).
- More than one docked window visible per zone (unchanged: a zone shows its
  active window; its other windows are strip/tab entries).
- Left/right sidebar behavior — untouched.

## Testing

- **Rust:** state round-trip tests for `bottom_split_ratio` default +
  backward compat (old state.toml without the field loads).
- **Node (new suite `test_bottom_zone_split.mjs`):**
  - `normalizeZoneName` aliasing (`'bottom'` → `'bottom-left'`, pass-through
    for the rest, fallback for junk).
  - Loading persisted assignments containing `'bottom'` zone values and a
    `'bottom'` key in `active_zone_windows` lands windows in `bottom-left`
    with active/closed state preserved.
  - Pair logic for the bottom: divider hidden with one active window, lone
    window gets full width, both active restores ratio, bar hides when both
    empty.
  - Strip sectioning: windows split into left/right ends by zone.
  - Context menu contains both bottom targets and marks the current one.
- **Existing suites** (`test_tool_zone_markup`, `test_tool_window_closed_state`,
  `test_panel_host`) updated where they reference the `bottom` zone id.
- **Manual:** drag a tool between bottom sections; divider drag; height
  resize; restart restores layout; zen mode hides the bar; popped-out window
  in a bottom section survives restart.
