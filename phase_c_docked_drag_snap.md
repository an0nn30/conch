# Phase C: Docked Drag + Snap (Branch Status + Next Plan)

## Goal
Enable users to reposition docked plugin views and tool windows by dragging their tabs/headers, with clear snap hints before drop.

## Scope Clarification
There are two related drag flows:
- Split-tree docked panes (`plugin_view` leaves inside the active tab canvas).
- Tool-window tabs (left/right side strip windows managed by `tool-window-manager.js`).

This branch has partial completion for split-tree pane drag, and no tab-drag support yet for tool windows.

## Branch Reality (as of 2026-04-01)

### Implemented now: split-tree pane drag (partial)
- `crates/conch_tauri/frontend/pane-dnd.js` exists and is wired in `index.html`.
- Drag handle registration is active for plugin-view pane headers (`registerDraggablePaneHeader` is called from `openPluginDockedViewFromRequest`).
- Pointer drag session exists with:
  - movement threshold,
  - Escape-to-cancel,
  - cancel on active-tab switch,
  - debug logging under key debug mode.
- Visual overlay exists (`.pane-dnd-overlay`, `.pane-dnd-zone`, `.pane-dnd-label`).
- `movePaneByDrop(...)` exists in `index.html` and rewrites split trees by removing the dragged leaf and reinserting by zone.

### Current behavior limitations
- Hit testing is canvas-level only: drop target is the active tab container rectangle, not a hovered target pane.
- `findDropTarget(...)` currently always returns `paneId: null`.
- Edge drops (`left/right/top/bottom`) currently insert against the full tree root, not around a specific hovered pane.
- `center` drop currently focuses the dragged pane (`dragPaneId`) and does not reattach/focus target.
- There is no multi-target snap guide system (only one overlay over the tab canvas).

### Tool-window tab drag status
- `tool-window-manager.js` supports `moveTo(id, zone)`.
- Re-zone is currently available only through context menu (`showContextMenu`), not drag.
- No drag affordance or snap guide overlay exists for strip tabs/tool-window tabs.

## Updated UX Contract

### A) Split-tree plugin-view pane drag
1. Drag starts from plugin-view pane header.
2. Hovering over a candidate pane shows snap guides for `left/right/top/bottom/center` relative to that candidate pane.
3. Edge drop moves dragged pane around the hovered target pane.
4. Center drop keeps layout unchanged and focuses target pane.
5. Invalid drop cancels and leaves layout unchanged.

### B) Tool-window tab drag
1. Drag starts from tool-window tab/strip button.
2. App shows zone guides (`left-top`, `left-bottom`, `right-top`, `right-bottom`; optional `bottom` later).
3. Dropping on a zone reassigns that tool window to the zone and activates it.
4. Invalid drop cancels without changing assignment.

## Plan: Next Implementation

### Phase C-Next-1: True target-pane snapping for split-tree drag
- Update `pane-dnd.js` hit testing:
  - Resolve hovered pane element via `[data-pane-id]` inside active tab container.
  - Exclude dragged pane element from candidate matching.
  - Return `{ paneId, rect, zone }` using hovered pane rect.
- Keep fallback behavior for empty tab canvas (root-level drop) only when no leaf candidate exists.
- Update `movePaneByDrop(...)` contract:
  - Require valid `targetPaneId` for pane-relative edge drops.
  - Preserve existing cross-tab and self-target guards.
  - `center` should focus `targetPaneId` (not dragged pane).

### Phase C-Next-2: Snap hints and guides polish
- Enhance overlay rendering to show:
  - candidate pane bounds,
  - highlighted snap zone,
  - textual hint (`Dock left`, etc.).
- Add light hysteresis/stability so the hover target does not flicker when pointer moves across dividers.
- Keep animation transitions already present in CSS, but bias for clarity over flourish.

### Phase C-Next-3: Tool-window tab drag with zone guides
- Add drag support in `tool-window-manager.js` for strip buttons (and any future header tabs):
  - pointerdown/pointermove/pointerup gesture (custom; no native HTML5 DnD).
  - capture dragged window id and current zone.
- Add tool-window drop guide overlay:
  - four visible zone targets mapped to manager zones,
  - active target highlight + label,
  - cancellation on Escape.
- On commit:
  - call `moveTo(windowId, targetZone)`,
  - keep activation behavior consistent,
  - trigger existing persisted layout save path.

### Phase C-Next-4: Validation and docs
- Manual checks:
  1. Drag plugin-view pane around terminal panes and other plugin panes (all 5 zones).
  2. Verify center drop focuses target pane only.
  3. Drag side-strip tool windows between top/bottom and left/right sides.
  4. Verify saved layout captures new zone assignments.
  5. Escape cancel leaves both split tree and tool-window zones unchanged.
- Update `docs/plugin-sdk.md` only if external plugin API behavior changes (currently expected: no API change).

## Risks
- Tree rewrite mistakes when moving around nested splits.
  - Mitigation: strict preconditions + unit-like helper tests for `insertAroundLeaf` path.
- Hover jitter over thin split dividers.
  - Mitigation: nearest-pane fallback + short stability window.
- Event/listener leaks from pane and tool-window lifecycle churn.
  - Mitigation: explicit register/unregister cleanup paths and drag session teardown.

## Out of Scope for this phase
- Cross-tab drag.
- Cross-window drag.
- Terminal-pane drag by header.
- Persisting split-tree drag layout beyond current session (tool-window layout persistence already exists).
