# Conch Plugin SDK

Conch supports two plugin tiers:

| Tier | Language | Use Case | Build Step |
|------|----------|----------|------------|
| **Java** | Java, Kotlin, Scala, Groovy | Community plugins, rich UI, familiar ecosystem | Compile to `.jar` |
| **Lua** | Lua | Quick scripts, personal automation, no build step | Single `.lua` file |

Plugins are managed via **Settings > Plugins** -- scan, enable, disable, and persist across restarts.

## Table of Contents

- [Reality-Checked Sample Plugins](#reality-checked-sample-plugins)
- [Java Plugins](#java-plugins)
  - [Quick Start](#java-quick-start)
  - [Project Setup (Gradle)](#project-setup-gradle)
  - [ConchPlugin Interface](#conchplugin-interface)
  - [HostApi Reference](#java-hostapi)
  - [Widget Builder](#widget-builder)
  - [Handling Events](#handling-events-java)
  - [Tool-Window Plugins](#tool-window-plugins-java)
- [Lua Plugins](#lua-plugins)
  - [Quick Start](#lua-quick-start)
  - [Lua Metadata Fields](#lua-metadata-fields)
  - [Lua Plugin Lifecycle](#lua-plugin-lifecycle)
  - [Lua API Reference](#lua-api-reference)
  - [Panel Widget Functions](#panel-widget-functions)
  - [Net API](#net-api)
- [API Signatures Reference](#api-signatures-reference)
  - [Java Signatures](#java-signatures)
  - [Lua Signatures](#lua-signatures)
- [Widget System](#widget-system)
- [Widget Events](#widget-events)
- [Plugin Events](#plugin-events)
- [Render Lifecycle & Architecture](#render-lifecycle--architecture)
- [Form Dialogs](#form-dialogs)
- [Inter-Plugin Communication](#inter-plugin-communication)
- [Icons](#icons)
- [Plugin Search Paths](#plugin-search-paths)

---

## Reality-Checked Sample Plugins

These sample plugin ideas are meant to exercise the SDK as it exists today, not just the broader shared schema. They were cross-checked against the current Lua bindings, host API surface, and webview widget renderer.

If you want a docs-only acceptance test for the plugin system, these are strong candidates because they cover capabilities that are currently wired up end to end.

### 1. Session Scratchpad

- **Type:** Lua `action`
- **What it does:** Registers a command like `Scratchpad: Send Snippet`, prompts for text, optionally copies it to the clipboard, then writes it into the active terminal or opens a new tab and sends it there.
- **Capabilities exercised:** `ui.menu`, `ui.dialog`, `clipboard.read`, `clipboard.write`, `session.write`, `session.new_tab`, `config.read`, `config.write`
- **Why this is a good test:** It validates the fastest path from docs to a useful plugin: metadata headers, menu registration, dialogs, clipboard access, config persistence, and terminal interaction.

### 2. Session Dashboard

- **Type:** Lua `tool_window`
- **What it does:** Shows the active session, platform, last command result, and a few status badges inside a dockable tool window.
- **Capabilities exercised:** `ui.panel`, `session.exec`, `config.read`, `config.write`
- **Recommended widgets:** `heading`, `label`, `key_value`, `badge`, `progress`, `button`, `scroll_text`, `tabs`, `table`
- **Why this is a good test:** It covers the core tool-window lifecycle and the widget/rendering model that most plugin authors will rely on.

### 3. Network Probe

- **Type:** Java `action`
- **What it does:** Opens a form asking for host and ports, resolves DNS, scans the requested ports, and reports the results via notifications or a follow-up dialog.
- **Capabilities exercised:** `ui.menu`, `ui.dialog`, `ui.notify`, `net.resolve`, `net.scan`
- **Why this is a good test:** It is a strong Java-path test because it combines manifest metadata, permissions, dialogs, and structured result handling.

### 4. Command Runner

- **Type:** Java `tool_window`
- **What it does:** Provides a small command runner for plugin-owned tasks, runs commands locally or against the active session, and renders output plus history in a tool window.
- **Capabilities exercised:** `ui.panel`, `ui.dialog`, `ui.notify`, `session.exec`, `session.new_tab`, `config.read`, `config.write`
- **Recommended widgets:** `toolbar`, `text_input`, `button`, `scroll_text`, `table`, `html`
- **Why this is a good test:** It stresses the richer UI surface while still staying inside capabilities that are implemented today.

### 5. Bus Demo

- **Type:** Two plugins: one Lua `action`, one Lua or Java `tool_window`
- **What it does:** One plugin publishes events like `task.started` / `task.finished`, while the other subscribes to them, renders a live event log, and optionally exposes a query service.
- **Capabilities exercised:** `bus.publish`, `bus.subscribe`, `bus.query`, `ui.panel`, `ui.menu`
- **Why this is a good test:** Inter-plugin communication is one of the most important advanced workflows to validate with docs alone.

### 6. HTML Inspector

- **Type:** Lua or Java `tool_window`
- **What it does:** Renders a custom HTML-based inspector with buttons wired through `data-action` and theme-aware styles in the Shadow DOM.
- **Capabilities exercised:** `ui.panel`
- **Recommended widgets:** `html`
- **Why this is a good test:** It verifies the advanced escape hatch for custom UI without relying on undocumented renderer behavior.

---

## Java Plugins

Java plugins are JAR files loaded by an embedded JVM. Any JVM language works (Java, Kotlin, Scala, Groovy). The SDK JAR is embedded in the Conch binary -- no external files needed.

This is the recommended tier for community plugins. If you've written Bukkit/Paper plugins for Minecraft, this will feel familiar.

### Java Quick Start

**1. Create a Gradle project:**

```groovy
// build.gradle
plugins {
    id 'java'
}

dependencies {
    compileOnly files('path/to/conch-plugin-sdk.jar')
}

jar {
    manifest {
        attributes(
            'Plugin-Class': 'com.example.MyPlugin',
            'Plugin-Api': '^1.0',
            'Plugin-Permissions': 'ui.menu'
        )
    }
}
```

> Download `conch-plugin-sdk.jar` from the [Releases](https://github.com/an0nn30/rusty_conch/releases) page, or build it locally with `make java-sdk`.

**2. Implement `ConchPlugin`:**

```java
package com.example;

import conch.plugin.ConchPlugin;
import conch.plugin.HostApi;
import conch.plugin.PluginInfo;

public class MyPlugin implements ConchPlugin {

    @Override
    public PluginInfo getInfo() {
        return new PluginInfo(
            "My Plugin",
            "A simple Java plugin",
            "1.0.0"
        );
    }

    @Override
    public void setup() {
        HostApi.info("My plugin loaded!");
        HostApi.registerMenuItem("Tools", "Do Thing", "do_thing");
    }

    @Override
    public void onEvent(String eventJson) {
        if (eventJson.contains("do_thing")) {
            HostApi.info("Thing was done!");
            HostApi.notify("Success", "Thing was done!", "success", 3000);
        }
    }

    @Override
    public String render() {
        return "[]"; // No widgets (action plugin)
    }

    @Override
    public void teardown() {
        HostApi.info("My plugin unloaded");
    }
}
```

**3. Build and install:**

```bash
gradle build
cp build/libs/my-plugin.jar ~/.config/conch/plugins/
```

Open Conch, go to **Settings > Plugins**, and enable your plugin.

### Project Setup (Gradle)

```groovy
plugins {
    id 'java'
}

group = 'com.example'
version = '1.0.0'

java {
    sourceCompatibility = JavaVersion.VERSION_11
    targetCompatibility = JavaVersion.VERSION_11
}

dependencies {
    // The SDK is provided by Conch at runtime — don't bundle it.
    compileOnly files('path/to/conch-plugin-sdk.jar')
}

jar {
    manifest {
        // REQUIRED: tells Conch which class to load.
        // REQUIRED: explicit API + permission contract.
        attributes(
            'Plugin-Class': 'com.example.MyPlugin',
            'Plugin-Api': '^1.0',
            'Plugin-Permissions': 'ui.menu'
        )
    }
}
```

> **Tip: Bundling dependencies into a fat JAR.** Conch loads your plugin as a
> single JAR -- external dependencies (like Gson) must be bundled inside it.
> Use the Shadow plugin:
>
> ```groovy
> plugins {
>     id 'java'
>     id 'com.github.johnrengelman.shadow' version '8.1.1'
> }
>
> dependencies {
>     compileOnly files('libs/conch-plugin-sdk.jar')  // provided by Conch
>     implementation 'com.google.code.gson:gson:2.11.0'  // bundled into JAR
> }
>
> shadowJar {
>     archiveClassifier.set('')
>     manifest {
>         attributes(
>             'Plugin-Class': 'com.example.MyPlugin',
>             'Plugin-Api': '^1.0',
>             'Plugin-Permissions': 'ui.menu'
>         )
>     }
>     exclude 'META-INF/*.SF', 'META-INF/*.DSA', 'META-INF/*.RSA'
>     mergeServiceFiles()
> }
> ```

**Maven:**

```xml
<dependencies>
    <dependency>
        <groupId>conch.plugin</groupId>
        <artifactId>conch-plugin-sdk</artifactId>
        <version>1.0.0</version>
        <scope>system</scope>
        <systemPath>${project.basedir}/libs/conch-plugin-sdk.jar</systemPath>
    </dependency>
</dependencies>

<build>
    <plugins>
        <plugin>
            <groupId>org.apache.maven.plugins</groupId>
            <artifactId>maven-jar-plugin</artifactId>
            <configuration>
                <archive>
                    <manifestEntries>
                        <Plugin-Class>com.example.MyPlugin</Plugin-Class>
                        <Plugin-Api>^1.0</Plugin-Api>
                        <Plugin-Permissions>ui.menu</Plugin-Permissions>
                    </manifestEntries>
                </archive>
            </configuration>
        </plugin>
    </plugins>
</build>
```

### ConchPlugin Interface

Every Java plugin must implement `conch.plugin.ConchPlugin`:

| Method | Description |
|--------|-------------|
| `PluginInfo getInfo()` | Return plugin metadata (name, version, type, default zone) |
| `void setup()` | Called once on plugin load. Register menu items, initialize state. |
| `void onEvent(String eventJson)` | Handle events -- menu clicks, widget interactions, bus events. |
| `String onQuery(String method, String argsJson)` | Handle direct RPC queries from other plugins. Return a JSON value string (`"null"` by default). |
| `String render()` | Return widget tree as JSON array. Called on demand for tool-window plugins. |
| `void teardown()` | Clean up resources before unload. |

#### Plugin Types

Conch has two plugin types:

- **`action`** — Background plugin with no persistent UI. Interacts via menu items, keyboard shortcuts, and events.
- **`tool_window`** — Renders a dockable tool window using widgets or raw HTML. The user can move it between zones (left-top, left-bottom, right-top, right-bottom) at runtime.

```java
// Action plugin — no tool window, interacts via menu items and events.
new PluginInfo("My Tool", "Does things", "1.0.0");

// Tool-window plugin — renders in a dockable zone (right sidebar by default).
new PluginInfo("My Panel", "Shows info", "1.0.0", "tool_window", "right");

// Convenience factory method:
PluginInfo.toolWindow("My Panel", "Shows info", "1.0.0", "right");
```

Default zones: `"left"`, `"right"`, `"bottom"`. The user can reposition tool windows freely; the default zone is only used on first launch.

> **Backward compatibility:** The legacy type `"panel"` is accepted as an alias for `"tool_window"`.

### Java HostApi

Static methods on `conch.plugin.HostApi`.

**Logging:**

| Method | Description |
|--------|-------------|
| `log(int level, String message)` | Log a message (0=trace, 1=debug, 2=info, 3=warn, 4=error) |
| `trace(String message)` | Log at TRACE |
| `debug(String message)` | Log at DEBUG |
| `info(String message)` | Log at INFO |
| `warn(String message)` | Log at WARN |
| `error(String message)` | Log at ERROR |

**Menu Items:**

| Method | Description |
|--------|-------------|
| `registerMenuItem(String menu, String label, String action)` | Add a menu item |
| `registerMenuItemWithKeybind(String menu, String label, String action, String keybind)` | Add a menu item with keyboard shortcut (e.g. `"cmd+shift+j"`) |
| `registerCommand(String label, String action)` | Convenience alias for adding a command under `"Tools"` |
| `registerCommand(String label, String action, String keybind)` | `"Tools"` command with keybind |

> **Overload note:** `registerCommand`/`register_command` with **3 args** means `(label, action, keybind)` under `"Tools"`.  
> For a custom menu name:
> - Java: use `registerMenuItem(...)` / `registerMenuItemWithKeybind(...)`
> - Lua: use 4-arg `app.register_command(menu, label, action, keybind?)`

Users can override plugin keybinds in **Settings > Keyboard Shortcuts > Plugin Shortcuts**.
Overrides are stored in `conch.keyboard.plugin_shortcuts` using key format `"<plugin>:<action>"`.

**Notifications:**

| Method | Description |
|--------|-------------|
| `notify(String title, String body, String level, int durationMs)` | Show a toast notification (level: `"info"`, `"success"`, `"warn"`, `"error"`) |
| `notify(String title, String body, String level)` | Show notification with default duration |

**Status Bar:**

| Method | Description |
|--------|-------------|
| `setStatus(String text, int level, float progress)` | Update the global status bar. Level: 0=info, 1=warn, 2=error, 3=success. Progress: 0.0-1.0 for a progress bar, negative to hide. Pass null text to clear. |

**Permissions:**

| Method | Description |
|--------|-------------|
| `checkPermission(String capability)` | Check whether this plugin has a capability (for example `"session.exec"` or `"net.scan"`) |

**Dialogs (blocking):**

| Method | Description |
|--------|-------------|
| `prompt(String message, String defaultValue)` | Show a text input dialog, returns entered text or null |
| `prompt(String message)` | Prompt with no default value |
| `confirm(String message)` | Show Yes/No dialog, returns true/false |
| `alert(String title, String message)` | Show an alert dialog |
| `showError(String title, String message)` | Show an error dialog |
| `showForm(String formJson)` | Show a multi-field form dialog (returns JSON result or null) |

**Clipboard:**

| Method | Description |
|--------|-------------|
| `clipboardSet(String text)` | Copy text to system clipboard |
| `clipboardGet()` | Get clipboard text (returns null if unavailable) |

**Theme:**

| Method | Description |
|--------|-------------|
| `getTheme()` | Get current theme JSON (name, appearance mode, dark mode, and resolved color map) |

**Config (persistent per-plugin storage):**

| Method | Description |
|--------|-------------|
| `getConfig(String key)` | Read a config value (returns JSON string or null) |
| `setConfig(String key, String value)` | Write a config value (null to delete) |

Config is stored at `~/.config/conch/plugins/<plugin-name>/<key>.json`.

**Inter-Plugin Communication:**

| Method | Description |
|--------|-------------|
| `subscribe(String eventType)` | Subscribe to bus events from other plugins |
| `publishEvent(String eventType, String dataJson)` | Publish a bus event |
| `queryPlugin(String target, String method, String argsJson)` | RPC query to another plugin/service; returns JSON result or null |
| `registerService(String name)` | Register this plugin as a named RPC service |

**Terminal / Tabs:**

| Method | Description |
|--------|-------------|
| `writeToPty(String text)` | Write text to the focused terminal (include `\n` for Enter) |
| `newTab(String command, boolean plain)` | Open a new tab (plain=true bypasses terminal.shell config) |
| `newTab()` | Open a new tab with default shell |
| `newPlainTab(String command)` | Open a plain shell tab and run a command |
| `getActiveSession()` | Get active session metadata JSON (local/ssh, window/pane identifiers, SSH host/user/port when applicable) |
| `execActiveSession(String command)` | Execute on active session and return JSON with `status/stdout/stderr/exit_code` |

**Session / Net helpers:**

| Method | Description |
|--------|-------------|
| `platform()` | Returns `"macos"`, `"linux"`, `"windows"`, or `"unknown"` |
| `execLocal(String command)` | Execute a local shell command and return JSON result (`stdout/stderr/exit_code/status`) |
| `time()` | Unix timestamp in seconds (float) |
| `resolve(String host)` | DNS resolve helper (returns IP string array) |
| `scan(String host, int[] ports, Integer timeoutMs)` | TCP scan helper (returns `ScanResult[]` for open ports) |

> **Note:** Java and Lua now both support pub/sub and RPC query APIs. Query responses in Java are handled via `ConchPlugin.onQuery(...)`.

### Widget Builder

The `conch.plugin.Widgets` class provides a fluent builder for constructing widget trees without writing raw JSON:

```java
@Override
public String render() {
    return new Widgets()
        .heading("System Info")
        .separator()
        .keyValue("OS", System.getProperty("os.name"))
        .keyValue("Java", System.getProperty("java.version"))
        .separator()
        .button("refresh", "Refresh")
        .toJson();
}
```

**Available builder methods:**

| Method | Description |
|--------|-------------|
| `heading(String text)` | Section heading (large, bold) |
| `label(String text)` | Text label |
| `label(String text, String style)` | Styled label (`"secondary"`, `"muted"`, `"accent"`, `"warn"`, `"error"`) |
| `text(String text)` | Monospace text |
| `keyValue(String key, String value)` | Key-value pair row |
| `separator()` | Horizontal rule |
| `spacer()` | Flexible spacer |
| `spacer(float size)` | Fixed-size spacer (in points) |
| `badge(String text, String variant)` | Status badge (`"info"`, `"success"`, `"warn"`, `"error"`) |
| `progress(String id, float fraction, String label)` | Progress bar (0.0-1.0) |
| `button(String id, String label)` | Clickable button |
| `button(String id, String label, String icon)` | Button with icon |
| `textInput(String id, String value, String hint)` | Single-line text input |
| `checkbox(String id, String label, boolean checked)` | Checkbox toggle |
| `horizontal(Widgets children)` | Horizontal layout container |
| `vertical(Widgets children)` | Vertical layout container |
| `scrollArea(Float maxHeight, Widgets children)` | Scrollable container |
| `raw(String json)` | Raw JSON widget (for types not covered by the builder) |

For widget types not covered by the builder (e.g., `tree_view`, `table`, `toolbar`, `combo_box`, `tabs`, `split_pane`, `path_bar`, `image`, `icon_label`), use the `raw()` method with a JSON string matching the Widget System schema below.

### Handling Events (Java)

Events arrive as JSON strings in `onEvent()`:

```java
@Override
public void onEvent(String eventJson) {
    // Menu action
    if (eventJson.contains("\"my_action\"")) {
        HostApi.info("Menu item clicked!");
    }

    // Button click
    if (eventJson.contains("\"button_click\"") && eventJson.contains("\"my_button\"")) {
        HostApi.info("Button clicked!");
    }
}
```

For structured parsing, add Gson (`implementation 'com.google.code.gson:gson:2.11.0'`):

```java
JsonObject event = JsonParser.parseString(eventJson).getAsJsonObject();
if (event.get("kind").getAsString().equals("menu_action")) {
    String action = event.get("action").getAsString();
}
```

### Tool-Window Plugins (Java)

Set `pluginType` to `"tool_window"` and specify a default zone:

```java
@Override
public PluginInfo getInfo() {
    return PluginInfo.toolWindow("My Panel", "Shows info", "1.0.0", "right");
}

@Override
public String render() {
    return new Widgets()
        .heading("My Panel")
        .label("Hello from Java!")
        .toJson();
}
```

The tool window appears in the right sidebar by default. Users can drag it to any zone via the context menu.

---

## Lua Plugins

Lua plugins are single `.lua` files -- no compilation, no project setup. Drop a file in the plugins directory and enable it via Settings > Plugins.

### Lua Quick Start

Create a file (e.g., `~/.config/conch/plugins/my-script.lua`):

```lua
-- plugin-name: My Script
-- plugin-description: A quick automation script
-- plugin-type: action
-- plugin-version: 1.0.0

function setup()
    app.log("info", "My script loaded!")
    app.register_menu_item("Tools", "Run My Script", "run_script")
end

function on_event(event)
    if type(event) == "table" and event.action == "run_script" then
        app.notify("Done", "Script executed!", "success")
    end
end
```

Metadata is declared in comments at the top of the file. Enable via **Settings > Plugins**.

### Lua Metadata Fields

Lua plugins declare metadata in `-- plugin-*` comment headers at the top of the file. The parser reads all consecutive comment lines (and blank lines) from the start of the file, stopping at the first line of actual code.

| Header | Required | Description |
|--------|----------|-------------|
| `-- plugin-name: Name` | Yes | Display name (used as the plugin's unique identifier) |
| `-- plugin-description: ...` | No | Short description shown in Settings > Plugins |
| `-- plugin-version: 1.0.0` | No | Semver version string (default: `"0.0.0"`) |
| `-- plugin-api: ^1.0` | No | Required host plugin API version/range (legacy plugins may omit) |
| `-- plugin-permissions: cap1, cap2` | No | Declared capability list for permission gating (e.g. `clipboard.read, ui.menu, ui.panel`) |
| `-- plugin-type: action` | No | `"action"` (default) or `"tool_window"` |
| `-- plugin-location: left` | No | Default zone: `"left"` (default for tool-window plugins), `"right"`, `"bottom"` |
| `-- plugin-icon: icon.png` | No | Custom icon for the tool window tab (filename relative to plugin location) |
| `-- plugin-keybind: action = binding \| Description` | No | Register a keyboard shortcut (repeatable) |

**Keybind format:** `action_id = key_combo | Optional description`

- `action_id` is the action string delivered in the event
- `key_combo` uses the same format as menu keybinds (e.g., `cmd+shift+i`)
- The description after `|` is optional
- Users can override plugin keybinds in Settings without plugin code changes

```lua
-- plugin-name: System Info
-- plugin-description: Live system information panel
-- plugin-type: tool_window
-- plugin-version: 1.3.0
-- plugin-api: ^1.0
-- plugin-permissions: ui.panel, ui.menu
-- plugin-location: right
-- plugin-icon: system-info.png
-- plugin-keybind: open_panel = cmd+shift+i | Toggle System Info panel
-- plugin-keybind: refresh = cmd+r
```

> **Backward compatibility:** The legacy type `"panel"` is accepted as an alias for `"tool_window"`.

### Lua Plugin Lifecycle

Each Lua plugin runs on a dedicated OS thread with its own Lua VM. The host manages the lifecycle through five optional functions that the plugin can define at the global scope:

| Function | When Called | Description |
|----------|------------|-------------|
| `setup()` | Once, after the plugin source is loaded | Initialize state, register menu items, subscribe to events |
| `render()` | On demand, for tool-window plugins | Build the widget tree using `ui.panel_*` functions |
| `on_event(event)` | When any event targets this plugin | Handle widget interactions, menu actions, bus events. The `event` argument is a native Lua table. |
| `on_query(method, args_json)` | When another plugin sends an RPC query | Handle inter-plugin queries. `args_json` is a JSON string. Return a JSON string as the response. |
| `teardown()` | When the plugin is unloaded or the app shuts down | Clean up resources |

All functions are optional. A minimal plugin only needs `setup()`. Tool-window plugins need `render()`. Plugins that respond to RPC queries need `on_query()`.

**Example tool-window plugin with full lifecycle:**

```lua
-- plugin-name: Status Monitor
-- plugin-type: tool_window
-- plugin-location: bottom

local status = "idle"

function setup()
    app.register_menu_item("Tools", "Reset Monitor", "reset")
    app.register_service("status_monitor")
    app.subscribe("ssh.connected")
end

function render()
    ui.panel_heading("Status Monitor")
    ui.panel_kv("Current", status)
    ui.panel_button("refresh", "Refresh")
end

function on_event(event)
    if event.kind == "menu_action" and event.action == "reset" then
        status = "idle"
    elseif event.kind == "widget" and event.type == "button_click" and event.id == "refresh" then
        status = "refreshing..."
    elseif event.kind == "bus_event" and event.event_type == "ssh.connected" then
        status = "connected to " .. tostring(event.data.host)
    end
end

function on_query(method, args_json)
    if method == "get_status" then
        return '{"status":"' .. status .. '"}'
    end
    return "null"
end

function teardown()
    app.log("info", "Status Monitor shutting down")
end
```

### Lua API Reference

Functions are organized across four global tables: `app`, `ui`, `session`, and `net`.

**`app` -- Core plugin operations:**

| Function | Description |
|----------|-------------|
| `app.log(level, message)` | Log (level: `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"`) |
| `app.register_menu_item(menu, label, action, keybind?)` | Add a menu item (keybind e.g. `"cmd+shift+j"`) |
| `app.register_command(...)` | Convenience alias for menu commands (defaults to `"Tools"` when menu is omitted) |
| `app.register_service(name)` | Register as a named service for inter-plugin queries |
| `app.subscribe(event_type)` | Subscribe to bus events |
| `app.publish(event_type, data)` | Publish a bus event (data is a Lua table, serialized to JSON) |
| `app.get_config(key)` | Read persisted config value (returns string or nil) |
| `app.set_config(key, value)` | Write persisted config value |
| `app.notify(title, body, level?, duration_ms?)` | Show a toast notification (level: `"info"`, `"success"`, `"warn"`, `"error"`) |
| `app.set_status(text?, level?, progress?)` | Update global status bar (`level`: `"info"`, `"warn"`, `"error"`, `"success"`; `progress < 0` hides progress) |
| `app.clipboard(text)` | Copy text to system clipboard |
| `app.clipboard_get()` | Get clipboard text (returns string or nil) |
| `app.get_theme()` | Get current theme JSON string (or nil if unavailable) |
| `app.query_plugin(target, method, args?)` | RPC query to another plugin (args is a Lua table or nil; returns string or nil) |

**`ui` -- Dialogs and panel widgets:**

| Function | Description |
|----------|-------------|
| `ui.prompt(message, default?)` | Blocking text input dialog (returns string or nil) |
| `ui.confirm(message)` | Blocking Yes/No dialog (returns boolean) |
| `ui.alert(title, message)` | Blocking alert dialog |
| `ui.error(title, message)` | Blocking error dialog |
| `ui.form(title, fields)` | Multi-field form dialog (returns table or nil) |
| `ui.panel_*` functions | See [Panel Widget Functions](#panel-widget-functions) below |

**`session` -- Terminal and system operations:**

| Function | Description |
|----------|-------------|
| `session.platform()` | Get current OS platform (returns `"macos"`, `"linux"`, `"windows"`, or `"unknown"`) |
| `session.current()` | Get info about the active session (always includes `platform` and `type`; may include `window_label`, `pane_id`, `key`, and SSH fields like `host`, `user`, `port`) |
| `session.exec_local(command)` | Run a local host shell command (returns table with `stdout`, `stderr`, `exit_code`, `status`) |
| `session.exec_active(command)` | Run a command on the active session (SSH pane = remote exec, local pane = local exec; returns table with `stdout`, `stderr`, `exit_code`, `status`) |
| `session.exec(command)` | Backward-compatible alias for `session.exec_local(command)` |
| `session.write(text)` | Write text to the focused terminal PTY |
| `session.new_tab(command?, plain?)` | Open a new tab (plain=true uses OS default shell) |

> **Note:** `session.platform()` is a function call, not a property access. Use `session.platform()` (with parentheses).

```lua
-- Always runs on the host machine:
local local_result = session.exec_local("uname -a")

-- Runs against the active pane:
-- - active SSH pane => remote command over SSH
-- - active local pane => local host command
local active_result = session.exec_active("pwd")
```

**`net` -- Network operations:**

| Function | Description |
|----------|-------------|
| `net.time()` | Current Unix timestamp (float, seconds since epoch) |
| `net.resolve(hostname)` | DNS lookup (returns array of IP address strings) |
| `net.scan(host, ports, timeout_ms?, concurrency?)` | TCP port scan (returns array of `{port, open}` tables) |

### Panel Widget Functions

For tool-window plugins (`plugin-type: tool_window`), use `ui.panel_*` functions inside `render()` to build the widget tree:

```lua
-- plugin-type: tool_window
-- plugin-location: left

function render()
    ui.panel_heading("My Panel")
    ui.panel_separator()
    ui.panel_label("Hello from Lua!")
    ui.panel_kv("Status", "OK")
    ui.panel_button("refresh", "Refresh")
end
```

**Display widgets:**

| Function | Arguments | Description |
|----------|-----------|-------------|
| `ui.panel_heading(text)` | text | Section heading |
| `ui.panel_label(text, style?)` | text, style | Styled label (style: `"secondary"`, `"muted"`, `"accent"`, `"warn"`, `"error"`) |
| `ui.panel_text(text)` | text | Monospace text |
| `ui.panel_scroll_text(id, text, max_height?)` | id, text, max_height | Scrollable log output |
| `ui.panel_kv(key, value)` | key, value | Key-value row |
| `ui.panel_separator()` | -- | Horizontal rule |
| `ui.panel_spacer(size?)` | size | Spacing (nil = flexible fill) |
| `ui.panel_icon_label(icon, text, style?)` | icon, text, style | Icon + label combination |
| `ui.panel_badge(text, variant)` | text, variant | Status badge (`"info"`, `"success"`, `"warn"`, `"error"`) |
| `ui.panel_progress(id, fraction, label?)` | id, fraction, label | Progress bar (0.0-1.0) |
| `ui.panel_image(id?, src, width?, height?)` **(pending)** | id, src, width, height | Image by file path or `data:` URI |

**Interactive widgets:**

| Function | Arguments | Description |
|----------|-----------|-------------|
| `ui.panel_button(id, label, icon?)` | id, label, icon | Clickable button |
| `ui.panel_text_input(id, value, hint?)` | id, value, hint | Single-line text input (submit on Enter) |
| `ui.panel_text_edit(id, value, hint?, lines?)` | id, value, hint, lines | Multi-line text editor |
| `ui.panel_checkbox(id, label, checked)` | id, label, checked | Checkbox toggle |
| `ui.panel_combobox(id, selected, options)` | id, selected, options | Dropdown (options: array of strings or `{value, label}` tables) |

**Complex widgets:**

| Function | Arguments | Description |
|----------|-----------|-------------|
| `ui.panel_table(columns, rows)` | columns, rows | Data table (simple: array of column names + array of row arrays) |
| `ui.panel_tree(id, nodes, selected?)` | id, nodes, selected | Tree view with icons and context menus |
| `ui.panel_toolbar(id?, items)` | id, items | Toolbar with buttons, separators, spacers, and text inputs |
| `ui.panel_path_bar(id, segments)` **(pending)** | id, segments | Clickable breadcrumb path bar |
| `ui.panel_tabs(id, active, tabs)` | id, active, tabs | Tabbed container (active is 0-based index) |

**Layout containers:**

Layout functions take a callback that builds the child widgets:

| Function | Arguments | Description |
|----------|-----------|-------------|
| `ui.panel_horizontal(func, spacing?)` | func, spacing | Horizontal row of widgets |
| `ui.panel_vertical(func, spacing?)` | func, spacing | Vertical column of widgets |
| `ui.panel_scroll_area(func, max_height?)` | func, max_height | Scrollable container |
| `ui.panel_drop_zone(id, label, func?)` **(pending)** | id, label, func | Drag-and-drop target area |

```lua
function render()
    ui.panel_heading("Layout Example")
    ui.panel_horizontal(function()
        ui.panel_button("save", "Save")
        ui.panel_button("cancel", "Cancel")
    end, 8)

    ui.panel_scroll_area(function()
        for i = 1, 50 do
            ui.panel_label("Line " .. i)
        end
    end, 200)
end
```

**Other:**

| Function | Description |
|----------|-------------|
| `ui.panel_clear()` | Clear the widget accumulator (rarely needed; the host clears before each `render()`) |

### Net API

| Function | Description |
|----------|-------------|
| `net.time()` | Current Unix timestamp (float, seconds since epoch) |
| `net.resolve(hostname)` | DNS lookup (returns array of IP address strings, empty on failure) |
| `net.scan(host, ports, timeout_ms?, concurrency?)` | TCP port scan (returns array of `{port, open}` tables for open ports; timeout default: 1000ms) |

---

## API Signatures Reference

This section is a reference index of currently available API signatures from the SDK source.

### Java Signatures

#### `conch.plugin.ConchPlugin`

```java
PluginInfo getInfo();
void setup();
void onEvent(String eventJson);
default String onQuery(String method, String argsJson); // default returns "null"
String render();
void teardown();
```

#### `conch.plugin.PluginInfo`

```java
// Fields
public final String name;
public final String description;
public final String version;
public final String pluginType;    // "action" | "tool_window"
public final String panelLocation; // "none" | "left" | "right" | "bottom"

// Constructors
public PluginInfo(String name, String description, String version,
                  String pluginType, String panelLocation);
public PluginInfo(String name, String description, String version); // action/none

// Factory methods
public static PluginInfo toolWindow(String name, String description,
                                    String version, String defaultZone);
```

#### `conch.plugin.HostApi`

```java
// Permission
public static native boolean checkPermission(String capability);

// Logging
public static native void log(int level, String message);
public static void trace(String message);
public static void debug(String message);
public static void info(String message);
public static void warn(String message);
public static void error(String message);

// Menu
public static native void registerMenuItem(String menu, String label, String action);
public static native void registerMenuItemWithKeybind(String menu, String label, String action, String keybind);
public static void registerCommand(String label, String action);
public static void registerCommand(String label, String action, String keybind);

// Notifications / status
public static native void notify(String title, String body, String level, int durationMs);
public static void notify(String title, String body, String level);
public static native void setStatus(String text, int level, float progress);

// Clipboard / theme / config
public static native void clipboardSet(String text);
public static native String clipboardGet();
public static native String getTheme();
public static native String getConfig(String key);
public static native void setConfig(String key, String value);

// Dialogs / forms
public static native String prompt(String message, String defaultValue);
public static String prompt(String message);
public static native boolean confirm(String message);
public static native void alert(String title, String message);
public static native void showError(String title, String message);
public static native String showForm(String formDescriptorJson);

// Bus / RPC
public static native void subscribe(String eventType);
public static native void publishEvent(String eventType, String dataJson);
public static native String queryPlugin(String target, String method, String argsJson);
public static native void registerService(String name);

// Terminal / session
public static native void writeToPty(String text);
public static native void newTab(String command, boolean plain);
public static native String getActiveSession();
public static native String execActiveSession(String command);
public static void newTab();
public static void newPlainTab(String command);
public static String platform();
public static String execLocal(String command);

// Net helpers
public static double time();
public static String[] resolve(String host);
public static ScanResult[] scan(String host, int[] ports, Integer timeoutMs);

public static final class ScanResult {
    public final int port;
    public final boolean open;
    public ScanResult(int port, boolean open);
}
```

#### `conch.plugin.Widgets`

```java
// Layout
public Widgets horizontal(Widgets children);
public Widgets vertical(Widgets children);
public Widgets scrollArea(Float maxHeight, Widgets children);

// Display
public Widgets heading(String text);
public Widgets label(String text);
public Widgets label(String text, String style);
public Widgets text(String text);
public Widgets keyValue(String key, String value);
public Widgets separator();
public Widgets spacer();
public Widgets spacer(float size);
public Widgets badge(String text, String variant);
public Widgets progress(String id, float fraction, String label);

// Interactive
public Widgets button(String id, String label);
public Widgets button(String id, String label, String icon);
public Widgets textInput(String id, String value, String hint);
public Widgets checkbox(String id, String label, boolean checked);

// Raw + serialization
public Widgets raw(String json);
public Widgets html(String content);
public Widgets html(String content, String css);
public String toJson();
```

### Lua Signatures

#### Lua plugin lifecycle hooks

```lua
function setup() end                      -- optional
function on_event(event) end              -- optional
function on_query(method, args_json) end  -- optional; return JSON string
function render() end                     -- optional (tool-window plugins usually implement)
function teardown() end                   -- optional
```

#### `app` table

```lua
app.log(level, message)
app.clipboard(text)
app.clipboard_get() -> string|nil
app.get_theme() -> string|nil
app.publish(event_type, data_table)
app.subscribe(event_type)
app.notify(title, body, level?, duration_ms?)
app.set_status(text?, level?, progress?) -- progress < 0 hides progress bar
app.register_service(name)
app.register_menu_item(menu, label, action, keybind?)

-- Overloads:
app.register_command(label, action)
app.register_command(label, action, keybind?)
app.register_command(menu, label, action, keybind?)

-- Note: 3 args means (label, action, keybind?) under "Tools".
-- Use 4 args to specify a menu name.
-- Example: app.register_command("Plugins", "Open Monitor", "open_monitor", nil)

app.query_plugin(target, method, args?) -> string|nil
app.get_config(key) -> string|nil
app.set_config(key, value)
```

#### `ui` table

```lua
ui.panel_clear()

-- Display
ui.panel_heading(text)
ui.panel_label(text, style?)
ui.panel_text(text)
ui.panel_scroll_text(id, text, max_height?)
ui.panel_kv(key, value)
ui.panel_separator()
ui.panel_spacer(size?)
ui.panel_icon_label(icon, text, style?)
ui.panel_badge(text, variant)
ui.panel_progress(id, fraction, label?)
ui.panel_image(id?, src, width?, height?)

-- Interactive
ui.panel_button(id, label, icon?)
ui.panel_text_input(id, value, hint?)
ui.panel_text_edit(id, value, hint?, lines?)
ui.panel_checkbox(id, label, checked)
ui.panel_combobox(id, selected, options)

-- Complex
ui.panel_table(columns, rows)
ui.panel_tree(id, nodes, selected?)
ui.panel_toolbar(id?, items)
ui.panel_path_bar(id, segments)
ui.panel_tabs(id, active, tabs)
ui.panel_html(content, css?)

-- Layout containers (callback receives no args)
ui.panel_horizontal(func, spacing?)
ui.panel_vertical(func, spacing?)
ui.panel_scroll_area(func, max_height?)
ui.panel_drop_zone(id, label, func?)

-- Render control
ui.request_render()              -- push current widgets to frontend immediately

-- Dialogs
ui.form(title, fields) -> table|nil
ui.alert(title, message)
ui.error(title, message)
ui.confirm(message) -> boolean
ui.prompt(message, default?) -> string|nil
```

#### `session` table

```lua
session.platform() -> "macos"|"linux"|"windows"|"unknown"
session.exec_local(command) -> {stdout, stderr, exit_code, status}
session.exec(command) -> {stdout, stderr, exit_code, status}          -- alias of exec_local
session.exec_active(command) -> {stdout, stderr, exit_code, status}
session.current() -> table
session.write(text)
session.new_tab(command?, plain?)
```

#### `net` table

```lua
net.time() -> number
net.resolve(hostname) -> string[]
net.scan(host, ports, timeout_ms?, concurrency?) -> { {port=number, open=true}, ... }
```

---

## Widget System

Both plugin tiers share the same declarative widget system. Plugins return a JSON array of widget objects, and the host renders them as HTML in the webview. Each widget has a `"type"` field that determines its kind, and additional fields for configuration.

> **Note:** Some widget types are defined in the SDK but not yet supported by the webview renderer. These are marked with **(pending)** below. Using them will display `[unknown widget: ...]` until renderer support is added.
>
> **Verified renderer subset:** the current webview renderer supports `heading`, `label`, `text`, `scroll_text`, `key_value`, `separator`, `spacer`, `icon_label`, `badge`, `progress`, `button`, `text_input`, `text_edit`, `checkbox`, `combo_box`, `toolbar`, `tree_view`, `table`, `horizontal`, `vertical`, `scroll_area`, `tabs`, and `html`.

### Layout Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `horizontal` | `id?`, `children`, `spacing?`, `centered?` | Horizontal row of child widgets |
| `vertical` | `id?`, `children`, `spacing?` | Vertical column of child widgets |
| `split_pane` **(pending)** | `id`, `direction`, `ratio`, `resizable`, `left`, `right` | Two-pane split with adjustable ratio |
| `scroll_area` | `id?`, `max_height?`, `children` | Scrollable container |
| `tabs` | `id`, `active`, `tabs: [{label, icon?, children}]` | Tabbed container (active is 0-based index) |
| `drop_zone` **(pending)** | `id`, `label`, `children` | Drag-and-drop target area |
| `context_menu` **(pending)** | `child`, `items: [{id, label, icon?, enabled?, shortcut?}]` | Wraps a widget with a right-click menu |

**SplitPane details:**
- `direction`: `"horizontal"` or `"vertical"`
- `ratio`: float 0.0-1.0 (e.g. 0.5 = equal split)
- `resizable`: boolean, whether the user can drag to resize
- `left` / `right`: single child widget each (the two panes)

### Display Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `heading` | `text` | Section heading (large, bold) |
| `label` | `text`, `style?` | Styled text label |
| `text` | `text` | Monospace text |
| `scroll_text` | `id`, `text`, `max_height?` | Scrollable monospace text (sticks to bottom, useful for logs) |
| `key_value` | `key`, `value` | Key-value pair (label left, value right) |
| `separator` | -- | Horizontal rule |
| `spacer` | `size?` | Spacing (nil/null = flexible fill, otherwise points) |
| `icon_label` | `icon`, `text`, `style?` | Icon + label combination |
| `badge` | `text`, `variant` | Status badge |
| `progress` | `id`, `fraction`, `label?` | Progress bar (fraction: 0.0-1.0) |
| `image` **(pending)** | `id?`, `src`, `width?`, `height?` | Image by file path or `data:` URI |

**Style values** (for `label` and `icon_label`): `"normal"`, `"secondary"`, `"muted"`, `"accent"`, `"warn"`, `"error"`.

**Badge variants**: `"info"`, `"success"`, `"warn"`, `"error"`.

### Interactive Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `button` | `id`, `label`, `icon?`, `enabled?` | Clickable button |
| `text_input` | `id`, `value`, `hint?`, `submit_on_enter?`, `request_focus?` | Single-line text input |
| `text_edit` | `id`, `value`, `hint?`, `lines?` | Multi-line text editor |
| `checkbox` | `id`, `label`, `checked` | Checkbox toggle |
| `combo_box` | `id`, `selected`, `options: [{value, label}]` | Dropdown selection |

### Complex Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `toolbar` | `id?`, `items` | Toolbar with buttons, separators, spacers, and text inputs |
| `path_bar` **(pending)** | `id`, `segments` | Clickable breadcrumb path bar (segments: array of strings) |
| `tree_view` | `id`, `nodes`, `selected?` | Hierarchical tree with icons, badges, and context menus |
| `table` | `id`, `columns`, `rows`, `sort_column?`, `sort_ascending?`, `selected_row?` | Sortable, selectable data table with context menus |
| `html` | `content`, `css?` | Raw HTML rendered in a Shadow DOM with theme variable access |

**Toolbar items** (each has a `"type"` field):

| Item Type | Fields | Description |
|-----------|--------|-------------|
| `button` | `id`, `icon?`, `label?`, `tooltip?`, `enabled?` | Toolbar button |
| `separator` | -- | Visual separator |
| `spacer` | -- | Flexible space |
| `text_input` | `id`, `value`, `hint?` | Inline text input |

**TreeNode fields:**

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique node identifier |
| `label` | string | Display text |
| `icon?` | string | Icon name (see [Icons](#icons)) |
| `icon_color?` | string | Color hint (e.g. `"blue"`, `"muted"`) |
| `bold?` | boolean | Render label in bold |
| `badge?` | string | Status badge text |
| `expanded?` | boolean | Whether the node is expanded |
| `children` | array | Child TreeNode objects |
| `context_menu?` | array | Array of ContextMenuItem objects |

**TableColumn fields:**

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Column identifier |
| `label` | string | Header text |
| `sortable?` | boolean | Whether clicking the header sorts |
| `width?` | float | Column width in points (null = auto) |
| `visible?` | boolean | Whether the column is visible (default: true) |

**TableRow fields:**

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Row identifier |
| `cells` | array | Array of cell values (string or `{text, icon?, badge?}`) |
| `context_menu?` | array | Array of ContextMenuItem objects. Defined in the SDK, but row context menus are not yet rendered in the current webview implementation. |

**ContextMenuItem fields:**

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Action identifier (delivered in events) |
| `label` | string | Menu item text |
| `icon?` | string | Icon name |
| `enabled?` | boolean | Whether the item is clickable (default: true) |
| `shortcut?` | string | Keyboard shortcut hint text (display only) |

> **Current renderer note for tables:** rich table cells currently render `text` and optional `icon`. The `badge` field exists in the shared schema, but the current webview renderer does not display a badge inside table cells yet.

**Html widget details:**

The `html` widget renders raw HTML inside a Shadow DOM for full CSS isolation. Theme variables (`--bg`, `--fg`, `--green`, `--red`, etc.) are forwarded into the shadow root so plugin styles stay on-theme. Elements with a `data-action="action_id"` attribute emit `button_click` events back to the plugin when clicked.

| Field | Type | Description |
|-------|------|-------------|
| `content` | string | Raw HTML string to render inside the shadow root |
| `css?` | string | Optional CSS injected into the shadow root's `<style>` |

Works in tool-window plugins.

---

## Widget Events

Events generated by interactive widgets. These are delivered to the plugin wrapped in a `PluginEvent` envelope with `"kind": "widget"`.

> **Current renderer note:** events tied to pending widgets or not-yet-wired interactions are part of the shared event schema, but are not emitted by the current webview renderer. Today that includes `tree_context_menu`, `table_context_menu`, `table_header_context_menu`, `path_bar_navigate`, `drop`, and `context_menu_action`.

| Event Type | Fields | Trigger |
|------------|--------|---------|
| `button_click` | `id` | Button pressed |
| `text_input_changed` | `id`, `value` | Text input value changed (debounced) |
| `text_input_submit` | `id`, `value` | Enter pressed in text input |
| `text_input_arrow_down` | `id` | Arrow-down pressed in text input |
| `text_input_arrow_up` | `id` | Arrow-up pressed in text input |
| `text_edit_changed` | `id`, `value` | Multi-line text editor changed |
| `checkbox_changed` | `id`, `checked` | Checkbox toggled |
| `combo_box_changed` | `id`, `value` | Dropdown selection changed |
| `tree_select` | `id`, `node_id` | Tree node selected (single click) |
| `tree_activate` | `id`, `node_id` | Tree node double-clicked |
| `tree_toggle` | `id`, `node_id`, `expanded` | Tree node expanded or collapsed |
| `tree_context_menu` | `id`, `node_id`, `action` | Context menu action on a tree node |
| `table_select` | `id`, `row_id` | Table row selected |
| `table_activate` | `id`, `row_id` | Table row double-clicked |
| `table_sort` | `id`, `column`, `ascending` | Column header clicked (sort) |
| `table_context_menu` | `id`, `row_id`, `action` | Context menu action on a table row |
| `table_header_context_menu` | `id`, `column` | Right-click on a table column header |
| `tab_changed` | `id`, `active` | Tab switched (active is 0-based index) |
| `path_bar_navigate` | `id`, `segment_index` | Breadcrumb segment clicked (0-based index) |
| `drop` | `id`, `source?`, `items` | Items dropped onto a DropZone |
| `toolbar_input_submit` | `id`, `value` | Toolbar text input submitted |
| `context_menu_action` | `action` | Standalone context menu action triggered |

---

## Plugin Events

All events are delivered to plugins wrapped in a top-level `PluginEvent` envelope with a `"kind"` field that identifies the event category.

### Event Kinds

| Kind | Fields | Description |
|------|--------|-------------|
| `widget` | (nested widget event fields) | A widget interaction from plugin UI. Contains widget fields (e.g. `type`, `id`, `value`). |
| `menu_action` | `action` | A menu item registered by this plugin was clicked |
| `bus_event` | `event_type`, `data` | An inter-plugin pub/sub event |
| `bus_query` | `method`, `args` | Direct RPC queries are routed to query callbacks (`on_query` / `onQuery`) rather than `on_event` |
| `theme_changed` | `theme_json` | Reserved event kind in the shared schema (not currently emitted by the host) |
| `shutdown` | -- | Reserved event kind in the shared schema (plugins are currently shut down via `teardown()`) |

### Event JSON Examples

```json
{ "kind": "menu_action", "action": "do_something" }
{ "kind": "widget", "type": "button_click", "id": "my_button" }
{ "kind": "widget", "type": "tree_context_menu", "id": "tree1", "node_id": "srv1", "action": "delete" }
{ "kind": "bus_event", "event_type": "ssh.connected", "data": { "host": "10.0.0.1" } }
{ "kind": "theme_changed", "theme_json": "{...}" }
{ "kind": "shutdown" }
```

**Java** plugins receive these as JSON strings in `onEvent()` and must parse them.

**Lua** plugins receive a **native Lua table** -- access fields directly:

```lua
function on_event(event)
    if event.kind == "menu_action" then
        app.log("info", "Action: " .. event.action)
    elseif event.kind == "widget" then
        if event.type == "button_click" then
            app.log("info", "Button: " .. event.id)
        elseif event.type == "tree_context_menu" then
            app.log("info", "Tree action: " .. event.action .. " on " .. event.node_id)
        end
    elseif event.kind == "bus_event" then
        app.log("info", "Bus event: " .. event.event_type)
    end
end
```

> **Note on `bus_query` handling:** Direct queries are handled by dedicated query callbacks rather than `on_event`:
> - Lua: `on_query(method, args_json)`
> - Java: `onQuery(String method, String argsJson)`

---

## Render Lifecycle & Architecture

This section explains the internal rendering pipeline — how widgets get from plugin code to the screen, how re-renders are triggered, and how the push/pull mechanisms work. Understanding this is essential for plugin developers who need loading states or immediate UI updates.

### Plugin Types & Rendering Models

| Plugin Type | Registration | Render Trigger | Container |
|-------------|-------------|----------------|-----------|
| **Tool Window** | Auto-registered on load via `register_panel()` | Pull (frontend requests) + Push (`ui.request_render()`) | Sidebar panel (left/right) or bottom panel |
| **Action** | Event-driven (no persistent UI) | N/A | No persistent container |

### The Widget Pipeline

Both plugin types follow the same pipeline:

```
Plugin code (Lua/Java)
    ↓  calls ui.panel_*() functions
Widget Accumulator (in-memory list)
    ↓  serialized to JSON
Rust backend (HostApi / Tauri commands)
    ↓  emits Tauri event or returns via oneshot channel
Frontend JS (plugin-widgets.js)
    ↓  renderWidgets() → renderWidget() → DOM elements
Browser (webview)
```

### Pull-Based Rendering (RenderRequest)

The default mechanism. The frontend asks the plugin to render, and the plugin responds with a widget tree.

```
Frontend                         Rust Backend                    Plugin Thread
   │                                │                               │
   │─ invoke('request_plugin_render')─▶│                               │
   │                                │─ PluginMail::RenderRequest ──▶│
   │                                │                               │─ clear accumulator
   │                                │                               │─ call render()
   │                                │                               │─ ui.panel_*() fills accumulator
   │                                │◀── JSON widget array ─────────│
   │◀── widget JSON ───────────────│                               │
   │─ renderWidgets(container, json)│                               │
```

**When does this happen for tool-window plugins?**

- **Initial load:** When a tool-window plugin is first registered, the frontend does one `request_plugin_render` call.
- **After widget events:** When the user interacts with a widget (button click, text input, etc.), the frontend sends the event to the plugin, then automatically requests a fresh render.
### Push-Based Rendering (set_widgets / request_render)

Plugins can push widget updates to the frontend at any time — useful for showing loading states during blocking operations.

```
Plugin Thread                    Rust Backend                    Frontend
   │                                │                               │
   │─ ui.request_render() ────────▶│                               │
   │   (serializes accumulator)     │─ emit('plugin-widgets-updated')▶│
   │                                │                               │─ renderWidgets()
```

**How it works internally:**

1. `ui.request_render()` reads the current widget accumulator (without draining it)
2. Serializes widgets to JSON
3. Calls `HostApi::set_widgets(panel_handle, json)`
4. `TauriHostApi` emits a `plugin-widgets-updated` Tauri event
5. Frontend listener catches the event and re-renders the container

**Example — loading state during a blocking operation:**

```lua
function switch_env(env)
    switching = true
    render()                -- rebuild widgets (now shows loading spinner)
    ui.request_render()     -- push to frontend immediately

    local r = session.exec_local(cmd)  -- blocks for several seconds
    switching = false

    refresh()               -- update state variables
    render()                -- rebuild widgets (now shows new state)
    ui.request_render()     -- push final state to frontend
end
```

> **Note:** `ui.request_render()` applies to tool-window plugin UI.

### Auto Re-render After Events

When a user interacts with a widget, the frontend handles re-rendering automatically:

**Tool-window plugins:** `sendEvent()` → `plugin_widget_event` (Tauri command) → `on_event()` runs → frontend calls `refreshPanelPlugin()` → `request_plugin_render` → `render()` → update DOM.

In both cases, plugins **do not need to call `ui.request_render()`** after handling events — the re-render happens automatically. Use `ui.request_render()` only when you need to push an update **during** a long-running operation (before the event handler returns).

### Key Source Files

| File | Role |
|------|------|
| `crates/conch_plugin/src/lua/runner.rs` | Lua plugin thread, mailbox loop, `handle_render()` |
| `crates/conch_plugin/src/lua/api/ui.rs` | `ui.panel_*` Lua bindings, `ui.request_render()` |
| `crates/conch_plugin/src/lua/api/mod.rs` | Widget accumulator, HostApi bridge, PanelHandleStore |
| `crates/conch_plugin/src/host_api.rs` | `HostApi` trait definition (`set_widgets`, `register_panel`) |
| `crates/conch_tauri/src/plugins/tauri_host_api.rs` | `TauriHostApi` — emits Tauri events for widget updates |
| `crates/conch_tauri/src/plugins/mod.rs` | `request_plugin_render` command |
| `crates/conch_tauri/frontend/plugin-widgets.js` | Frontend renderer, `sendEvent()`, `refreshPanelPlugin()` |

---

## Form Dialogs

Both tiers share the same form JSON format. The dialog blocks until the user submits or cancels.

### Form Field Types

| Type | Fields | Description |
|------|--------|-------------|
| `text` | `id`, `label`, `value?`, `hint?` | Single-line text input |
| `password` | `id`, `label`, `value?` | Masked password input |
| `number` | `id`, `label`, `value?` | Numeric input |
| `combo` | `id`, `label`, `options[]`, `value?` | Dropdown select |
| `checkbox` | `id`, `label`, `value?` | Boolean toggle |
| `host_port` | `host_id`, `port_id`, `label`, `host_value?`, `port_value?` | Host + port row |
| `file_picker` | `id`, `label`, `value?` | File path input |
| `collapsible` | -- | Collapsible section |
| `separator` | -- | Horizontal rule |
| `label` | `text` | Read-only text |

### Example

```java
String result = HostApi.showForm("""
    {
        "title": "Connection Settings",
        "fields": [
            {"type": "text", "id": "host", "label": "Hostname"},
            {"type": "number", "id": "port", "label": "Port", "value": 22},
            {"type": "password", "id": "password", "label": "Password"},
            {"type": "combo", "id": "auth", "label": "Auth Method",
             "options": ["Password", "SSH Key"], "value": "Password"}
        ]
    }
    """);
if (result != null) {
    // result = {"host":"...", "port":"22", "password":"...", "auth":"Password", "_action":"ok"}
}
```

Lua equivalent:

```lua
local result = ui.form("Connection Settings", {
    { type = "text", id = "host", label = "Hostname" },
    { type = "number", id = "port", label = "Port", value = 22 },
    { type = "password", id = "password", label = "Password" },
    { type = "combo", id = "auth", label = "Auth Method",
      options = {"Password", "SSH Key"}, value = "Password" },
})
if result then
    local host = result.host
end
```

---

## Inter-Plugin Communication

Conch provides two mechanisms for plugins to communicate: pub/sub events for broadcasting, and RPC queries for direct request/response.

### Pub/Sub Events

Subscribe to event types and publish events. Events are broadcast to all subscribers (except the publisher itself). Subscriptions and publishing use a central message bus.

```java
// Java
HostApi.subscribe("my.event_type");
HostApi.publishEvent("my.event_type", "{\"key\": \"value\"}");
```

```lua
-- Lua
app.subscribe("my.event_type")
app.publish("my.event_type", { key = "value" })
```

Received events arrive as `bus_event` plugin events:

```lua
function on_event(event)
    if event.kind == "bus_event" and event.event_type == "my.event_type" then
        app.log("info", "Received: " .. tostring(event.data.key))
    end
end
```

### RPC Queries

Plugins can send direct queries to other plugins and receive a response. The target plugin must be loaded and can be addressed either by plugin name or by service name.

**Lua (sender):**

```lua
local result = app.query_plugin("Other Plugin", "method_name", { arg1 = "value" })
-- result is a JSON string, or nil if the target plugin was not found
```

**Java (sender):**

```java
String result = HostApi.queryPlugin("Other Plugin", "method_name", "{\"arg1\":\"value\"}");
```

**Lua (receiver):**

```lua
function on_query(method, args_json)
    if method == "method_name" then
        return '{"result": "success"}'
    end
    return "null"
end
```

**Java (receiver):**

```java
@Override
public String onQuery(String method, String argsJson) {
    if ("method_name".equals(method)) {
        return "{\"result\":\"success\"}";
    }
    return "null";
}
```

### Service Registry

Plugins can register themselves as named services. This allows other plugins to discover services by name rather than requiring knowledge of the specific plugin name.

```lua
-- Provider plugin
app.register_service("database")

-- Consumer plugin
local result = app.query_plugin("database", "get_all", {})
```

```java
// Provider plugin
HostApi.registerService("database");

// Consumer plugin
String result = HostApi.queryPlugin("database", "get_all", "{}");
```

---

## Icons

Plugins can reference built-in icons by name in widget fields like `icon`, `TreeNode.icon`, and `ToolbarItem.icon`. These are 16x16 PNGs with theme-aware variants (the host selects the correct variant automatically).

| Icon Name | Description |
|-----------|-------------|
| `file` | File icon |
| `folder` | Closed folder |
| `folder-open` | Open folder |
| `folder-new` | New folder |
| `server` | Server/rack |
| `network-server` | Network server |
| `terminal` | Terminal/console |
| `computer` | Computer/monitor |
| `tab-sessions` | Sessions tab |
| `tab-files` | Files tab |
| `tab-tools` | Tools tab |
| `tab-macros` | Macros tab |
| `tab-close` | Close tab (X) |
| `go-down` | Down arrow |
| `go-up` | Up arrow |
| `go-home` | Home/house |
| `go-previous` | Back arrow |
| `go-next` | Forward arrow |
| `refresh` | Refresh/reload |
| `sidebar-folder` | Sidebar folder |
| `transfer-down` | Download |
| `transfer-up` | Upload |
| `locked` | Locked padlock |
| `unlocked` | Unlocked padlock |
| `eye` | Eye/view (show/hide toggle) |

---

## Plugin Search Paths

Conch scans these directories for plugins:

1. `target/debug/` and `target/release/` (development)
2. Executable directory and `plugins/` subdirectory
3. `~/.config/conch/plugins/` (user plugins)
4. Custom paths from `[conch.plugins] search_paths` in `config.toml`

**File extensions:**
- Java: `.jar` (must have `Plugin-Class` in `META-INF/MANIFEST.MF`)
- Lua: `.lua` (must have `-- plugin-name:` metadata comment)

Plugins are **not loaded automatically** -- use **Settings > Plugins** to enable them. Enabled plugins are remembered across restarts.
