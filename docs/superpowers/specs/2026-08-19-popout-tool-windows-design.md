# Pop-Out Tool Windows — Design

**Status:** Draft
**Date:** 2026-08-19
**Scope:** Any registered tool window — built-in (SFTP/file-explorer, Hosts/ssh-sessions, Tunnels) or plugin — can switch to "Window" view mode: its panel re-hosts in its own OS window with a dock-back affordance, IntelliJ-style. The capability is a platform trait carried by the tool-window registration contract, never per-panel opt-in.

## Product rules (settled in brainstorm, 2026-08-19)

1. **Platform trait:** if it registers with `toolWindowManager.register(id, …)`, it can pop out — plugin tool windows included, day one.
2. **View modes: Dock and Window only.** Float/always-on-top, pinned/unpinned auto-hide, drag-tear-off are out of scope (future features).
3. **The popped window's header carries a dock-back icon** (the IntelliJ affordance); dock-back returns the panel to the zone it came from.
4. **OS close = hide** (IntelliJ behavior): the tool window stays in Window mode; summoning it (rail icon, shortcut) re-shows the window. Dock-back is the only path back to the dock.
5. **Window mode and bounds persist** across restarts in the existing layout state; a Window-mode tool window that was visible reopens as a window at its saved bounds.

## Architecture

A pop-out is a fresh webview re-hosting the registered panel (webview DOM cannot move between windows — settings/chooser precedent). Rust owns window lifecycle via a registry; the popped webview boots `index.html` in a panel-host branch so every panel's dependencies exist by construction.

### Rust: `panel_host.rs`

- **Registry** (managed `Mutex`): keyed `(parent_label, tool_window_id)`; entries carry `req_id` (process-monotonic) and a request-unique `window_label` = `panelhost-<parent_label>-<req_id>` (unique labels are settled law: Tauri clears its label map only after the async destroy round-trips — same-label rebuild collides). All lookups by exact stored label; parent-derivation by label parsing is banned.
- **Commands:** `open_panel_host(window, tool_window_id, title)` (callers: main app windows only — reject `chooser-`/`settings`/`panelhost-` labels; open against a live same-key entry = cancel-and-recreate, never adopt); `get_panel_host_request()` (exact window-label match → `{ reqId, toolWindowId, parentLabel, title }` camelCase); `panel_host_ready()` (show + focus; built `.visible(false)` with `arm_window_show_fallback`); `focus_panel_host(tool_window_id)`; `dock_panel_host(tool_window_id)` (destroys the host window after emitting the re-dock event to the parent); `hide_panel_host` / show path via summon.
- **Lifecycle hooks:** the host window's `CloseRequested` is intercepted — prevent close, `hide()` the window, emit `panel-host-hidden {toolWindowId}` to the parent (rail state sync). The webview and panel state survive; summon = `show()+set_focus()`. Parent `Destroyed` destroys all its panel hosts (exactly-once through the registry; late resolvers no-op). Dock-back and displacement destroy (not close — destroy skips CloseRequested; settled law).
- **Sizing:** default 520×400 logical floor, persisted per-tool-window bounds override (clamped to floor and monitor work area), centered on parent on first open, never moved/resized after show by us; user resizes/moves freely and bounds save on move/resize (debounced through the existing layout save).
- **Session resolution:** `effective_session_window_label` generalizes to consult BOTH the chooser registry and the panel-host registry (one resolver, one truth). Every session-keyed command (`sftp_*`, ssh channel queries a panel makes, plugin host-API calls that are window-scoped) resolves a `panelhost-*` caller to its parent. `tauri_host_api`'s window-scoped fields (appearance/dark-mode noted in the terminal-themes review) resolve the parent likewise.

### The trait surface (frontend, `toolWindowManager`)

- Per-id view-mode state: `'dock' | 'window'`, default `'dock'`, stored beside the existing zone assignments.
- Every tool window's header (rendered by the MANAGER, not panels — this is what makes the trait inheritable) gains a View Mode submenu: `Dock ✓ / Window`. The implementer traces the existing header/gear affordance and extends it; if a tool window type has no header controls today, the manager adds the standard one.
- Choosing Window: panel unmounts from its zone (zone remembers the assignment for dock-back), `open_panel_host(id, title)` fires, rail icon stays lit while the window is visible.
- Rail-icon toggle routes by mode: dock → today's behavior; window → `focus_panel_host` when hidden/unfocused, hide when visible+focused (trace today's exact toggle semantics and mirror them).
- Dock-back (from the popped window's header, or by selecting Dock in the popped window's own View Mode menu): notify parent (`panel-host-docked` event) → parent remounts the panel into its remembered zone → Rust destroys the host window.

### The panel host boot (`index.html`, boot branch)

- No new HTML entry. `open_panel_host` builds the window with URL `index.html?toolWindowHost=<id>`. The startup runtime branches EARLY: panel-host boots skip terminal/PTY/tab-bar/titlebar-menu initialization entirely and instead: initialize config + appearance (`appearance.js` — the host follows app appearance like every window), data services, keyboard shortcut handling scoped to the host, the plugin-widget bridge, and toast surface; call `get_panel_host_request()`; render the host chrome (slim header: tool-window title, dock-back button); ask `toolWindowManager`'s registration table for the panel with the requested id and mount it full-height; `panel_host_ready()`. Unknown/missing id → self-close (a host with no panel must not linger; chooser precedent).
- The host window participates in `config-changed` and `tl-appearance-changed` exactly as settings/chooser do (`appearance-sync.js` or its equivalent wiring in the branch).

### The parent-state event bridge (closed list)

Panels consume (a) app-global Rust broadcasts — `config-changed`, transfer progress, plugin events — which reach the host for free; and (b) parent-window state, bridged explicitly:

- `active-pane-changed` — payload: the parent's focused pane identity (pane id, kind, session key if remote). Consumer: files-panel's `onTabChanged` (SFTP follows the parent's active tab).
- Anything else the per-panel dependency audit (plan Task 1) surfaces becomes a NAMED addition to this list in the spec via spec-sync; the bridge API refuses unlisted event names (a constant array, not a passthrough — an unbounded bridge is how this rots).

Mechanism: parent runtime calls `panel_host_broadcast(event, payload)` → Rust `emit_to`s every live host of that parent → host runtime re-dispatches to the mounted panel through the same callback interface the manager uses in-window.

### Persistence

Layout state (existing `tool_window_zones` / `active_tool_windows` neighborhood) gains per-tool-window `view_mode` and `window_bounds {x, y, width, height}` (logical px). Absent keys default to dock — old state files load unchanged. On launch, a Window-mode tool window that was visible at save time reopens as a window at its clamped bounds; hidden ones wait for summon. Plugin tool-window ids persist the same way (ids are stable strings).

## Non-Goals

- Float/always-on-top mode; pinned/unpinned auto-hide; Undock; Move-to-zone changes (exists or not, untouched).
- Drag-to-tear-off; menu-only in v1.
- Moving a panel between different MAIN windows; a pop-out belongs to the parent that opened it.
- Multi-panel tabs inside one popped window.
- Popping out the editor, terminal panes, or the settings/chooser surfaces (not tool windows).

## Constraints

- Branch `feat/popout-tool-windows`; CLAUDE.md rules (no main commits, no Co-Authored-By trailers, imperative commits, unit tests required).
- Zero behavior change while everything stays docked: old layout state files load identically; a fresh config renders today's UI byte-for-byte.
- Tokens-only CSS; boundary script's only allowed failure `tl-dialog.js:334`.
- Baselines at branch: 732 cargo tests, 33 frontend suites, token parity + extractor goldens green.
- Never git in `/Users/dustin/projects/TermLab` (read-only reference repo).

## Testing

- **Rust:** registry unit tests (exactly-once across racing exits, displacement cancel-and-recreate, hide-vs-destroy paths, parent-death drain, unique labels, caller validation); the generalized session resolver (chooser caller → parent, panel-host caller → parent, stale label → itself, both registries consulted); persistence round-trip of `view_mode`/`window_bounds` incl. absent-key defaults.
- **Frontend (vm suites):** boot branch (panel-host flag → no terminal init, one panel mounted, ready called after mount); View Mode menu appears for every registered id including a synthetic plugin registration (the trait test — register a fake tool window, assert it gets the menu with zero panel-side code); rail-toggle routing by mode; event-bridge re-dispatch (`active-pane-changed` reaches the mounted panel's callback); bridge refuses unlisted event names; dock-back round-trip remounts into the remembered zone.
- **Per-panel smoke:** each built-in panel mounts in the host harness without touching missing globals (the "just works" executable check).
- **Manual checklist section L:** pop out each built-in + one plugin tool window; dock back; close-hides then summon re-shows; restart persistence (window mode + bounds); two main windows with independent pop-outs of the same tool window id; `[SSH]` SFTP-in-window follows the parent's active tab and transfers work (session resolver in anger); appearance flip restyles popped windows live.

## Known limitations

- A popped panel's webview survives hide (state preserved) but not app restart (panel state resets; window mode/bounds persist).
- Keyboard shortcuts that act on "the current terminal pane" are inert inside a panel host (no panes there); shortcut routing scoped accordingly.
