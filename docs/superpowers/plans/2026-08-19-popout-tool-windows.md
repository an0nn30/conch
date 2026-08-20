# Pop-Out Tool Windows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Any registered tool window — built-in or plugin — can switch to Window view mode: its panel re-hosts in its own OS window with a dock-back header, hide-on-close, and persisted mode+bounds.

**Architecture:** A Rust `PanelHostRegistry` (the chooser-registry pattern, but persistent windows with hide/show lifecycle) owns host windows labeled `panelhost-<parent>-<reqId>` loading `index.html`; the boot branches on the label prefix and mounts exactly one registered panel. The tool-window manager renders the View Mode menu entries for every registration (the inheritable trait), routes the rail toggle by mode, and remounts on dock-back. One shared session resolver consults both the chooser and panel-host registries.

**Tech Stack:** Tauri 2.10.3 (WebviewWindowBuilder, emit_to, managed state), IIFE frontend, vm-sandbox `.mjs` suites.

**Spec:** `docs/superpowers/specs/2026-08-19-popout-tool-windows-design.md` (amendments from scouting, applied in Task 6's spec-sync but binding NOW: label-prefix boot instead of a query flag; View Mode as two flattened `tl-menu` entries, not a submenu; window bounds persisted Rust-side from window events).

## Global Constraints

- Branch `feat/popout-tool-windows`; never commit to main; NO Co-Authored-By trailers; imperative commits; unit tests required (CLAUDE.md). Never git in `/Users/dustin/projects/TermLab`.
- Zero behavior change while everything stays docked; old layout state files load identically (`#[serde(default)]` discipline).
- Unique window labels; destroy-not-close for teardown paths; hide via intercepted CloseRequested for the user-close path; exactly-once registry resolution — all settled law from the chooser branch.
- Tokens-only CSS; boundary script's only allowed failure `tl-dialog.js:334`.
- Baselines: 732 cargo, 33 frontend suites, parity + extractor goldens green. Branch base main@38aa5e2.
- The event bridge accepts ONLY names in its exported constant list (`['active-pane-changed']` at birth); unlisted names throw.

## File Structure

- `crates/termlab_tauri/src/panel_host.rs` — NEW: registry, request/lifecycle commands, builder, bounds persistence, broadcast relay.
- `crates/termlab_tauri/src/window_registry_resolver.rs` — NEW (small): the shared caller→parent session resolver over both registries; `chooser_window.rs` and `remote/sftp_commands.rs` + `plugins/tauri_host_api.rs` rewire to it.
- `crates/termlab_core/src/config/persistent.rs` — MODIFY: `LayoutConfig` gains `tool_window_view_modes: HashMap<String,String>` + `tool_window_bounds: HashMap<String, WindowBoundsRecord>`; `crates/termlab_tauri/src/commands.rs` payload plumbing for view modes (bounds are Rust-written, not JS payload).
- `frontend/app/layout/tool-window-manager.js` — MODIFY: view-mode state, menu entries, toggle routing, dock-back remount, `getViewModes()` getter, `getRegistration(id)`.
- `frontend/app/tool-window-runtime.js` — MODIFY: registrations-only init mode; layout payload gains `tool_window_view_modes`.
- `frontend/app/panel-host-runtime.js` — NEW: the host boot (chrome, mount, bridge re-dispatch, ready); `frontend/app/main-runtime.js` — MODIFY: label-prefix branch.
- `frontend/styles/design-system/components/toolwindow.css` — MODIFY: host chrome styles (tokens only).
- Tests: `#[cfg(test)]` in the two new Rust modules + persistent.rs; NEW `scripts/tests/test_panel_host.mjs`; MODIFY `test_tool_window_*` suites as they exist (trace).

---

### Task 1: `panel_host.rs` — registry, commands, builder, lifecycle, bounds

**Files:** Create `crates/termlab_tauri/src/panel_host.rs`; modify `crates/termlab_tauri/src/lib.rs` (manage state, register commands, window-event hooks beside the chooser's).

**Interfaces (later tasks rely on exact names):**
- Commands: `open_panel_host(window, tool_window_id: String, title: String) -> Result<u64, String>` (main-window callers only — reject labels starting `chooser-`/`panelhost-` and the exact label `settings`; open against a live `(parent, tool_window_id)` = cancel-and-recreate); `get_panel_host_request(window) -> Result<PanelHostRequest, String>` (exact window-label match; serialized camelCase `{reqId, toolWindowId, parentLabel, title}`); `panel_host_ready(window)`; `focus_panel_host(window, tool_window_id)`; `hide_panel_host(window, tool_window_id)`; `dock_panel_host(window, tool_window_id)` (emit `panel-host-docked {toolWindowId}` to parent, then DESTROY); `panel_host_broadcast(window, event: String, payload: serde_json::Value)` (parent-callers only; `emit_to` every live host of that parent as `panel-host-event {event, payload}`).
- Events to parent: `panel-host-docked {toolWindowId}`, `panel-host-hidden {toolWindowId}`, `panel-host-shown {toolWindowId}`.
- Builder: `WebviewUrl::App("index.html".into())`, label `panelhost-<parent>-<req_id>`, title from arg, `.visible(false)` + `arm_window_show_fallback`, `.theme()` per settings-window rules, min 360×240, inner size = persisted bounds for this tool_window_id (clamped ≥ min, ≤ work area) else 520×400, positioned at persisted x/y (clamped on-screen) else centered on parent. Menu removed.
- Lifecycle hooks in lib.rs: `CloseRequested` on a `panelhost-*` label → prevent close, `hide()`, emit `panel-host-hidden` to parent (registry entry SURVIVES — this is the one registry whose entries outlive window visibility); parent `Destroyed` → destroy all its hosts + drain entries; host `Destroyed` without registry entry → no-op (re-entry law); host `Moved`/`Resized` → debounce-persist bounds for its tool_window_id (load-mutate-save `PersistentState`, logical px — this task adds `tool_window_bounds` reads/writes but the STRUCT lands in Task 2; to keep this task self-contained, Task 1 defines the struct+field in persistent.rs and Task 2 only adds view_modes — adjust: **Task 1 owns `tool_window_bounds` + `WindowBoundsRecord {x,y,width,height}: f64` in persistent.rs**, Task 2 owns `tool_window_view_modes`).

Registry semantics (pure struct, unit-testable): `open(parent, tool_window_id, title)` mints req_id + unique label, returns displaced entry (if any) for the command layer to destroy first; `get_by_window_label`; `hosts_of_parent`; `remove(parent, tool_window_id)`; hide/show mutate a `visible` flag on the entry (summon path: `focus_panel_host` shows+focuses if entry exists, else returns `Err("no host")` so the frontend falls back to `open_panel_host`).

- [ ] Registry unit tests first (failing): unique labels + fresh req_ids across opens; displacement returns old entry and inserts fresh; per-parent drain leaves other parents; exactly-once remove; visible-flag transitions; caller validation (`panelhost-*` cannot open hosts). Bounds: `WindowBoundsRecord` round-trips persistent.rs TOML incl. absent-key default (empty map).
- [ ] Implement; `cargo test --workspace` (732 + new); clippy clean on new files. Commit: `git commit -m "Add the panel host registry, commands, and window lifecycle"`

### Task 2: Shared session resolver + view-mode persistence

**Files:** Create `crates/termlab_tauri/src/window_registry_resolver.rs`; modify `chooser_window.rs` (delegate its `effective_session_window_label`), `remote/sftp_commands.rs:session_caller_label` (call the shared fn), `plugins/tauri_host_api.rs` (window-scoped fields resolve parent for `panelhost-*` callers), `crates/termlab_core/src/config/persistent.rs` (+`tool_window_view_modes: HashMap<String,String>` with serde default), `crates/termlab_tauri/src/commands.rs` (save_window_layout payload gains optional `tool_window_view_modes`, merged only when `Some` — mirror `tool_window_zones` at :239-240/:344-348).

**Interfaces:** `window_registry_resolver::effective_session_window_label(app, caller_label) -> String` — checks the chooser registry, then the panel-host registry, else identity. Frontend layout payload key: `tool_window_view_modes` (id → `"dock"|"window"`).

- [ ] Failing tests: resolver matrix (chooser caller→parent, panel-host caller→parent, main window→itself, stale labels of both prefixes→themselves — registry-level tests plus one app-handle-level smoke); view_modes TOML round-trip + absent default; save_window_layout merge-only-when-Some.
- [ ] Implement; verify the old chooser resolver call sites all route through the shared fn (grep `effective_session_window_label` — one definition). Full cargo + clippy. Commit: `git commit -m "Share the caller-to-parent session resolver and persist view modes"`

### Task 3: The trait surface in the manager

**Files:** Modify `frontend/app/layout/tool-window-manager.js` (view-mode state, menu entries, toggle routing, dock-back remount, `getViewModes()`, `getRegistration(id)`, restore-from-layout), `frontend/app/tool-window-runtime.js` (layout payload + restore plumbing, summon/reopen wiring), `frontend/app/manager-compose-runtime.js` only if delegate wiring requires (trace). Test: NEW `scripts/tests/test_panel_host.mjs` part 1 (manager half) — follow the vm-harness idiom of `test_settings_terminal_theme_picker.mjs`.

**Interfaces consumed:** Task 1 commands/events. **Produces:** manager APIs `setViewMode(id, mode)`, `getViewModes()`, `getRegistration(id) -> {id, title, icon, type, renderFn}` (Task 4 mounts via this).

Mechanics (from scouting — trace and cite): the gear menu built in `showContextMenu` (`tool-window-manager.js:861-898` via `tlMenu.open`) gains TWO flattened entries for the targeted id: `View Mode: Dock` (checked when dock) and `View Mode: Window` (checked when window) — tl-menu has NO submenu support; do not add any. Selecting Window: remember the zone assignment, detach the panel element from its zone (the manager keeps the registration; `renderFn` is render-once so the HOST re-renders fresh — do NOT try to move DOM), `invoke('open_panel_host', {toolWindowId, title})`, mark mode. Rail toggle (`toggle()` at `:299-312`): window-mode branch — entry visible per last known state → `hide_panel_host`; else `focus_panel_host`, falling back to `open_panel_host` on `Err` (window was never opened this session / app relaunch). Listen for `panel-host-hidden/shown/docked` to sync rail lit-state and, on docked, remount: re-activate the id in its remembered zone (a fresh `renderFn` render — same lazy path `ensureWindowElement` uses). Restore: `tool_window_view_modes` from saved layout seeds the mode map; window-mode tool windows that were active at save are summoned at startup (after registrations complete), others wait.

- [ ] Failing checks: menu entries appear for EVERY registered id including a synthetic plugin registration (`register('plugin:fake', {renderFn})` → menu carries both View Mode entries — the trait test); selecting Window invokes `open_panel_host` with id+title and detaches from zone; toggle routing matrix (window+visible→hide, window+hidden→focus, focus-Err→open, dock→legacy behavior byte-identical — pin by asserting the legacy path functions are called unchanged); docked event remounts into the REMEMBERED zone; view modes appear in the save payload; restore seeds modes and summons previously-visible window-mode ids.
- [ ] Implement; full frontend suite; `node --check`. Commit: `git commit -m "Give every tool window a View Mode with a window host route"`

### Task 4: The panel host boot

**Files:** Create `frontend/app/panel-host-runtime.js`; modify `frontend/app/main-runtime.js` (the branch), `frontend/app/tool-window-runtime.js` (registrations-only init mode), `frontend/styles/design-system/components/toolwindow.css` (host chrome), `frontend/index.html` (script tag for the new runtime, before main-runtime). Test: `test_panel_host.mjs` part 2.

**Mechanics (from scouting):** `main-runtime.js` branches AFTER `currentWindowLabel = await invoke('current_window_label')` (`main-runtime.js:56`) and BEFORE `termlabComposeRuntime.create` (`:108`): if the label starts with `panelhost-`, hand off to `window.termlabPanelHostRuntime.boot({invoke, listen, listenOnCurrentWindow, currentWindow, tauriClient})` and return — no compose, no event-wiring, no `createTab`, no shortcut-runtime table (scoped keys only: Escape does nothing; the keyboard-router is loaded but only host-chrome handlers register). `boot()`: `get_panel_host_request()` (failure → self-close via `currentWindow.close()` — CloseRequested-hide only applies to REGISTERED hosts; an unregistered window's close proceeds… verify: the hide-hook looks up the registry and lets close proceed when no entry exists — Task 1 must have implemented it that way; if not, fix here and note); apply appearance + theme css (the same calls settings.html's boot makes — `applyAppearance`, `applyThemeCss`, `termlabAppearanceSync` wiring); `pluginWidgets.init({invoke, listen})` (NO terminal callbacks — the `if (opts.xxx)` guards make plugin tab/pty actions no-ops, scout-verified); run tool-window-runtime's registrations-only mode (new export: `registerAll({registrationsOnly: true})` — registers the four built-ins + replays `get_plugin_panels` + subscribes `plugin-panel-registered/removed`, but builds NO zones/strips/layout); render host chrome (slim header: title from request, dock-back button invoking `dock_panel_host`, styled `.tl-panelhost__*` tokens-only); `getRegistration(request.toolWindowId)` → mount `renderFn` into the content root (unknown id → self-close); subscribe `panel-host-event` and re-dispatch (Task 5 wires consumers); `panel_host_ready()`.

- [ ] Failing checks (vm harness): label-prefix branch takes the host path and never touches compose/tab globals (stub them as throwing canaries); unknown-id self-close; chrome renders title + dock-back wired to `dock_panel_host`; the requested registration's renderFn receives the content root; ready called after mount; plugin-widgets initialized without terminal callbacks.
- [ ] Implement; full frontend suite + boundary; `node --check`. Commit: `git commit -m "Boot panel host windows from the main entry by label prefix"`

### Task 5: The parent-state event bridge

**Files:** Modify `frontend/app/manager-compose-runtime.js` (the two `filesPanel.onTabChanged` call sites at `:57` and `:143` ALSO publish to the bridge), NEW small module or manager addition `frontend/app/core/panel-host-bridge.js` (exported constant `BRIDGE_EVENTS = ['active-pane-changed']`; `publish(event, payload)` throws on unlisted names, else `invoke('panel_host_broadcast', …)` fire-and-forget), `frontend/app/panel-host-runtime.js` (re-dispatch: `active-pane-changed` → `filesPanel.onTabChanged(payload)`). Test: `test_panel_host.mjs` part 3.

**Payload contract (scout-verified duck-type `files-panel.js:177-213`):** `{type: 'local'|'ssh', spawned: bool, paneId, focusedPaneId, id}` — serialize the fields the parent's pane/tab object carries; the panel reads `type`, `spawned`, and one of the three ids. Session-key fields resolve against the PARENT thanks to Task 2's resolver (the panel host invokes sftp commands directly).

- [ ] Failing checks: publish of a listed event invokes the relay with the exact payload; unlisted name THROWS synchronously; host re-dispatch calls `filesPanel.onTabChanged` with the payload; the two parent call sites publish (harness: stub bridge, trigger the delegate callbacks, assert).
- [ ] Implement; full suites. Commit: `git commit -m "Bridge parent pane state to panel host windows"`

### Task 6: Sweep, checklist section L, spec sync

**Files:** checklist note (section L after K's last step, 132), spec, any loose ends.

- [ ] Sweep: full cargo + frontend + boundary + parity/goldens; greps — no `toolWindowHost=` query remnants; `panelhost-` label construction only in `panel_host.rs`; bridge constant is the single event-name source (no stray literals).
- [ ] Checklist L (steps 133+, house voice, `[SSH]` tags, bold judgment call-outs): pop out each built-in; pop out a plugin tool window (the trait in anger); dock back returns to the remembered zone; OS-close hides + rail summon re-shows same state (scroll preserved); restart persistence (mode + bounds, incl. off-screen-monitor clamp); two main windows, independent pop-outs of the same id; `[SSH]` SFTP-in-window follows the parent's active tab and uploads/downloads work; appearance flip restyles hosts live; **judgment: default 520×400 size and header density**.
- [ ] Spec sync: the three scouting amendments (label-prefix boot, flattened menu entries, Rust-side bounds persistence) plus anything execution changed; every cited identifier grep-verified (the phantom-citation lesson).
- [ ] Commit: `git commit -m "Add pop-out tool windows checklist section and sync the spec"`
