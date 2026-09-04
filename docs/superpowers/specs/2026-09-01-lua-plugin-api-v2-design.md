# Lua Plugin API v2 — Core Design

**Status:** approved; step 1 of 6 implemented (see Sequencing)
**Date:** 2026-09-01
**Scope:** the plugin core — rendering, data bridge, events, packaging, config

## Why

The plugin surface is wide but shallow. Roughly 34 `ui.panel_*` functions exist,
yet a plugin cannot express "my state changed, redraw me," cannot observe the
terminal it lives in, and cannot read a file or call an API without being
granted arbitrary shell execution.

Four findings drove this design:

1. **No reactive model.** `ui.request_render()` does not call `render()` — it
   flushes whatever is in the widget accumulator. Widget events do not mark a
   plugin dirty (`push_tool_window_render` is only called from
   `handle_bus_event`). The frontend compensates with a 120 ms race: send the
   event, wait, and pull a render if no push arrived.
2. **Plugins are blind to the terminal.** `PluginEvent` has six variants, and
   the only host-published bus event in the app is `host.tick`, a 1 Hz clock.
   Nothing reports a tab opening, a session connecting, or focus moving.
3. **The security model is defeated by its own escape hatches.** No filesystem
   or HTTP API exists, so anything real routes through `session.exec` — the one
   capability equivalent to full code execution. Separately, the `html` widget
   assigned plugin markup to `innerHTML` with no CSP, which let any plugin
   holding `ui.panel` reach the Tauri bridge. (Fixed separately on
   `fix/plugin-widget-html-xss`.)
4. **The data bridge is unsafe and duplicated.** Events reach Lua by generating
   Lua source and `load()`ing it. Object keys escape `"` but not `\`, so a
   plugin publishing an event with a key ending in a backslash injects
   arbitrary Lua into every subscriber's VM. Two more hand-rolled converters
   live in `ui.rs`.

## Goals

- A core that makes tool-window plugins genuinely good, designed so later lanes
  (terminal automation, HTTP/filesystem, remote/SSH) are additive rather than a
  second rewrite.
- Composable, testable plugin code: `render` as a pure function of state.
- One data bridge, one metadata format, one event mechanism.
- Capability enforcement that is not routinely bypassed by design.

## Non-goals

Terminal output events and interception, HTTP, filesystem access, and the
remote/SSH surface are all explicitly deferred. They are the reason the core is
shaped this way, but none of them ship in it.

## Compatibility

**Clean break.** The v1 Lua API is removed, not shimmed. Nobody outside this
repo has shipped a plugin. The in-repo plugins and examples are migrated as part
of the work. Host API version becomes `2.0`; plugins declare `api = "^2.0"`.

---

## 1. Render & invalidation

### Lua shape

`render` returns a widget table. No accumulator, no hidden global state, no
side effects.

```lua
local state = { count = 0, filter = "" }

function render(ctx)
  return ui.vstack {
    ui.heading("Counter"),
    ui.label(("count = %d"):format(state.count)),
    ui.text_input {
      value = state.filter,
      hint = "filter…",
      on_change = function(v) state.filter = v end,
    },
    ui.button {
      text = "Increment",
      on_click = function() state.count = state.count + 1 end,
    },
  }
end
```

Because constructors return values, `local function row(item) return ui.hstack{…} end`
composes. That is the property that lets plugin authors build their own
abstractions instead of waiting for new `panel_*` functions.

`ctx` carries `surface` (`"panel"`, `"view"`, `"settings"`), `view_id`, and the
current theme. This replaces the separate `render_view` entry point.

### Callbacks

Handlers are Lua functions on the widget. During serialization the host walks
the tree; each function-valued field is stored in a per-surface callback
registry under a generated id of the form `"g<generation>:<counter>"`, and the
id is emitted in the JSON. Incoming widget events are resolved through that
registry.

The generation prefix is load-bearing: a click on a button that no longer exists
after a re-render arrives with a stale id and is dropped with a debug log,
rather than being misrouted to whichever handler now occupies that slot.

### The loop

Each surface has a dirty flag, set two ways:

- **Automatically**, after any event callback returns. The common case is
  correct without ceremony.
- **Explicitly**, via `ui.refresh()`, for updates not originating in a callback
  — timers, events, and later async results.

`ui.refresh()` sets the flag and posts `PluginMail::Invalidate` to the plugin's
own mailbox (the runner holds a clone of its sender in app data). The runner
drains the mailbox and, if the flag is set, clears it and renders once — so a
burst of invalidations collapses into a single render. No polling thread, and it
fits the existing blocking-mailbox runner without restructuring it.

Refresh called from inside `render` is permitted but capped: after 3 consecutive
render-triggering-render cycles the runner logs a warning and stops.

Transport is unchanged: `set_widgets` → `plugin-widgets-updated` → frontend.

### Reconciliation

The frontend reconciles instead of tearing down. Same widget type at the same
position updates in place; otherwise the node is replaced. No keyed algorithm
initially — a `key` field is added when reorderable tables and trees need it.

This deletes `captureFocusState` / `restoreFocusState`, which exist only to
survive `container.innerHTML = ''` on every repaint. `request_focus` remains,
but becomes a deliberate "focus this on open" rather than damage control.

Reconciliation lives in JS because it owns DOM nodes. That requires a JS test
harness, which the repo did not have; **vitest + jsdom** were added in
`fix/plugin-widget-html-xss` (merged in #107). Removing the `html` widget in
the widget-schema step also removes that branch's sanitizer along with it.

### Removed

The widget accumulator and all `ui.panel_*` functions, `render_view`,
`ui.request_render`, `push_tool_window_render`, the 120 ms pull fallback in
`sendEvent`, and the focus capture/restore pair.

---

## 2. Data bridge & widget schema

### Two paths, deliberately separate

**Data** — bus payloads, config values, service args and results, dialog
payloads — goes through mlua's serde integration in both directions. This
deletes all four hand-rolled converters: `json_value_to_lua_literal` in
`runner.rs` (the injection vector), `json_to_lua_value` / `lua_value_to_json`
in `ui.rs`, and `set_lua_table_from_json_map` in `session.rs`.

The fourth was found during planning, not when this spec was written. It
copied only strings, booleans and numbers, so nested objects and arrays were
silently dropped from `session.current()` and `session.exec_active()` — a data
loss bug nobody had noticed.

**Widgets** use a typed walker, because they carry Lua functions that serde
cannot represent. `ui.button{…}` returns a table tagged `__widget = "button"`;
the walker converts tagged tables into `Widget` variants, recurses through
`children`, and hoists function fields into the callback registry.

The widget path is typed and validating; the data path is generic. Keeping them
separate is the point.

### Serde conventions

These are pinned explicitly and tested. The behaviors below were verified
against mlua 0.10 with a characterization spike rather than assumed:

| Case | Behavior | Source |
|---|---|---|
| Empty Lua table `{}` | Serializes to `{}` (object) | mlua default |
| Lua `nil` in a table | Key absent from output, not `null` | mlua default |
| JSON `null` → Lua | `nil` | requires `serialize_unit_to_null(false)` and `serialize_none_to_null(false)`; the default is a lightuserdata sentinel that is not `nil` |
| Integer-valued number | Preserved as integer (`math.type` reports `integer`) | mlua default |
| Array tables | No metatable attached | requires `set_array_metatable(false)` |
| Lua function in a payload | Error: `unsupported value type 'function'` | mlua default; v1 silently emitted `null` |
| Recursive table | Error: `recursive table detected` | mlua default; v1 recursed until the stack overflowed |
| Sparse array `{[1]='a',[3]='c'}` | Truncates to `["a"]` at the first gap | mlua default; documented and tested as a known edge |

The two error cases are improvements: both are plugin bugs that previously
produced silent corruption or a crash.

### Errors that say where

The walker tracks its position, so failures read:

```
render(): vstack[2] > hstack[1] > button: field 'text' expected string, got table
```

Today a malformed widget fails opaquely or silently renders nothing.

### Schema cleanup

**Deleted — declared but never rendered:** `split_pane`, `path_bar`, `image`,
`drop_zone`. Three are callable, documented Lua functions today that emit the
literal text `[unknown widget: image]` into the user's panel via the renderer's
default branch. Nothing regresses by removing them. `image` is the most likely
to return first.

**Deleted — security:** `html`. It exists as an escape hatch for gaps in the
widget set; the correct response to those gaps is to close them, not to keep a
hole that voids the capability model. If it returns, it comes back behind its
own capability with sanitization. Note this deletion requires migrating
`examples/plugins/lua-tmux-manager.lua`, which builds its entire UI from it.

**Consolidated:** `Label`, `Text`, and `IconLabel` become one
`ui.label{ text, style, icon, mono }`. `Badge`, `KeyValue`, `Heading`, and
`Progress` keep their own identities.

---

## 3. Host events & lifecycle

### Subscription replaces `on_event`

```lua
function setup()
  events.on("tab.focused", function(e) state.active = e.tab_id end)
  events.on("session.connected", function(e) state.hosts[e.tab_id] = e.host end)
end
```

`events.on(name, fn)` returns an unsubscribe handle and permits multiple
handlers per event. Handlers mark the surface dirty on return.

Host events and inter-plugin bus events share one call — a plugin should not
care whether the publisher was the host or another plugin. `events.on` registers
the bus subscription itself, removing today's footgun where `app.subscribe` and
the `on_event` branch must both be written or the plugin silently receives
nothing.

### Initial event set

| Event | Payload |
|---|---|
| `tab.opened` | `tab_id`, `title`, `kind` (`local` / `ssh`) |
| `tab.closed` | `tab_id` |
| `tab.focused` | `tab_id`, `previous_tab_id` |
| `tab.renamed` | `tab_id`, `title` |
| `session.connected` | `tab_id`, `host`, `user` |
| `session.disconnected` | `tab_id`, `reason` |
| `app.theme_changed` | `theme` |
| `app.config_changed` | — |

Emitted from a single `host_events` module in `termlab_tauri` that the tab and
session code calls — one place to add an event, one place to test, rather than
`bus.publish` calls scattered across `lib.rs` and `remote/`.

Terminal output events are out of scope; they need streaming and backpressure,
which is a different mechanism.

### Events are gated

Tab titles routinely contain working directory paths; session events carry
hostnames and usernames. Subscription to `tab.*` is gated behind `events.tabs`
and `session.*` behind `events.sessions`. A denied subscription means the
handler never fires, plus the existing one-time user warning.

`app.theme_changed` and `app.config_changed` are ungated — they carry no
user data, and a plugin that cannot observe the theme cannot render correctly.

### Timers

```lua
local id = app.set_interval(5000, function() refresh_data() end)
app.set_timeout(200, function() … end)
app.clear_timer(id)
```

Implemented as a min-heap of due timers with `recv_timeout` on the mailbox in
place of `blocking_recv`. Once timers exist, the 1 Hz global `host.tick` has no
reason to exist and is removed — plugins that poll schedule their own interval.

### Blocking-call guard

`session.exec_local` calls `Command::output()` with no timeout, and the
instruction-limit hook cannot fire during a blocking Rust call, so
`exec_local("sleep 9999")` wedges a plugin's mailbox permanently. It gains a
default timeout with an override: `session.exec_local(cmd, { timeout_ms = 5000 })`.
Full async is deferred.

### Removed

`on_event`, `app.subscribe`, the `PluginEvent::BusEvent` / `MenuAction` /
`ThemeChanged` variants (all become named events), and the `host.tick`
publisher.

---

## 4. Packaging & manifest

A plugin is a directory:

```
session-dashboard/
  plugin.toml
  init.lua
  lib/format.lua
```

```toml
id = "session-dashboard"          # stable identity; defaults to directory name
name = "Session Dashboard"
version = "1.0.0"
api = "^2.0"
description = "Live view of open sessions"
entry = "init.lua"
type = "tool_window"              # or "action"
location = "left"
icon = "icon.png"

permissions = ["ui.panel", "events.tabs", "config.read"]

[[keybinds]]
action = "toggle"
binding = "cmd+shift+d"
description = "Toggle dashboard"
```

TOML for consistency with `config.toml` and `state.toml`; `termlab_core` already
deserializes it.

**Single-file plugins are removed.** Keeping them means keeping comment-header
parsing alongside the manifest — two metadata formats for one concept, which is
how parsers get buggy and docs get confusing. The cost is one extra file for a
trivial plugin.

**`require` returns, scoped to the package.** A custom Lua searcher resolves
module names against the plugin's own directory only: no absolute paths, no `..`
escapes, results cached. This is what makes multi-file plugins possible without
reopening general filesystem access.

Discovery scans for subdirectories containing `plugin.toml`.

---

## 5. Config, JSON, and table layout

Config becomes structured. `config.get("servers")` returns a Lua table;
`config.set("servers", t)` accepts one. The same per-plugin JSON files back it;
the serde bridge does the conversion, so plugins stop hand-rolling
serialization against a string-only store.

A `json` library joins the sandbox (`json.encode`, `json.decode`). Its absence
is why `on_query` is currently string-in, string-out.

Inter-plugin RPC becomes symmetric with events and structured:

```lua
services.handle("list_sessions", function(args) return state.sessions end)
local result = services.call("other-plugin", "list_sessions", { limit = 10 })
```

### Resulting surface

| Table | Contents |
|---|---|
| `ui` | widget constructors, dialogs, docked views, `refresh` |
| `app` | log, notify, status, clipboard, theme, menu/commands, timers |
| `events` | `on`, `off` |
| `services` | `handle`, `call` |
| `config` | `get`, `set` |
| `session` | tabs, exec, current session |
| `json` | `encode`, `decode` |

Seven tables with clear ownership, replacing four with overlapping ones
(`app.query_plugin` beside `on_query`, `app.subscribe` beside `on_event`, config
split across `get_config` and `get_setting_value`).

---

## Documentation requirements

Documentation drift has been a recurring problem in this repo, and
`docs/plugin-sdk.md` is the only real contract plugin authors have. A stale
entry is worse than a missing one: `ui.panel_image` was documented for a widget
that never rendered.

Therefore:

- **Every task that adds, renames, removes, or changes the signature of anything
  in the plugin surface updates `docs/plugin-sdk.md` in the same commit.** Doc
  updates are never batched to the end of the branch.
- **Branch review includes a documentation review.** The doc is diffed against
  the actual API surface, checking both directions: entries describing functions
  that no longer exist, and functions with no entry.
- `docs/plugin-security-model.md` is updated in the same way for capability
  changes — the new `events.*` capabilities, and the removal of `html`.

## Testing

| Area | Coverage |
|---|---|
| Widget walker | Per-variant conversion, callback hoisting, error paths with positions |
| Data bridge | Round trips, plus the escaping cases that break the current codegen — specifically a key ending in a backslash |
| Render loop | Invalidation coalescing, stale-callback-id rejection, the refresh-during-render cap |
| Reconciler | In-place update, replacement, focus and scroll preservation (vitest + jsdom) |
| Events | Registration, dispatch, unsubscribe, capability denial |
| Timers | Scheduling, cancellation, interval repetition |
| Manifest | Parsing, defaults, permission lists, invalid input |
| `require` searcher | Resolution within the package, rejection of `..` and absolute paths |

`render` being pure means the Rust suite can load a Lua chunk, call `render`,
and assert on the serialized tree — real coverage of the whole Lua→JSON path
with no GUI.

## Migration

- `examples/plugins/lua-tmux-manager.lua` — largest job; entirely `html`-based,
  needs rebuilding on real widgets.
- `examples/plugins/lua-docked-view-example.lua`, `plugins/*.lua` — port to
  directory layout and the new render model.
- `docs/plugin-sdk.md` — the Lua half is rewritten; the Java half changes where
  the shared `HostApi` trait changes.
- `editors/vscode` and `editors/neovim` — completion and stub definitions track
  the new tables.
- Java tier: `HostApi` changes (panel invalidation, events) apply to both tiers.
  Java plugins keep their own callback model but gain the same events.

## Sequencing

Six pieces, in dependency order. Each is independently reviewable and leaves the
build green.

1. ~~Data bridge (serde)~~ — **done**, merged in #108. Plan:
   `docs/superpowers/plans/2026-09-01-serde-data-bridge.md`
2. Widget schema + walker — new constructors, deletions, typed errors
3. Render loop + reconciler — invalidation, diffing, callback registry
4. Events + timers — `host_events` module, capability gates, `host.tick` removal
5. Packaging — manifest, directory discovery, scoped `require`
6. Migration — in-repo plugins, editors, docs sweep

Carried out of step 1 and still open: `handle_query` (`runner.rs`) hands
`on_query` a hand-serialized JSON **string** and silently discards a malformed
reply with `serde_json::from_str(&result).ok()`. It is the last hand-rolled
JSON edge in the Lua runtime and belongs with the services work in step 4.

This is a substantial build. Whether it lands as one long-lived branch or a
sequence of six is a delivery decision to make before implementation starts;
the sequence above works either way.

## Open questions

None blocking. Two to revisit during implementation:

- Whether keyed reconciliation is needed before tables and trees ship, or can
  follow once a plugin actually reorders rows.
- Whether `action`-type plugins (no panel) still need a `render` entry point at
  all, or should be pure `setup` + menu handlers.
