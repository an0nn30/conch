# Conch Plugin SDK

Conch supports two plugin tiers:

| Tier | Language | Use Case | Build Step |
|------|----------|----------|------------|
| **Java** | Java, Kotlin, Scala, Groovy | Community plugins, rich UI, familiar ecosystem | Compile to `.jar` |
| **Lua** | Lua | Quick scripts, personal automation, no build step | Single `.lua` file |

Plugins are managed via **Tools > Plugin Manager** — scan, enable, disable, and persist across restarts.

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
  - [Lua API Reference](#lua-api-reference)
  - [Panel Widget Functions](#panel-widget-functions)
  - [Net API](#net-api)
- [Widget System](#widget-system)
- [Widget Events](#widget-events)
- [Plugin Events](#plugin-events)
- [Form Dialogs](#form-dialogs)
- [Inter-Plugin Communication](#inter-plugin-communication)
- [Plugin Search Paths](#plugin-search-paths)

---

## Java Plugins

Java plugins are JAR files loaded by an embedded JVM. Any JVM language works (Java, Kotlin, Scala, Groovy). The SDK JAR is embedded in the Conch binary — no external files needed.

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

Open Conch, go to **Tools > Plugin Manager**, and enable your plugin.

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
> single JAR — external dependencies (like Gson) must be bundled inside it.
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
| `void onEvent(String eventJson)` | Handle events — menu clicks, widget interactions, bus events. |
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

Available builder methods: `heading`, `label`, `text`, `keyValue`, `separator`, `spacer`, `badge`, `progress`, `button`, `textInput`, `checkbox`, `horizontal`, `vertical`, `scrollArea`, `raw`.

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

Lua plugins are single `.lua` files — no compilation, no project setup. Drop a file in the plugins directory and enable it via the Plugin Manager.

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

Metadata is declared in comments at the top. Enable via **Tools > Plugin Manager**.

### Lua API Reference

All functions are on the `app` global table.

| Function | Description |
|----------|-------------|
| `app.log(level, message)` | Log (level: `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"`) |
| `app.register_menu_item(menu, label, action, keybind?)` | Add a menu item |
| `app.register_service(name)` | Register as a named service for inter-plugin queries |
| `app.subscribe(event_type)` | Subscribe to bus events |
| `app.publish(event_type, data)` | Publish a bus event |
| `app.get_config(key)` | Read persisted config value |
| `app.set_config(key, value)` | Write persisted config value |
| `app.notify(title, body, level?, duration_ms?)` | Show a toast notification |
| `app.clipboard(text)` | Copy to clipboard |
| `app.clipboard_get()` | Get clipboard text |
| `app.query_plugin(target, method, args?)` | RPC query to another plugin |
| `ui.prompt(message, default?)` | Blocking text input dialog |
| `ui.confirm(message)` | Blocking Yes/No dialog |
| `ui.alert(title, message)` | Blocking alert dialog |
| `ui.error(title, message)` | Blocking error dialog |
| `ui.form(title, fields)` | Multi-field form dialog |
| `session.exec(command)` | Run a local shell command |
| `session.write(text)` | Write to focused terminal |
| `session.new_tab(command?, plain?)` | Open a new tab |
| `session.platform` | Get current OS platform |

### Net API

| Function | Description |
|----------|-------------|
| `net.time()` | Current Unix timestamp (float) |
| `net.resolve(hostname)` | DNS lookup → array of IP strings |
| `net.scan(host, ports, timeout_ms?, concurrency?)` | TCP port scan |

### Panel Widget Functions

For panel plugins (`plugin-type: panel`), use `ui.panel_*` functions in `render()`:

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

**Display:** `panel_heading`, `panel_label`, `panel_text`, `panel_scroll_text`, `panel_kv`, `panel_separator`, `panel_spacer`, `panel_icon_label`, `panel_badge`, `panel_progress`, `panel_image`

**Interactive:** `panel_button`, `panel_text_input`, `panel_text_edit`, `panel_checkbox`, `panel_combobox`

**Complex:** `panel_table`, `panel_tree`, `panel_toolbar`, `panel_path_bar`, `panel_tabs`

**Layout:** `panel_horizontal`, `panel_vertical`, `panel_scroll_area`, `panel_drop_zone`

---

## Widget System

Both plugin tiers share the same declarative widget system. Plugins return a JSON array of widget objects, and the host renders them as HTML in the webview.

### Layout Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `horizontal` | `id?`, `children`, `spacing?` | Horizontal layout |
| `vertical` | `id?`, `children`, `spacing?` | Vertical layout |
| `scroll_area` | `id?`, `max_height?`, `children` | Scrollable region |
| `tabs` | `id`, `active`, `tabs: [{label, children}]` | Tabbed container |

### Display Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `heading` | `text` | Section heading |
| `label` | `text`, `style?` | Styled text (secondary/muted/accent/warn/error) |
| `text` | `text` | Monospace text |
| `scroll_text` | `id`, `text`, `max_height?` | Scrollable log output |
| `key_value` | `key`, `value` | Key-value row |
| `separator` | — | Horizontal rule |
| `spacer` | `size?` | Spacing |
| `badge` | `text`, `variant` | Status badge (info/success/warn/error) |
| `progress` | `id`, `fraction`, `label?` | Progress bar (0.0–1.0) |

### Interactive Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `button` | `id`, `label`, `icon?`, `enabled?` | Clickable button |
| `text_input` | `id`, `value`, `hint?`, `submit_on_enter?` | Single-line text input |
| `text_edit` | `id`, `value`, `hint?`, `lines?` | Multi-line editor |
| `checkbox` | `id`, `label`, `checked` | Toggle |
| `combo_box` | `id`, `selected`, `options: [{value, label}]` | Dropdown |

### Complex Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `toolbar` | `id?`, `items` | Button/separator/input toolbar |
| `tree_view` | `id`, `nodes`, `selected?` | Hierarchical tree with icons |
| `table` | `id`, `columns`, `rows`, `sort_column?`, `sort_ascending?` | Sortable data table |

---

## Widget Events

| Event | Fields | Trigger |
|-------|--------|---------|
| `button_click` | `id` | Button pressed |
| `text_input_changed` | `id`, `value` | Text input changed |
| `text_input_submit` | `id`, `value` | Enter pressed |
| `checkbox_changed` | `id`, `checked` | Checkbox toggled |
| `combo_box_changed` | `id`, `value` | Dropdown changed |
| `tree_select` | `id`, `node_id` | Tree node selected |
| `tree_activate` | `id`, `node_id` | Tree node double-clicked |
| `table_select` | `id`, `row_id` | Table row selected |
| `table_sort` | `id`, `column`, `ascending` | Column header clicked |
| `tab_changed` | `id`, `active` | Tab switched |

---

## Plugin Events

Events are delivered as JSON:

```json
{ "kind": "menu_action", "action": "do_something" }
{ "kind": "widget", "type": "button_click", "id": "my_button" }
{ "kind": "bus_event", "event_type": "ssh.connected", "data": { "host": "..." } }
```

**Java** plugins receive these as JSON strings and must parse them.

**Lua** plugins receive a **native Lua table** — access fields directly:

```lua
function on_event(event)
    if event.kind == "menu_action" then
        app.log("info", "Action: " .. event.action)
    end
end
```

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
| `separator` | — | Horizontal rule |
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

### Pub/Sub Events

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

### RPC Queries

```lua
-- Lua
local result = app.query_plugin("Other Plugin", "method_name", { arg1 = "value" })
```

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

Plugins are **not loaded automatically** — use **Tools > Plugin Manager** to enable them. Enabled plugins are remembered across restarts.
