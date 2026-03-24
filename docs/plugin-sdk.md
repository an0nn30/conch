# Conch Plugin SDK

Conch supports two plugin tiers:

| Tier | Language | Use Case | Build Step |
|------|----------|----------|------------|
| **Java** | Java, Kotlin, Scala, Groovy | Community plugins, rich UI, familiar ecosystem | Compile to `.jar` |
| **Lua** | Lua | Quick scripts, personal automation, no build step | Single `.lua` file |

Plugins are managed via **Settings > Plugins** -- scan, enable, disable, and persist across restarts.

## Table of Contents

- [Java Plugins](#java-plugins)
  - [Quick Start](#java-quick-start)
  - [Project Setup (Gradle)](#project-setup-gradle)
  - [ConchPlugin Interface](#conchplugin-interface)
  - [HostApi Reference](#java-hostapi)
  - [Widget Builder](#widget-builder)
  - [Handling Events](#handling-events-java)
  - [Panel Plugins](#panel-plugins-java)
- [Lua Plugins](#lua-plugins)
  - [Quick Start](#lua-quick-start)
  - [Lua Metadata Fields](#lua-metadata-fields)
  - [Lua Plugin Lifecycle](#lua-plugin-lifecycle)
  - [Lua API Reference](#lua-api-reference)
  - [Panel Widget Functions](#panel-widget-functions)
  - [Net API](#net-api)
- [Widget System](#widget-system)
- [Widget Events](#widget-events)
- [Plugin Events](#plugin-events)
- [Form Dialogs](#form-dialogs)
- [Inter-Plugin Communication](#inter-plugin-communication)
- [Icons](#icons)
- [Plugin Search Paths](#plugin-search-paths)

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
        attributes 'Plugin-Class': 'com.example.MyPlugin'
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
        return "[]"; // No panel widgets
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
        attributes 'Plugin-Class': 'com.example.MyPlugin'
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
>         attributes 'Plugin-Class': 'com.example.MyPlugin'
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
| `PluginInfo getInfo()` | Return plugin metadata (name, version, type, panel location) |
| `void setup()` | Called once on plugin load. Register menu items, initialize state. |
| `void onEvent(String eventJson)` | Handle events -- menu clicks, widget interactions, bus events. |
| `String render()` | Return widget tree as JSON array. Called on demand for panel plugins. |
| `void teardown()` | Clean up resources before unload. |

#### Plugin Types

```java
// Action plugin — no panel, interacts via menu items and events.
new PluginInfo("My Tool", "Does things", "1.0.0");

// Panel plugin — renders widgets in a sidebar or bottom panel.
new PluginInfo("My Panel", "Shows info", "1.0.0", "panel", "bottom");
```

Panel locations: `"left"`, `"right"`, `"bottom"`.

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

**Notifications:**

| Method | Description |
|--------|-------------|
| `notify(String title, String body, String level, int durationMs)` | Show a toast notification (level: `"info"`, `"success"`, `"warn"`, `"error"`) |
| `notify(String title, String body, String level)` | Show notification with default duration |

**Status Bar:**

| Method | Description |
|--------|-------------|
| `setStatus(String text, int level, float progress)` | Update the global status bar. Level: 0=info, 1=warn, 2=error, 3=success. Progress: 0.0-1.0 for a progress bar, negative to hide. Pass null text to clear. |

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

**Terminal / Tabs:**

| Method | Description |
|--------|-------------|
| `writeToPty(String text)` | Write text to the focused terminal (include `\n` for Enter) |
| `newTab(String command, boolean plain)` | Open a new tab (plain=true bypasses terminal.shell config) |
| `newTab()` | Open a new tab with default shell |
| `newPlainTab(String command)` | Open a plain shell tab and run a command |

> **Note:** The Java SDK does not currently expose `queryPlugin`, `registerService`, or `getTheme` methods. These are available only in the Lua API.

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

### Panel Plugins (Java)

Set `pluginType` to `"panel"` and specify a location:

```java
@Override
public PluginInfo getInfo() {
    return new PluginInfo("My Panel", "Shows info", "1.0.0", "panel", "right");
}

@Override
public String render() {
    return new Widgets()
        .heading("My Panel")
        .label("Hello from Java!")
        .toJson();
}
```

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
| `-- plugin-type: action` | No | `"action"` (default) or `"panel"` |
| `-- plugin-location: left` | No | Panel location: `"left"` (default for panel plugins), `"right"`, `"bottom"` |
| `-- plugin-icon: icon.png` | No | Custom icon for the panel tab (filename relative to plugin location) |
| `-- plugin-keybind: action = binding \| Description` | No | Register a keyboard shortcut (repeatable) |

**Keybind format:** `action_id = key_combo | Optional description`

- `action_id` is the action string delivered in the event
- `key_combo` uses the same format as menu keybinds (e.g., `cmd+shift+i`)
- The description after `|` is optional

```lua
-- plugin-name: System Info
-- plugin-description: Live system information panel
-- plugin-type: panel
-- plugin-version: 1.3.0
-- plugin-location: right
-- plugin-icon: system-info.png
-- plugin-keybind: open_panel = cmd+shift+i | Toggle System Info panel
-- plugin-keybind: refresh = cmd+r
```

### Lua Plugin Lifecycle

Each Lua plugin runs on a dedicated OS thread with its own Lua VM. The host manages the lifecycle through five optional functions that the plugin can define at the global scope:

| Function | When Called | Description |
|----------|------------|-------------|
| `setup()` | Once, after the plugin source is loaded | Initialize state, register menu items, subscribe to events |
| `render()` | On demand, for panel plugins | Build the widget tree using `ui.panel_*` functions |
| `on_event(event)` | When any event targets this plugin | Handle widget interactions, menu actions, bus events. The `event` argument is a native Lua table. |
| `on_query(method, args_json)` | When another plugin sends an RPC query | Handle inter-plugin queries. `args_json` is a JSON string. Return a JSON string as the response. |
| `teardown()` | When the plugin is unloaded or the app shuts down | Clean up resources |

All functions are optional. A minimal plugin only needs `setup()`. Panel plugins need `render()`. Plugins that respond to RPC queries need `on_query()`.

**Example panel plugin with full lifecycle:**

```lua
-- plugin-name: Status Monitor
-- plugin-type: panel
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
| `app.register_service(name)` | Register as a named service for inter-plugin queries |
| `app.subscribe(event_type)` | Subscribe to bus events |
| `app.publish(event_type, data)` | Publish a bus event (data is a Lua table, serialized to JSON) |
| `app.get_config(key)` | Read persisted config value (returns string or nil) |
| `app.set_config(key, value)` | Write persisted config value |
| `app.notify(title, body, level?, duration_ms?)` | Show a toast notification (level: `"info"`, `"success"`, `"warn"`, `"error"`) |
| `app.clipboard(text)` | Copy text to system clipboard |
| `app.clipboard_get()` | Get clipboard text (returns string or nil) |
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
| `session.current()` | Get info about the active session (returns table with `platform` and `type` fields) |
| `session.exec(command)` | Run a local shell command (returns table with `stdout`, `stderr`, `exit_code`, `status`) |
| `session.write(text)` | Write text to the focused terminal PTY |
| `session.new_tab(command?, plain?)` | Open a new tab (plain=true uses OS default shell) |

> **Note:** `session.platform()` is a function call, not a property access. Use `session.platform()` (with parentheses).

**`net` -- Network operations:**

| Function | Description |
|----------|-------------|
| `net.time()` | Current Unix timestamp (float, seconds since epoch) |
| `net.resolve(hostname)` | DNS lookup (returns array of IP address strings) |
| `net.scan(host, ports, timeout_ms?, concurrency?)` | TCP port scan (returns array of `{port, open}` tables) |

### Panel Widget Functions

For panel plugins (`plugin-type: panel`), use `ui.panel_*` functions inside `render()` to build the widget tree:

```lua
-- plugin-type: panel
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

## Widget System

Both plugin tiers share the same declarative widget system. Plugins return a JSON array of widget objects, and the host renders them as HTML in the webview. Each widget has a `"type"` field that determines its kind, and additional fields for configuration.

> **Note:** Some widget types are defined in the SDK but not yet supported by the webview renderer. These are marked with **(pending)** below. Using them will display `[unknown widget: ...]` until renderer support is added.

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
| `context_menu?` | array | Array of ContextMenuItem objects |

**ContextMenuItem fields:**

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Action identifier (delivered in events) |
| `label` | string | Menu item text |
| `icon?` | string | Icon name |
| `enabled?` | boolean | Whether the item is clickable (default: true) |
| `shortcut?` | string | Keyboard shortcut hint text (display only) |

---

## Widget Events

Events generated by interactive widgets. These are delivered to the plugin wrapped in a `PluginEvent` envelope with `"kind": "widget"`.

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
| `toolbar_input_changed` | `id`, `value` | Toolbar text input changed |
| `context_menu_action` | `action` | Standalone context menu action triggered |

---

## Plugin Events

All events are delivered to plugins wrapped in a top-level `PluginEvent` envelope with a `"kind"` field that identifies the event category.

### Event Kinds

| Kind | Fields | Description |
|------|--------|-------------|
| `widget` | (nested widget event fields) | A widget interaction from one of the plugin's panels. Contains the widget event fields listed above (e.g. `type`, `id`, `value`). |
| `menu_action` | `action` | A menu item registered by this plugin was clicked |
| `bus_event` | `event_type`, `data` | An inter-plugin pub/sub event |
| `bus_query` | `request_id`, `method`, `args` | A direct RPC query from another plugin |
| `theme_changed` | `theme_json` | The host theme changed (color scheme switch) |
| `shutdown` | -- | The plugin is being unloaded |

### Event JSON Examples

```json
{ "kind": "menu_action", "action": "do_something" }
{ "kind": "widget", "type": "button_click", "id": "my_button" }
{ "kind": "widget", "type": "tree_context_menu", "id": "tree1", "node_id": "srv1", "action": "delete" }
{ "kind": "bus_event", "event_type": "ssh.connected", "data": { "host": "10.0.0.1" } }
{ "kind": "bus_query", "request_id": "abc123", "method": "get_status", "args": {} }
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

> **Note on Lua `bus_query` events:** For Lua plugins, direct queries are handled by the separate `on_query(method, args_json)` function rather than through `on_event`. The runner dispatches bus queries to `on_query` automatically.

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

**Lua (receiver):**

```lua
function on_query(method, args_json)
    if method == "method_name" then
        return '{"result": "success"}'
    end
    return "null"
end
```

### Service Registry

Plugins can register themselves as named services. This allows other plugins to discover services by name rather than requiring knowledge of the specific plugin name.

```lua
-- Provider plugin
app.register_service("database")

-- Consumer plugin
local result = app.query_plugin("database", "get_all", {})
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
