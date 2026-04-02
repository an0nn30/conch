# Plugin Security Model

This document defines the plugin API compatibility and permission model for Conch plugins.

## Objectives

- Keep plugins robust across host upgrades.
- Enforce least privilege.
- Make capability access explicit and auditable.
- Preserve backward compatibility for existing plugins during rollout.

## Versioning Model

Conch tracks plugin API compatibility separately from plugin release versions.

- `plugin-version`: plugin's own release version (already supported)
- `plugin-api`: host API requirement (new)

### Host API version

Defined in `conch_plugin_sdk`:

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

- `^1` or `^1.0` (same major)
- `1`, `1.0`, `1.0.0` (same major, required minor <= host minor)

If plugin does not declare `plugin-api`, host allows loading as legacy mode.

If incompatible, plugin is rejected with a clear error.

## Permission Model (Phase 2)

Plugins declare requested capabilities. Host enforces denied-by-default runtime gates.

### Metadata

Lua:

- `-- plugin-permissions: clipboard.read, clipboard.write, ui.menu`

Java manifest:

- `Plugin-Permissions: clipboard.read,clipboard.write,ui.menu`

### Capability Groups

- `ui.menu`
- `ui.panel`
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
| `notify`, `set_status` | `ui.notify` |
| `show_form`, `show_confirm`, `show_prompt`, `show_alert`, `show_error`, `show_context_menu` | `ui.dialog` |
| `clipboard_get` | `clipboard.read` |
| `clipboard_set` | `clipboard.write` |
| `get_config` | `config.read` |
| `set_config` | `config.write` |
| `publish_event` | `bus.publish` |
| `subscribe` | `bus.subscribe` |
| `query_plugin` | `bus.query` |
| `write_to_pty` | `session.write` |
| `new_tab` | `session.new_tab` |
| `session_prompt` | `session.exec` |
| `open_session` | `session.open` |
| `close_session` | `session.close` |
| `set_session_status` | `session.status` |

### Capability Recipes

These are practical capability bundles for common plugin shapes that are realistic with the current SDK:

| Plugin idea | Typical capabilities |
|---|---|
| Session Scratchpad | `ui.menu`, `ui.dialog`, `clipboard.read`, `clipboard.write`, `session.write`, `session.new_tab`, `config.read`, `config.write` |
| Session Dashboard | `ui.panel`, `session.exec`, `config.read`, `config.write` |
| Network Probe | `ui.menu`, `ui.dialog`, `ui.notify`, `net.resolve`, `net.scan` |
| Command Runner | `ui.panel`, `ui.dialog`, `ui.notify`, `session.exec`, `session.new_tab`, `config.read`, `config.write` |
| Bus Demo publisher | `ui.menu`, `bus.publish` |
| Bus Demo monitor | `ui.panel`, `bus.subscribe`, `bus.query` |

These recipes are intentionally small. Start with the narrowest set that supports the workflow, then add more only when the plugin genuinely needs them.


## Consent UX

When enabling a plugin:

- Show requested capabilities grouped by risk tier.
- Offer `Allow all`, `Deny`, and `Allow selected`.
- Persist grants by plugin identity + plugin version + plugin fingerprint.

When plugin upgrades and requests additional capabilities:

- Prompt only for newly requested capabilities.

## Persistence

Persist grants in a dedicated file, e.g.:

- `~/.config/conch/plugin_permissions.toml`

Suggested schema:

- plugin identity (name + source + canonical path/jar hash)
- requested capabilities
- granted capabilities
- denied capabilities
- plugin version
- last prompted timestamp

## Rollout Plan

1. Phase 1: API compatibility checks (`plugin-api`) with legacy fallback when missing.
2. Phase 2: capability declarations + host-side permission enforcement wrapper.
3. Phase 3: permission consent UI + management in Settings.
4. Phase 4: strict mode option (deny undeclared capabilities).
