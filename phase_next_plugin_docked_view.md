# Phase: Next Plugin Docked View

## Goal
Enable plugins to create dockable split leaves inside terminal tabs (for example, terminal on top + plugin form/monitor on bottom) using the existing split pane system, without PTY content in plugin leaves.

## Non-goals
- Replacing existing left/right/bottom plugin panels.
- Reworking the widget system or introducing a second renderer.
- Breaking existing plugin APIs.

## Current Architecture Summary
- Split tree is generic and can host any leaf keyed by `paneId`.
  - Frontend: `crates/termlab_tauri/frontend/split-tree.js`, `split-pane.js`.
- Pane lifecycle is currently terminal-centric in `frontend/index.html`.
- Plugin panels are currently side-panel/bottom-panel surfaces, keyed mostly by plugin name + handle.
  - Backend metadata: `crates/termlab_tauri/src/plugins/mod.rs` (`PanelInfo`).
  - Widget updates/event routing: `frontend/plugin-widgets.js`.
- Permission-gated host API is centralized via `HostApi` + `PermissionCheckedHostApi`.

## High-level Design
Unify split leaves under two kinds:
1. `terminal` leaf (existing local/ssh behavior)
2. `plugin_view` leaf (widget-rendered, no PTY)

This keeps one split engine and one focus/navigation model.

## Permissions
Add capability:
- `ui.dock` — allows plugin to create/close/focus docked plugin views in split trees.

Existing `ui.panel` remains for side/bottom static panel registration.

## API Signatures (Host API)
These are additive and must be parity-implemented for Lua + Java + Rust trait.

### Rust trait (`termlab_plugin::HostApi`)
```rust
fn open_docked_view(&self, req_json: &str) -> Option<String>;
fn close_docked_view(&self, view_id: &str) -> bool;
fn focus_docked_view(&self, view_id: &str) -> bool;
```

`req_json`/result JSON are explicit envelopes to keep JNI/Lua bridging simple.

### Request/Response schema (JSON)
`open_docked_view` request:
```json
{
  "id": "optional-stable-id",
  "title": "Resource Monitor",
  "icon": "activity",
  "target": {
    "scope": "active_tab"
  },
  "dock": {
    "relative_to": "active_pane",
    "direction": "horizontal",
    "placement": "after",
    "ratio": 0.35
  },
  "render": {
    "mode": "view"
  }
}
```

`open_docked_view` response:
```json
{
  "view_id": "plugin:resource-monitor:view:1",
  "pane_id": 42,
  "tab_id": 7
}
```

Notes:
- `direction`: split direction in current tree semantics (`vertical` side-by-side, `horizontal` stacked).
- `ratio`: fraction for new view leaf.
- `render.mode=view` signals view-scoped rendering path.

### Java (`java-sdk/src/termlab/plugin/HostApi.java`)
```java
public static native String openDockedView(String requestJson);
public static native boolean closeDockedView(String viewId);
public static native boolean focusDockedView(String viewId);
```

### Lua (`app`/`ui` table additions)
```lua
-- returns table { view_id = "...", pane_id = number, tab_id = number } or nil
ui.open_docked_view(opts)

-- returns true/false
ui.close_docked_view(view_id)
ui.focus_docked_view(view_id)
```

## Plugin Runtime Hooks
Add optional per-view render hook (fallback safe):

### Lua
```lua
function render_view(view_id)
  -- return widgets for that specific view
end
```
Fallback to existing `render()` when `render_view` missing.

### Java (`TermLabPlugin` default method)
```java
default String renderView(String viewId) {
    return render();
}
```

No breaking change to existing plugins.

## View-scoped Routing (Core Concern + Solution)

### Problem
Current panel/widget flow is mostly plugin-name scoped. If a plugin has multiple docked views, update/render/event routing becomes ambiguous.

### Solution
Introduce `view_id` as first-class routing key end-to-end.

#### New backend state
Add map(s) in plugin state:
- `views_by_id: HashMap<String, PluginViewInfo>`
- `pane_to_view: HashMap<u32, String>`

`PluginViewInfo` fields:
- `view_id: String`
- `plugin_name: String`
- `pane_id: u32`
- `tab_scope/meta`
- `title`, `icon`
- `last_widgets_json: String`

#### New frontend event payloads
- `plugin-view-opened`:
  - `{ view_id, plugin, pane_id, tab_id, title, icon, dock }`
- `plugin-view-closed`:
  - `{ view_id, plugin, pane_id, tab_id }`
- `plugin-view-widgets-updated`:
  - `{ view_id, plugin, widgets_json }`

#### Render request command
Add command:
- `request_plugin_view_render(plugin_name, view_id) -> Option<String>`

Backend dispatch:
- send `RenderRequest::View { view_id, reply }` to plugin runtime.
- runtime calls `render_view(view_id)` if implemented; otherwise `render()`.

#### Widget event envelope
Current frontend sends:
```json
{ "kind": "widget", "type": "...", ... }
```
Extend to include optional context:
```json
{ "kind": "widget", "view_id": "...", "type": "...", ... }
```

Backend forwards unchanged JSON; plugin can disambiguate by `view_id`.

#### Runtime compatibility
- Existing plugins ignore `view_id` and continue working.
- New plugins consume `view_id` for per-view state.

## Frontend Model Changes
Refactor pane record in `frontend/index.html`:
- `kind: 'terminal' | 'plugin_view'`
- terminal-only fields (`term`, `fitAddon`, `spawned`, `connectionId`) only present for `terminal`.
- plugin-view fields: `viewId`, `pluginName`, `widgetContainerEl`.

Behavior rules:
- Split/focus/close/navigation apply to both kinds.
- PTY resize/write/input handlers only for `terminal` kind.
- Context menu can expose split/close for both; terminal-only actions guarded.

## Lifecycle Rules
- Plugin unload/disable:
  - close all its docked views
  - remove leaves from split tree
  - move focus to nearest surviving leaf
- Closing a plugin_view leaf:
  - notify backend (`close_docked_view`) to cleanup maps
- Last leaf in tab:
  - same existing tab-close behavior

## Persistence
Phase 1 can keep docked views ephemeral.
Phase 2 optional persistence:
- store per-tab docked plugin view descriptors in persistent layout state.
- restore views only for enabled plugins on startup.

## Phased Implementation Plan

### Phase A: Data model + permissions
- Add `ui.dock` to capability allowlist and permission wrapper.
- Introduce `PluginViewInfo` state and ID helpers.

### Phase B: Host API plumbing
- Extend `HostApi` trait.
- Implement in `TauriHostApi`.
- Wire JNI natives + Java SDK methods.
- Wire Lua API wrappers.

### Phase C: Runtime render/event routing
- Add `RenderRequest::View` mailbox variant.
- Add Lua `render_view` dispatch fallback.
- Add Java `renderView` default-path dispatch fallback.

### Phase D: Frontend mixed-leaf support
- Refactor pane model to support `plugin_view` leaves.
- Handle open/close/update view events.
- Render widget container inside plugin leaf and bind via `plugin-widgets.js`.

### Phase E: UX hardening
- Focus restoration invariants.
- Shortcut/menu behavior with plugin leaf focused.
- Add explicit “Close Plugin View” action.

### Phase F: Docs/examples/tests
- Update `docs/plugin-sdk.md` with new signatures and JSON contracts.
- Add one Lua and one Java example for docked bottom monitor view.
- Add tests for multi-view routing correctness.

## Testing Checklist
- Open two docked views from same plugin; verify independent rendering.
- Send widget events from each view; plugin receives correct `view_id`.
- Split/close around plugin views and terminal panes; no orphaned tree nodes.
- Disable plugin while views open; cleanup and focus recovery are correct.
- Existing panel plugins still register/render unchanged.
- Existing action plugins unaffected.
- Java and Lua parity: same capabilities and semantics.

## Risks and Mitigations
- Risk: plugin-name scoped routing collisions.
  - Mitigation: enforce `view_id` routing for all docked view commands/events.
- Risk: terminal-specific assumptions in pane code.
  - Mitigation: explicit `kind` guards for all PTY operations.
- Risk: API drift between Lua and Java.
  - Mitigation: single canonical docs table + parity tests in CI.

## Locked Product Decisions
1. Docked views are created in the active tab only (workstation-per-tab model).
2. Duplicate plugin-provided IDs auto-dedupe (focus existing view instead of creating another).
3. No docked-view persistence across app restarts in v1 (session-only).
4. Users can minimize or close plugin views even if plugin treats them as primary UI.

## Implementation Defaults
- Scope: active tab only.
- Duplicate IDs: dedupe by `(plugin, id)` within tab; focus existing view.
- Persistence: session-only.
- User close/minimize: always allowed; plugin may reopen via command/menu.
