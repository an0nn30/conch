# Conch Plugin System

Conch includes a Lua 5.4 plugin system that lets you automate tasks, build tools, and extend the terminal with custom functionality. Plugins run in a sandboxed environment with access to sessions, UI dialogs, cryptography, and application controls.

## Getting Started

### Plugin Location

Place `.lua` files in the plugins directory:

```
~/.config/conch/plugins/
```

Conch scans this directory on startup and when you click Refresh in the Plugins panel. Symlinks are supported, so you can keep plugins in a git repo and link them here.

### Example Plugins

Conch ships with example plugins in `examples/plugins/`:

| Plugin | Type | Shortcut | Description |
|--------|------|----------|-------------|
| **System Info** (`system-info.lua`) | Panel | Cmd+Shift+I | Live system information — hostname, memory, disk, load average, top processes. Platform-aware (macOS/Linux). |
| **Port Scanner** (`port-scanner.lua`) | Panel | Cmd+Shift+O | TCP port scanner with common port detection, range scanning, and service identification. |
| **Encrypt / Decrypt** (`encrypt-decrypt.lua`) | Action | Cmd+Shift+Y | AES encryption/decryption (CBC, GCM, ECB) with PBKDF2 key derivation. |

To use them, symlink into your plugins directory:

```bash
ln -s /path/to/rusty_conch/examples/plugins/*.lua ~/.config/conch/plugins/
```

## Plugin Types

### Action Plugins (default)

Action plugins run once when triggered and then exit. They appear in the **Tools** menu and can be launched from the Plugins sidebar tab or via keyboard shortcut.

Use action plugins for one-shot tasks: encryption, deployment scripts, server management, etc.

### Panel Plugins

Panel plugins create persistent sidebar tabs with live-updating content. They stay running and periodically refresh their display. Declare a panel plugin with:

```lua
-- plugin-type: panel
```

Panel plugins appear as additional tabs in the left sidebar alongside Files and Plugins. They are activated when loaded and run continuously until unloaded.

## Plugin Header

Every plugin should start with metadata comments:

```lua
-- plugin-name: My Plugin
-- plugin-description: A short description of what it does
-- plugin-version: 1.0.0
-- plugin-type: panel
-- plugin-icon: my-icon.png
-- plugin-keybind: open_panel = cmd+shift+i | Toggle my panel
```

These comments must be at the top of the file (before any code).

| Header | Required | Description |
|--------|----------|-------------|
| `plugin-name` | No | Display name (defaults to filename) |
| `plugin-description` | No | Short description shown in the sidebar |
| `plugin-version` | No | Semver version string |
| `plugin-type` | No | `panel` for panel plugins (default: action) |
| `plugin-icon` | No | Path to a 16x16 icon (PNG, JPEG, GIF, BMP, WebP, ICO) |
| `plugin-keybind` | No | Keyboard shortcut declaration (repeatable) |

## Plugin Icons

Plugins can provide a custom icon that appears on their sidebar tab and in the plugin list.

### Declaring an Icon

Add a `-- plugin-icon:` comment to the plugin header:

```lua
-- plugin-icon: my-icon.png
```

Paths are relative to the plugin file's directory, or absolute. The icon should be a 16x16 image in one of: PNG, JPEG, GIF, BMP, WebP, or ICO format.

### Validation

Icons are validated before loading:
- File must exist and have an allowed image extension
- File size must be between 16 bytes and 2 MB
- File header bytes must match a known image format (magic number check)
- The image must decode successfully

This prevents path traversal, non-image files, and injection attempts from being loaded.

### Runtime Icon Setting

Plugins can also set their icon at runtime:

```lua
app.set_icon("/path/to/icon.png")  -- returns true on success, false on failure
```

The same validation rules apply. The path must point to a real image file.

## Plugin Keybindings

Plugins can declare default keyboard shortcuts that work globally in the app.

### Declaring Keybindings

Add `-- plugin-keybind:` comments to the plugin header:

```lua
-- plugin-keybind: action = binding | description
```

- **`action`** — what happens when the shortcut is pressed:
  - `open_panel` — switch to this plugin's panel tab (panel plugins only)
  - `run` — execute the plugin (action plugins only)
  - Any other name — sent as a custom event to the running plugin
- **`binding`** — key combo string, e.g. `cmd+shift+i`, `ctrl+alt+s`
- **`description`** — optional human-readable label (after `|`)

Multiple keybindings can be declared:

```lua
-- plugin-keybind: open_panel = cmd+shift+i | Show panel
-- plugin-keybind: refresh = cmd+shift+r | Force refresh
```

### Priority

Plugin keybindings are **lower priority** than app-level shortcuts. The evaluation order is:

1. App-level shortcuts (new tab, close tab, toggle sidebar, etc.)
2. Plugin keybindings
3. File browser keyboard navigation
4. Terminal PTY forwarding

If a plugin binding conflicts with an app shortcut, it is silently skipped with a log warning.

### User Overrides

Users can override plugin keybindings in `config.toml`:

```toml
[conch.keyboard.plugins]
"system-info.open_panel" = "cmd+shift+i"
"encrypt-decrypt.run" = "cmd+shift+y"
```

The key format is `"plugin-filename-stem.action_name"`. Config values take precedence over the plugin's default binding.

### Runtime Registration

Plugins can also register keybindings dynamically at runtime:

```lua
app.register_keybind("my_action", "cmd+alt+k", "Do something cool")
```

Returns `true` on success, `false` if the binding conflicts with an app shortcut.

## Loading and Unloading Plugins

Plugins must be explicitly **loaded** before they are active. The Plugins sidebar tab shows all discovered plugins with:

- Checkboxes to toggle load state
- Green **Loaded** / gray **Not loaded** status indicators
- A **panel** badge for panel-type plugins
- An **Apply** button that appears when changes are pending

Load state is persisted across sessions in `state.toml`. Panel plugins are activated and deactivated in-place without requiring a restart.

### Running Plugins

- **Action plugins**: Open the Plugins tab and click Run, or use the Tools menu, or press the plugin's keyboard shortcut.
- **Panel plugins**: Once loaded, they appear as sidebar tabs. Use `open_panel` keybinding or click the tab.
- **Cmd+Shift+P** (configurable) opens a plugin search for quick access.

## Panel Plugin Lifecycle

Panel plugins define up to four functions:

```lua
function setup()
    -- Called once when the panel is first activated.
    -- Use for one-time initialization.
end

function render()
    -- Called on each refresh cycle to build the panel UI.
    -- Use ui.panel_* functions to describe widgets.
    ui.panel_clear()
    ui.panel_heading("Hello")
    ui.panel_label("World")
end

function on_click(button_id)
    -- Called when a panel button is clicked.
    if button_id == "refresh" then
        ui.panel_clear()
        ui.panel_label("Refreshing...")
    end
end

function on_keybind(action)
    -- Called when a custom keybinding is triggered.
    if action == "refresh" then
        -- handle the "refresh" keybind action
    end
end
```

The `render()` function is called every 10 seconds by default. Use `ui.set_refresh(seconds)` to change the interval.

### Silent Command Execution

`session.exec(cmd)` runs commands **silently** — it does not inject them into the active terminal:

- **SSH sessions**: Opens a separate SSH channel, runs the command, and returns stdout. The terminal PTY is untouched.
- **Local sessions**: Runs via a subprocess (`sh -c "..."`).
- **No active session**: Falls back to local subprocess execution.

This means panel plugins can poll system info without interfering with whatever you're typing in the terminal.

### Platform Detection

Use `session.platform()` to detect the OS of the active session and tailor commands accordingly:

```lua
local platform = session.platform()  -- "macos", "linux", "freebsd", etc.

if platform == "macos" then
    local mem = session.exec("sysctl -n hw.memsize")
elseif platform == "linux" then
    local mem = session.exec("cat /proc/meminfo")
end
```

For SSH sessions, this runs `uname -s` on the remote host. For local sessions, it uses compile-time detection.

### Panel Widget API (`ui.panel_*`)

| Function | Description |
|----------|-------------|
| `ui.panel_clear()` | Clear current panel contents |
| `ui.panel_heading(text)` | Bold section header |
| `ui.panel_text(text)` | Monospace text block |
| `ui.panel_label(text)` | Proportional text |
| `ui.panel_separator()` | Horizontal divider |
| `ui.panel_table(columns, rows)` | Data table |
| `ui.panel_progress(label, fraction, text)` | Progress bar (fraction 0.0–1.0) |
| `ui.panel_button(id, label)` | Clickable button (triggers `on_click`) |
| `ui.panel_kv(key, value)` | Key-value pair row |
| `ui.set_refresh(seconds)` | Set auto-refresh interval (default: 10s, 0 = manual only) |

## API Reference

Conch exposes five global tables to plugins: `session`, `app`, `ui`, `crypto`, and `net`.

### `session` — Session Interaction

| Function | Returns | Description |
|----------|---------|-------------|
| `session.exec(cmd)` | `string` | Execute a command silently and return stdout |
| `session.send(text)` | — | Send raw text to the active terminal (no newline) |
| `session.run(cmd)` | — | Send a command + newline to the active terminal |
| `session.platform()` | `string` | Get the OS: `"macos"`, `"linux"`, `"freebsd"`, etc. |
| `session.current()` | `table\|nil` | Get info about the active session |
| `session.all()` | `table` | Get info about all open sessions |
| `session.named(name)` | `table\|nil` | Get a handle to a session by name |

#### Session Info Table

Tables returned by `session.current()`, `session.all()`, and `session.named()` contain:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `string` | Unique session identifier |
| `title` | `string` | Session display title |
| `type` | `string` | `"local"` or `"ssh"` |

#### Named Session Handles

`session.named(name)` returns a handle table with bound methods that target that specific session:

```lua
local srv = session.named("webserver")
if srv then
    srv.run("uptime")          -- runs on "webserver", not the active tab
    srv.send("ls -la\n")       -- send raw text
    local out = srv.exec("hostname")
end
```

### `app` — Application Controls

| Function | Returns | Description |
|----------|---------|-------------|
| `app.open_session(name)` | — | Open a saved SSH connection by server name or host |
| `app.clipboard(text)` | — | Copy text to the system clipboard |
| `app.notify(msg_or_table)` | `string\|nil` | Show a toast notification (see [Notifications](#notifications)) |
| `app.log(msg)` | — | Log a message (visible in application logs) |
| `app.servers()` | `table` | Get a list of all configured server names |
| `app.server_details()` | `table` | Get servers with `name` and `host` fields |
| `app.set_icon(path)` | `bool` | Set the plugin's icon from a file path (validated) |
| `app.register_keybind(action, binding, desc?)` | `bool` | Register a keybinding at runtime |

#### Server Details

`app.server_details()` returns an array of tables with `name` and `host` fields:

```lua
local servers = app.server_details()
for _, srv in ipairs(servers) do
    print(srv.name .. " -> " .. srv.host)
end
```

### `ui` — User Interface

#### Output Panel

| Function | Returns | Description |
|----------|---------|-------------|
| `ui.append(text)` | — | Append a line to the plugin output panel in the sidebar |
| `ui.clear()` | — | Clear the plugin output panel |

#### Dialogs

All dialog functions are **blocking** — the plugin pauses until the user responds.

| Function | Returns | Description |
|----------|---------|-------------|
| `ui.form(title, fields)` | `table\|nil` | Show a form dialog; returns field values or `nil` if cancelled |
| `ui.prompt(message)` | `string\|nil` | Show a text input prompt |
| `ui.confirm(message)` | `boolean` | Show a Yes/No confirmation dialog |
| `ui.alert(title, message)` | — | Show an informational alert |
| `ui.error(title, message)` | — | Show an error alert (red text) |
| `ui.show(title, text)` | — | Show a read-only text viewer with a Copy button |
| `ui.table(title, columns, rows)` | — | Show a table viewer |

#### Progress Indicator

| Function | Returns | Description |
|----------|---------|-------------|
| `ui.progress(message)` | — | Show a progress spinner with a message |
| `ui.hide_progress()` | — | Hide the progress spinner |

#### Form Fields

The `ui.form()` function accepts a table of field descriptors:

| Type | Keys | Description |
|------|------|-------------|
| `"text"` | `name`, `label`, `default` | Single-line text input |
| `"password"` | `name`, `label` | Password input (masked) |
| `"combo"` | `name`, `label`, `options`, `default` | Dropdown select |
| `"checkbox"` | `name`, `label`, `default` | Boolean checkbox |
| `"separator"` | — | Visual separator line |
| `"label"` | `text` | Static text (italic, not editable) |

Returns a table mapping field `name` to the user's input, or `nil` if cancelled. Checkbox values are `"true"`/`"false"` strings.

### `crypto` — Cryptography

AES encryption and decryption with PBKDF2 key derivation.

| Function | Returns | Description |
|----------|---------|-------------|
| `crypto.encrypt(plaintext, passphrase, algorithm)` | `string` | Encrypt text, returns base64-encoded ciphertext |
| `crypto.decrypt(encoded, passphrase, algorithm)` | `string` | Decrypt base64-encoded ciphertext, returns plaintext |
| `crypto.algorithms()` | `table` | List supported algorithm strings |

Supported algorithms: `AES-128-CBC`, `AES-256-CBC`, `AES-128-GCM`, `AES-256-GCM`, `AES-128-ECB`, `AES-256-ECB`.

Key derivation uses PBKDF2-HMAC-SHA256 with 310,000 iterations and a random 16-byte salt. Output format: `base64(salt || iv || ciphertext)`.

### `net` — Networking

TCP port scanning, DNS resolution, and timing utilities. All networking runs from the app machine (not through SSH sessions). For scanning from a remote host's perspective, use `session.exec()` with the appropriate tools.

| Function | Returns | Description |
|----------|---------|-------------|
| `net.check_port(host, port, timeout_ms?)` | `bool` | Check if a single TCP port is open (default timeout: 1000ms) |
| `net.scan(host, ports, timeout_ms?, concurrency?)` | `table` | Scan a list of ports. Returns `{port=N, open=bool}` entries. Default concurrency: 50 |
| `net.scan_range(host, start, end, timeout_ms?, concurrency?)` | `table` | Scan a port range. Returns only open port numbers. Default concurrency: 100 |
| `net.resolve(hostname)` | `table` | DNS lookup. Returns a table of IP address strings |
| `net.time()` | `number` | Monotonic timestamp in seconds (for measuring durations) |

#### Port Scanning Examples

```lua
-- Check a single port
if net.check_port("example.com", 443) then
    print("HTTPS is open")
end

-- Scan common ports
local results = net.scan("192.168.1.1", {22, 80, 443, 3306, 5432})
for _, r in ipairs(results) do
    if r.open then print("Port " .. r.port .. " is open") end
end

-- Scan a range, get only open ports
local open = net.scan_range("10.0.0.1", 1, 1024, 2000, 200)
for _, port in ipairs(open) do
    print("Open: " .. port)
end

-- DNS resolution
local ips = net.resolve("github.com")
for _, ip in ipairs(ips) do print(ip) end

-- Timing a scan
local t0 = net.time()
net.scan_range("localhost", 1, 100)
local elapsed = net.time() - t0
print(string.format("Scan took %.1fs", elapsed))
```

## Notifications

Plugins can show toast notifications that slide in from the top-right corner of the app. Notifications support different severity levels, auto-dismiss or persistent display, and optional interactive buttons.

### Simple Notification

Pass a string for a quick fire-and-forget notification:

```lua
app.notify("Scan complete!")
```

This shows an info-level toast that auto-dismisses after 5 seconds.

### Rich Notifications

Pass a table for full control:

```lua
app.notify({
    title = "Scan Complete",
    body = "Found 3 open ports on 192.168.1.1",
    level = "success",
    duration = 8
})
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `body` | `string` | (required) | Main notification text |
| `title` | `string` | `nil` | Optional bold heading |
| `level` | `string` | `"info"` | Severity: `"info"`, `"success"`, `"warning"`, `"error"` |
| `duration` | `number` | `5` | Seconds before auto-dismiss. `0` = stays until manually dismissed |
| `buttons` | `table` | `nil` | List of button labels (makes the call blocking) |

### Notification with Buttons

When `buttons` is provided, `app.notify()` **blocks** until the user clicks a button and returns the clicked label:

```lua
local answer = app.notify({
    title = "Confirm",
    body = "Delete the backup file?",
    level = "warning",
    buttons = {"Yes", "No"}
})

if answer == "Yes" then
    session.exec("rm backup.tar.gz")
    app.notify({ title = "Done", body = "Backup deleted.", level = "success" })
end
```

Button notifications are always persistent (they stay until clicked or dismissed with the ✕ button). If dismissed without clicking a button, `app.notify()` returns an empty string.

### Notification Behavior

- Notifications stack vertically from the top-right corner
- Slide-in animation on appear, slide-out on dismiss
- Click the ✕ button to dismiss any notification early
- For panel plugins, clicking a non-button notification opens the plugin's panel
- Multiple notifications can be visible simultaneously
- Notifications render on top of all other UI content

### Levels

| Level | Accent | Use For |
|-------|--------|---------|
| `"info"` | Blue | General messages, status updates |
| `"success"` | Green | Completed operations, confirmations |
| `"warning"` | Yellow/Orange | Potential issues, confirmations needed |
| `"error"` | Red | Failures, critical problems |

## Plugin Validation

Use `conch check` to validate plugin files without launching the GUI:

```bash
conch check my-plugin.lua
conch check plugins/*.lua
```

The checker validates:

- **Header metadata** — well-formed `plugin-name`, `plugin-type`, `plugin-keybind`, `plugin-version`, `plugin-icon` comments
- **Lua syntax** — parses and loads the script in a sandboxed environment
- **API usage** — validates function names and argument counts for all API tables (`session`, `app`, `ui`, `crypto`, `net`)
- **Lifecycle functions** — invokes `setup()`, `render()`, `on_click()`, and `on_keybind()` to catch API errors inside function bodies
- **Common mistakes** — warns if `main()` is defined (should be `setup()`/`render()`), or if a panel plugin defines neither `setup()` nor `render()`

Output uses a GCC-style format for editor integration:

```
my-plugin.lua:7:1: error: in setup(): session.exec() expects 1 argument(s), got 2
my-plugin.lua: warning: missing plugin-description header comment
```

Exit code is `0` if no errors (warnings are OK), `1` if any errors were found.

## Sandboxing

Plugins run in a restricted Lua environment. The following standard library modules are **removed**:

- `os` — no file system operations or process execution
- `io` — no file I/O
- `loadfile` / `dofile` — no arbitrary file execution

The `require()` function is available but restricted to the plugin's own directory and the LuaRocks module path.

### LuaRocks Modules

Plugins can `require()` LuaRocks modules installed to:

```
~/.config/conch/lua_modules/
```

Install modules with:

```bash
luarocks --tree ~/.config/conch/lua_modules install <module-name>
```

Plugins can also `require()` other `.lua` files from their own directory.
