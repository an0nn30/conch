# Composite Panel Host (SFTP + Transfers pop-out) — Design

**Date:** 2026-08-26
**Status:** Approved in chat (spec pending user review)
**Owner request:** Popping out the SFTP tool window should produce a window
that contains BOTH the SFTP and Transfers tool windows — Transfers as a
bottom panel, SFTP as the main content — so it reads as its own complete
app. Docking returns both to their original locations in the main window.

## Summary

Tool-window registrations gain a declarative `companions` option. When a
tool with companions is popped out, its panel-host window mounts the main
tool plus each companion in a bottom section behind a draggable divider,
and the parent window *suppresses* the companions (they leave their zones;
their strip buttons show an "away" state that focuses the pop-out).
Suppression is runtime-only state derived from the live host — nothing new
is persisted — so dock-back, abort, and app restart all restore the
companions to their original zones for free. One consumer today:
`file-explorer` declares `companions: [{ id: 'transfer-center', position:
'bottom' }]`.

## Current state (what this builds on)

- Pop-out: `enterWindowMode(tw)` detaches the tool from its zone, sets
  `viewModes` to `'window'`, and calls `open_panel_host { toolWindowId,
  title }` (tool-window-manager.js:575). Rust (`panel_host.rs`) keeps a
  registry keyed by `(parent_label, tool_window_id)` and builds a fresh
  webview window; the host document boots via `panel-host-runtime.js`:
  `get_panel_host_request` → `registerAll({ registrationsOnly: true })` →
  `getRegistration(request.toolWindowId)` → `buildChrome(title, dock…)` →
  `mountRegistration(contentRootEl, registration)`.
- Dock-back: parent-initiated `dockFromWindowMode(id)` (mode flip →
  `dock_panel_host` → `activate(id)`), or host-initiated Dock button →
  `dock_panel_host` → `panel-host-docked` event → `notifyHostDocked(id,
  reqId)` in the parent. `panel-host-aborted` resets the mode without a
  remount. `hide_panel_host` hides the host without docking.
- Restart: `tool_window_view_modes` persists `'window'`; registration
  restore queues `pendingWindowSummons` and re-summons the host.

## Design

### Registration: `companions`

`register(id, opts)` accepts `companions: [{ id: string, position:
'bottom' }]` (only `'bottom'` is defined; unknown positions are treated as
`'bottom'`). Stored on the registration (`tw.companions`, default `[]`).
Only `file-explorer` declares one today, in tool-window-runtime.js:

```js
companions: [{ id: 'transfer-center', position: 'bottom' }],
```

A companion id that is never registered, or equals the host tool's own id,
is ignored at use time (composite degrades to today's solo host).

### Parent-side suppression

New runtime state in tool-window-manager.js (nothing persisted):

```js
// companionId → { hostId, wasActive } while a companion rides inside a
// live composite host. pendingCompanionSuppressions covers registration
// order on boot: the host tool can register (and summon its host) before
// the companion's own register() call arrives.
const companionSuppressions = new Map();
const pendingCompanionSuppressions = new Map(); // companionId → hostId
```

**Suppress** (per companion of a host tool): record `wasActive =
tw.active`; `deactivate`-style removal from its zone (active flag off,
element hidden, `zone.activeId` cleared if it was the active window — the
zone re-flows exactly as if the user closed it); strip button stays but
renders "away" (below). A companion that is itself in window mode at
suppress time is first docked via the existing `dockFromWindowMode(id)`
(its solo host closes), then suppressed.

**Lift** (per companion): remove from the maps; if `wasActive` and its
zone has no other active window, `activate(companionId)`; otherwise leave
it closed. Zone assignment was never touched, so "original location" is
inherent.

**When suppression applies:**
- `enterWindowMode(hostTw)` — suppress each companion before
  `open_panel_host` (so the main window re-flows in the same gesture).
  If `open_panel_host` rejects, the existing failure path
  (`dockFromWindowMode(id, { teardownHost: false })`) also lifts.
- `summonWindowHost(hostId)`'s open-fallback (post-restart rebuild) —
  same suppression before the open call.
- Registration restore: when the host tool registers in window mode with
  a live/pending summon, its companions enter
  `pendingCompanionSuppressions`; `register()` of a companion checks the
  map and suppresses immediately instead of mounting into its zone.

**When suppression lifts:**
- `dockFromWindowMode(hostId)` (parent-initiated dock, and the open-failure
  path).
- `notifyHostDocked(hostId, reqId)` (host Dock button), after its
  existing generation guard passes.
- The `panel-host-aborted` path (`resetToDock`-reachable host teardown).
- `unregister(hostId)`.

**NOT lifted** on `hideWindowHost(hostId)` — a hidden host still owns its
companions; summoning it back must not have to re-suppress.

**Companion strip button ("away" state):** a suppressed companion's strip
button renders with a new `strip-btn--away` class (dimmed, matching the
`.twm-strip-dragging` opacity idiom) and never `active`. Clicking it calls
`focus_panel_host` for the HOST tool's id (falling back to
`summonWindowHost(hostId)` if that rejects) instead of toggling the dock
panel. Context-menu actions on a suppressed companion (`moveTo`, view
mode) are inert while suppressed except `Hide`, which is also inert — the
menu still opens but those entries are disabled.

### Host request plumbing (additive)

- `open_panel_host` gains `companionIds: string[]` (frontend camelCase →
  serde rename as the existing fields do). `PanelHostRequest` and the
  registry entry carry `companion_ids: Vec<String>` with
  `#[serde(default)]`; `get_panel_host_request` returns it. Old callers
  and old persisted state need no changes.
- The parent computes the list at open time: registration `companions`
  filtered to ids with a live registration, minus the host tool's own id.

### Host window layout

`panel-host-runtime.js` `boot()`: after resolving the main registration,
resolve each `request.companion_ids` through `getRegistration`. With zero
resolvable companions the flow is byte-for-byte today's. Otherwise:

```
body
  chrome rootEl (title bar + Dock button, unchanged — one Dock docks all)
    contentRootEl            (main tool mount — SFTP)
  .panel-host-bottom
    .panel-host-divider      (row-resize drag, zone-divider idiom)
    .panel-host-bottom-content  (companion mount — Transfers)
```

- Both mounts go through the existing `mountRegistration`; all disposers
  run on `beforeunload` exactly like the single mount today.
- Divider: pointer-drag adjusts the bottom section height, clamp 120px to
  70% of the window, default 35%. Session-only — not persisted.
- The bottom section exists whenever companions mounted; there is no
  collapse/close affordance in v1 (the window IS the composite).
- Host default window size: when companions are present the parent passes
  the existing size logic a taller default (+40%, capped to screen) —
  implemented wherever `open_panel_host` sizes the window today; if sizing
  is host-side, the host applies it before showing.

### Rust changes (additive only)

`panel_host.rs`: `companion_ids: Vec<String>` on the open-command args,
`PanelHostEntry`, and `PanelHostRequest` (all `#[serde(default)]`).
Validation: ids are opaque strings; the host resolves them against its own
registrations, so Rust does no filtering beyond passing them through.
No event payload changes — dock/hide/abort still name only the host tool id.

### Edge cases

- Companion already popped out solo when the host pops out: parent docks
  the companion's solo host first (existing `dockFromWindowMode`), then
  suppresses it into the composite.
- Popping out the companion alone (SFTP stays docked): unchanged solo
  behavior — `companions` is read only from the tool being popped out.
- Host tool popped out, user quits: view mode `'window'` persists for the
  host tool only; restart re-summons the composite and re-suppresses via
  `pendingCompanionSuppressions`. If the saved layout marks the host tool
  closed-while-popped-out, no host opens and no suppression applies until
  the user summons it from the rail.
- Companion unregistered (plugin removed) mid-composite: `unregister`
  drops it from the suppression maps; the host window keeps its dead mount
  until next dock (acceptable; plugins can't be companions today).
- The SFTP↔Transfers pair plus the transfer-center's auto-open behavior:
  transfer auto-open (`features/transfers/auto-open.js`) calls
  `toolWindowManager.activate('transfer-center')` on a new transfer. The
  guard lives in `activate()` itself: an early return for any suppressed
  id, so every activation path (auto-open, palette, menu) is covered in
  one place. The composite host always shows Transfers anyway, which
  satisfies the intent.

### Files touched

- `crates/termlab_tauri/frontend/app/layout/tool-window-manager.js` —
  `companions` registration option, suppression state + apply/lift sites,
  strip "away" rendering/click routing, companion guards in context menu.
- `crates/termlab_tauri/frontend/app/tool-window-runtime.js` —
  `file-explorer` registration declares the companion; auto-open guard.
- `crates/termlab_tauri/frontend/app/panel-host-runtime.js` — composite
  boot layout, divider, multi-mount/dispose.
- `crates/termlab_tauri/src/panel_host.rs` — `companion_ids` plumbing,
  taller default size when companions present.
- `crates/termlab_tauri/frontend/styles/panels.css` (owns the existing
  panel-host chrome styles) — `.panel-host-bottom`, `.panel-host-divider`.
- `crates/termlab_tauri/frontend/styles/tool-windows.css` —
  `.strip-btn--away`.

### Out of scope

- More than one companion per host, or positions other than `'bottom'`
  (the shape allows them; nothing implements them).
- Persisting the composite divider ratio.
- Companion declarations for plugins.
- A separate dock/close affordance for the companion inside the host.

## Testing

- **Manager (VM, extend the panel-host/manager suites):** suppress on
  enterWindowMode (companion active → zone re-flows, wasActive recorded;
  companion closed → stays closed on lift); lift on dockFromWindowMode,
  notifyHostDocked, and abort; hide does NOT lift; solo-popped companion
  gets docked-then-suppressed; registration-order restore via
  `pendingCompanionSuppressions`; strip button renders `strip-btn--away`
  and routes click to host focus; open_panel_host receives the filtered
  `companionIds`.
- **Host boot (test_panel_host.mjs real-manager harness):** composite
  request mounts two registrations with the divider present; both
  disposers run on unload; unresolvable companion id degrades to solo.
- **Rust:** serde round-trip for `companion_ids` default + presence on
  the request/entry structs.
- **Manual:** pop out SFTP → transfers leaves the main bottom zone and
  appears in the host; drag divider; start a transfer (progress visible in
  host); dock → both return, prior active/closed state preserved; restart
  while popped out → composite restored; hide host then summon from rail.
