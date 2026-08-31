# Plugin Security Model

This document defines the plugin API compatibility and permission model for TermLab plugins.

## Objectives

- Keep plugins robust across host upgrades.
- Enforce least privilege.
- Make capability access explicit and auditable.
- Preserve backward compatibility for existing plugins during rollout.

## Versioning Model

TermLab tracks plugin API compatibility separately from plugin release versions.

- `plugin-version`: plugin's own release version (already supported)
- `plugin-api`: host API requirement (new)

### Host API version

Defined in `termlab_plugin_sdk`:

- `HOST_PLUGIN_API_MAJOR`
- `HOST_PLUGIN_API_MINOR`

Current host API is `1.0`.

### Plugin metadata fields

Lua headers:

- `-- plugin-api: ^1.0`

Java JAR manifest:

- `Plugin-Api: ^1.0`

### Compatibility (Phase 1)

Supported requirement syntax:

- `^1` or `^1.0` — caret range, matches any host with the **same major**
- `1`, `1.0`, `1.0.0` — bare version, requires an **exact major *and* minor match**

> The bare form is stricter than it looks: `1.0` does **not** match a `1.1` host.
> Prefer `^1.0` unless you specifically need to pin to one host minor.
> (`api_requirement_matches`, `crates/termlab_tauri/src/plugins/mod.rs`.)

If a plugin does not declare `plugin-api`, the host allows loading as legacy mode.
Note the asymmetry with permissions below: a **missing `plugin-api` is permissive,
a missing `plugin-permissions` is not**.

If incompatible, plugin is rejected with a clear error.

## Permission Model (Phase 2)

Plugins declare requested capabilities. Host enforces denied-by-default runtime gates.

> **A plugin that declares no permissions gets none.** There is no legacy fallback:
> when `plugin-permissions` is absent the host builds a `deny_all()` profile, so every
> gated call fails and raises a "Plugin Permission Denied" dialog. Declaring an
> *unknown* capability is worse still — it fails the load outright. Always declare the
> capabilities your plugin uses.

### Metadata

Lua:

- `-- plugin-permissions: clipboard.read, clipboard.write, ui.menu`

Java manifest:

- `Plugin-Permissions: clipboard.read,clipboard.write,ui.menu`

### Capability Groups

- `ui.menu`
- `ui.panel`
- `ui.settings`
- `ui.dock`
- `ui.notify`
- `ui.dialog`
- `clipboard.read`
- `clipboard.write`
- `config.read`
- `config.write`
- `bus.publish`
- `bus.subscribe`
- `bus.query`
- `session.write`
- `session.new_tab`
- `session.rename_tab`
- `session.exec`
- `session.open`
- `session.close`
- `session.status`
- `net.resolve`
- `net.scan`

### Host API mapping

| HostApi method | Required capability |
|---|---|
| `register_menu_item` | `ui.menu` |
| `register_panel`, `set_widgets` | `ui.panel` |
| `register_settings_section` | `ui.settings` |
| `open_docked_view`, `close_docked_view`, `focus_docked_view` | `ui.dock` |
| `notify`, `set_status` | `ui.notify` |
| `show_form`, `show_confirm`, `show_prompt`, `show_alert`, `show_error`, `show_context_menu` | `ui.dialog` |
| `clipboard_get` | `clipboard.read` |
| `clipboard_set` | `clipboard.write` |
| `get_config`, `get_setting_value` | `config.read` |
| `set_config`, `set_setting_draft` | `config.write` |
| `publish_event`, `register_service` | `bus.publish` |
| `subscribe` | `bus.subscribe` |
| `query_plugin` | `bus.query` |
| `write_to_pty` | `session.write` |
| `new_tab` | `session.new_tab` |
| `new_tab_with_title` | `session.new_tab` **and** `session.rename_tab` |
| `rename_active_tab`, `rename_tab_by_id`, `focus_tab_by_id` | `session.rename_tab` |
| `session_prompt`, `exec_active_session` | `session.exec` |
| `open_session` | `session.open` |
| `close_session` | `session.close` |
| `get_active_session`, `set_session_status` | `session.status` |

Gated in the language bindings rather than the host wrapper — the capability is still
enforced, just at a different layer (`lua/api/net.rs`, `lua/api/session.rs`,
`java-sdk/.../HostApi.java`):

| Binding call | Required capability |
|---|---|
| `net.resolve` | `net.resolve` |
| `net.scan` | `net.scan` |
| `exec_local` | `session.exec` |

**Deliberately ungated:** `log`, `get_theme`, and `plugin_name` need no capability.

### Capability Recipes

These are practical capability bundles for common plugin shapes that are realistic with the current SDK:

| Plugin idea | Typical capabilities |
|---|---|
| Session Scratchpad | `ui.menu`, `ui.dialog`, `clipboard.read`, `clipboard.write`, `session.write`, `session.new_tab`, `config.read`, `config.write` |
| Session Dashboard | `ui.panel`, `session.status`, `session.exec`, `config.read`, `config.write` |
| Network Probe | `ui.menu`, `ui.dialog`, `ui.notify`, `net.resolve`, `net.scan` |
| Command Runner | `ui.panel`, `ui.dialog`, `ui.notify`, `session.exec`, `session.new_tab`, `config.read`, `config.write` |
| Bus Demo publisher | `ui.menu`, `bus.publish` |
| Bus Demo monitor | `ui.panel`, `bus.subscribe`, `bus.query` |

These recipes are intentionally small. Start with the narrowest set that supports the workflow, then add more only when the plugin genuinely needs them.


## Consent UX (current behavior)

When you enable a plugin from **Settings > Plugins**, the host shows a single dialog
listing every capability the plugin declares, and asks "Allow and enable this plugin?"

- The choice is **all-or-nothing**. Accepting enables the plugin with everything it
  declared; declining leaves it disabled. There is no per-capability selection.
- Capabilities are shown as a flat list — they are **not** grouped by risk tier.
- The prompt fires on **manual enable only**. Plugins re-enabled from `state.toml`
  at startup are not re-prompted.

## Persistence (current behavior)

**Permission grants are not persisted.** The permission profile is an in-memory map
rebuilt from the plugin file's own declarations every time the plugin loads, so the
declaration in the `.lua` header or JAR manifest is always the source of truth.

What *is* persisted is only the list of enabled plugins, in `state.toml`.

> **Consequence worth knowing:** because grants are derived from the plugin file and
> never compared against a stored record, a plugin that is upgraded to declare *more*
> capabilities picks them up silently on the next launch, with no new prompt. Re-review
> plugins you update from untrusted sources.

## Lua sandbox

The Lua VM is created with a restricted standard library — `string`, `table`, `math`,
`utf8`, and `coroutine` only — and `loadfile`, `dofile`, and `require` are removed from
the global environment (`crates/termlab_plugin/src/lua/runner.rs`).

This means **`os.*`, `io.*`, `package`, and `debug` are unavailable**, as is loading
another Lua file or a C module. Common surprises: `os.time()`, `os.getenv()`, and
`io.open()` all fail. Use the host APIs instead — filesystem and process access is
mediated through capability-gated calls, not raw Lua stdlib.

## Rollout status

| Phase | Status |
|---|---|
| 1. API compatibility checks (`plugin-api`) with legacy fallback | **Shipped** |
| 2. Capability declarations + host-side enforcement wrapper | **Shipped** |
| 3. Permission consent UI | **Shipped** (all-or-nothing prompt) |
| 3b. Permission management in Settings | Not implemented |
| 4. Strict mode — deny undeclared capabilities | **Shipped** — this is the only mode; there is no permissive fallback |

### Planned / not yet implemented

The following were part of the original design and are **not** built. They are recorded
here as intent, not as behavior you can rely on:

- Per-capability consent (`Allow selected`) and risk-tier grouping in the prompt.
- Persisting grants to a dedicated store (e.g. `~/.config/termlab/plugin_permissions.toml`)
  keyed by plugin identity + version + fingerprint, with fields for requested/granted/
  denied capabilities and a last-prompted timestamp.
- Re-prompting only for **newly** requested capabilities when a plugin upgrades.
- A permissions management surface in Settings.
