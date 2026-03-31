# Phase C: Docked View Drag + Snap

## Goal
Allow users to drag plugin docked-view panes and drop/snap them into new docked positions within the active tab’s split tree.

## Product Scope (v1)
- Draggable pane headers for `plugin_view` leaves.
- Snap targets inside active tab:
  - `left`, `right`, `top`, `bottom` (split-dock)
  - `center` (focus/attach as sibling replacement behavior is deferred; for v1, center means focus target pane and no tree rewrite).
- Reorder/move applies only within the active tab.
- No cross-window or cross-tab drag in v1.

## Non-goals
- Dragging terminal panes (can be added later with same engine).
- Floating panes.
- Persisting dragged layout across app restarts.

## Current State
- Plugin docked views are represented as split leaves with `kind: 'plugin_view'` in `frontend/index.html`.
- View identity and routing are already keyed by `view_id`.
- Backend tracks bindings in:
  - `views_by_id`
  - `pane_to_view`
- Frontend can create/focus/close plugin view panes and re-render view widgets.

## UX Contract
1. Drag starts from plugin view pane header.
2. While dragging, hovered candidate pane shows snap overlay with 4 edge zones (+ optional center zone).
3. Drop on edge rewrites split tree to move dragged pane relative to target pane.
4. Drop on invalid area cancels drag and keeps layout unchanged.
5. After successful drop:
   - moved pane stays focused,
   - widget state remains intact,
   - backend binding remains valid (same `view_id` and `pane_id`).

## Frontend Architecture Additions

### New module
Add `crates/conch_tauri/frontend/pane-dnd.js`.

Responsibilities:
- Drag session lifecycle (`begin`, `update`, `commit`, `cancel`).
- Hit-testing pointer -> target pane + snap zone.
- Overlay rendering for snap affordances.
- Request split-tree rewrite callback supplied by `index.html`.

### Pane model assumptions
Continue using existing pane records from `index.html`.
Required fields used by DnD:
- `paneId`
- `tabId`
- `kind`
- `root` (DOM element)

No backend state changes required for v1 drag/snap.

## Split-Tree Rewrite Rules
Input:
- `dragPaneId` (must be `plugin_view`)
- `targetPaneId`
- `zone in {left,right,top,bottom,center}`

Rules:
1. Reject if panes are in different tabs.
2. Reject if `dragPaneId == targetPaneId`.
3. For `center`: no rewrite, just focus `targetPaneId`.
4. For edge zones:
   - Remove dragged leaf from current location.
   - Insert dragged leaf around target leaf using split direction:
     - `left/right` => `vertical`
     - `top/bottom` => `horizontal`
   - Use placement mapping:
     - `left/top` => dragged before target
     - `right/bottom` => dragged after target
5. Normalize tree:
   - collapse single-child splits,
   - clamp split ratios.

## API Surface (Frontend Internal)

### `pane-dnd.js`
```js
initPaneDnd({
  getActiveTabId,
  getPaneById,
  getAllPanesInTab,
  movePaneByDrop,    // (dragPaneId, targetPaneId, zone) => boolean
  onFocusPane,       // (paneId) => void
});

registerDraggablePaneHeader(paneId, headerEl, paneKind);
unregisterPane(paneId);
```

### `index.html` integration points
- On plugin view creation (`openPluginDockedViewFromRequest`), register header drag handle.
- On pane close, unregister pane from DnD manager.
- In `rebuildTreeDOM`, ensure header references remain current.

## View-Scoped Routing Safety
Drag/snap must not alter view routing identity.
- `view_id` remains attached to pane record.
- No call to `register_plugin_view_binding` needed when only position changes.
- If future implementation changes pane IDs during move, then binding must be re-registered; v1 should preserve pane IDs.

## Accessibility / Input
- Keep mouse-first in v1.
- Cursor hints:
  - header: `grab` / `grabbing`
- Escape cancels active drag session.

## Failure Modes + Guardrails
- If split-tree rewrite fails: rollback visually and log warning.
- If target pane disappears mid-drag: cancel.
- If active tab changes mid-drag: cancel.

## Observability
Add debug logs behind existing key debug flag:
- drag start (`paneId`, `tabId`)
- hover target updates (`targetPaneId`, `zone`)
- commit result (`success/failure`)

## Phased Implementation

### Phase C1: DnD scaffolding + overlays
- Add `pane-dnd.js` with drag session and visual snap zones.
- Wire registration for plugin view headers.

### Phase C2: Tree rewrite move engine
- Implement `movePaneByDrop(...)` in `index.html` using `splitTree` helpers.
- Add normalization pass and focus restore.

### Phase C3: Cleanup + resiliency
- Handle pane close/unregister edge cases.
- Add debug logs and cancellation safeguards.

### Phase C4: QA + docs
- Add section to `docs/plugin-sdk.md` clarifying this is user-driven host UX (no new plugin API).
- Add manual test checklist.

## Manual Test Checklist
1. Open one docked plugin view, drag to each edge around a terminal pane.
2. Open two docked views; move one around the other and around terminal panes.
3. Trigger widget events after each move; confirm view still responds.
4. Close moved pane; ensure no orphan overlay/listeners.
5. Disable plugin with moved view present; existing cleanup still works.
6. Press Escape during drag; layout unchanged.

## Risks
- Tree corruption from invalid rewrite sequences.
  - Mitigation: strict preconditions + post-normalization + fallback cancel.
- Event listener leaks from pane lifecycle churn.
  - Mitigation: explicit `register/unregister` and teardown on close.
- Drag UX jitter.
  - Mitigation: throttled hover updates and stable target selection.

## Future Extensions (Post-v1)
- Cross-tab drag/snap.
- Terminal-pane drag support via same DnD engine.
- Keyboard docking commands.
- Persist dragged layout in state.
