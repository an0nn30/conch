# Conch Plugin SDK

Conch supports three plugin tiers, each suited to different use cases:

| Tier | Language | Use Case | Build Step |
|------|----------|----------|------------|
| **Native** | Rust, C, Go | Performance-critical, session backends (SSH, SFTP) | Compile to `.dylib`/`.so`/`.dll` |
| **Java** | Java, Kotlin, Scala, Groovy | Community plugins, rich UI, familiar ecosystem | Compile to `.jar` |
| **Lua** | Lua | Quick scripts, personal automation, no build step | Single `.lua` file |

## Table of Contents

- [Java Plugins](#java-plugins)
  - [Quick Start](#java-quick-start)
  - [Project Setup (Gradle)](#project-setup-gradle)
  - [ConchPlugin Interface](#conchplugin-interface)
  - [HostApi Reference](#java-hostapi)
  - [Widget Builder](#widget-builder)
  - [Handling Events](#handling-events-java)
  - [Panel Plugins](#panel-plugins-java)
- [Native Plugins](#native-plugins)
  - [Architecture Overview](#architecture-overview)
  - [Quick Start (Rust)](#quick-start-rust)
  - [Required Exports](#required-exports)
  - [HostApi Reference](#hostapi-reference)
  - [Session Backends](#session-backends)
  - [Examples in Other Languages](#examples-in-other-languages)
- [Lua Plugins](#lua-plugins)
  - [Quick Start](#lua-quick-start)
- [Widget System](#widget-system)
- [Widget Events](#widget-events)
- [Plugin Events](#plugin-events)
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

Open Conch, go to the Plugin Manager, and load your plugin.

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

**Maven:**

```xml
<dependencies>
    <dependency>
        <groupId>conch.plugin</groupId>
        <artifactId>conch-plugin-sdk</artifactId>
        <version>0.3.0</version>
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
| `String render()` | Return widget tree as JSON array. Called every frame for panel plugins. |
| `void teardown()` | Clean up resources before unload. |

#### Plugin Types

```java
// Action plugin — no panel, interacts via menu items and events.
new PluginInfo("My Tool", "Does things", "1.0.0");

// Panel plugin — renders widgets in a sidebar or bottom panel.
new PluginInfo("My Panel", "Shows info", "1.0.0", "panel", "bottom");
```

Panel locations: `"left"`, `"right"`, `"bottom"`, `"none"`.

### Java HostApi

Static methods on `conch.plugin.HostApi`. The Java tier currently supports logging and menu registration. Additional host APIs (panels, dialogs, notifications, clipboard, etc.) are planned — see the [Native HostApi](#hostapi-reference) for the full set that will eventually be exposed to Java.

| Method | Description |
|--------|-------------|
| `log(int level, String message)` | Log a message (0=trace, 1=debug, 2=info, 3=warn, 4=error) |
| `trace(String message)` | Log at TRACE |
| `debug(String message)` | Log at DEBUG |
| `info(String message)` | Log at INFO |
| `warn(String message)` | Log at WARN |
| `error(String message)` | Log at ERROR |
| `registerMenuItem(String menu, String label, String action)` | Add a menu item to the app menu bar |

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
        .keyValue("Arch", System.getProperty("os.arch"))
        .separator()
        .button("refresh", "Refresh")
        .toJson();
}
```

Available builder methods:

| Method | Description |
|--------|-------------|
| `heading(text)` | Section heading |
| `label(text)` / `label(text, style)` | Text label with optional style |
| `text(text)` | Monospace text |
| `keyValue(key, value)` | Key-value display row |
| `separator()` | Horizontal rule |
| `spacer()` / `spacer(size)` | Flexible or fixed spacer |
| `badge(text, variant)` | Status badge (info/success/warn/error) |
| `progress(id, fraction, label)` | Progress bar |
| `button(id, label)` / `button(id, label, icon)` | Clickable button |
| `textInput(id, value, hint)` | Single-line text input |
| `checkbox(id, label, checked)` | Checkbox toggle |
| `horizontal(children)` | Horizontal layout |
| `vertical(children)` | Vertical layout |
| `scrollArea(maxHeight, children)` | Scrollable container |
| `raw(json)` | Insert raw JSON for unsupported widget types |

### Handling Events (Java)

Events arrive as JSON strings in `onEvent()`. Use `String.contains()` for simple matching:

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

For structured parsing, add a JSON library (Gson, Jackson) to your project:

```java
// With Gson
JsonObject event = JsonParser.parseString(eventJson).getAsJsonObject();
if (event.get("kind").getAsString().equals("menu_action")) {
    String action = event.get("action").getAsString();
    // ...
}
```

### Panel Plugins (Java)

To create a panel plugin, set `pluginType` to `"panel"` and specify a location:

```java
@Override
public PluginInfo getInfo() {
    return new PluginInfo(
        "My Panel",
        "Shows useful info",
        "1.0.0",
        "panel",    // Plugin type
        "bottom"    // Panel location: left, right, or bottom
    );
}

@Override
public String render() {
    // This is called every frame. Return your widget tree.
    return new Widgets()
        .heading("My Panel")
        .label("Hello from Java!")
        .toJson();
}
```

The panel is automatically registered when the plugin loads. When multiple plugins register at the same location, they appear as tabs.

---

## Native Plugins

Native plugins are compiled shared libraries (`.dylib` on macOS, `.so` on Linux, `.dll` on Windows). They communicate with the host through a C ABI, making the SDK language-agnostic.

Use native plugins for performance-critical functionality like terminal session backends (SSH, SFTP, serial).

### Architecture Overview

```
┌─────────────────────────────────────────────────┐
│                  Conch Host App                  │
│                                                  │
│  ┌──────────┐  ┌──────────┐  ┌───────────────┐  │
│  │ PluginBus│  │  Panel   │  │  HostApi       │  │
│  │ (IPC)    │  │ Registry │  │  (vtable)      │  │
│  └────┬─────┘  └────┬─────┘  └───────┬───────┘  │
│       │              │                │          │
└───────┼──────────────┼────────────────┼──────────┘
        │              │                │
        │   ┌──────────┴────────────────┘
        │   │  C ABI boundary
        │   │
   ┌────┴───┴──────────────────────────────────┐
   │            Plugin (.dylib/.so/.dll)        │
   │                                            │
   │  conch_plugin_info()     → metadata        │
   │  conch_plugin_setup()    → state pointer   │
   │  conch_plugin_event()    ← JSON events     │
   │  conch_plugin_render()   → JSON widgets    │
   │  conch_plugin_query()    ← RPC calls       │
   │  conch_plugin_teardown() → cleanup         │
   └────────────────────────────────────────────┘
```

Each plugin runs on its own thread. The host sends messages (render requests, events, queries) via a channel, and the plugin responds through exported functions.

### Quick Start (Rust)

**Cargo.toml:**
```toml
[package]
name = "my-plugin"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
conch_plugin_sdk = { path = "../../crates/conch_plugin_sdk" }
serde_json = "1"
```

**src/lib.rs:**
```rust
use std::ffi::CString;
use conch_plugin_sdk::{
    widgets::{PluginEvent, Widget, WidgetEvent},
    HostApi, PanelHandle, PanelLocation, PluginInfo, PluginType,
};

struct MyPlugin {
    api: &'static HostApi,
    panel: PanelHandle,
    counter: u64,
}

impl MyPlugin {
    fn new(api: &'static HostApi) -> Self {
        let msg = CString::new("My plugin loaded!").unwrap();
        (api.log)(2, msg.as_ptr());

        let name = CString::new("My Panel").unwrap();
        let panel = (api.register_panel)(PanelLocation::Left, name.as_ptr(), std::ptr::null());

        Self { api, panel, counter: 0 }
    }

    fn handle_event(&mut self, event: PluginEvent) {
        if let PluginEvent::Widget(WidgetEvent::ButtonClick { id }) = event {
            if id == "increment" {
                self.counter += 1;
            }
        }
    }

    fn render(&self) -> Vec<Widget> {
        vec![
            Widget::heading("My Plugin"),
            Widget::KeyValue {
                key: "Count".into(),
                value: self.counter.to_string(),
            },
            Widget::button("increment", "Add One"),
        ]
    }

    fn handle_query(&self, method: &str, args: serde_json::Value) -> serde_json::Value {
        match method {
            "get_count" => serde_json::json!({ "count": self.counter }),
            _ => serde_json::json!({ "error": "unknown method" }),
        }
    }
}

conch_plugin_sdk::declare_plugin!(
    info: PluginInfo {
        name: c"My Plugin".as_ptr(),
        description: c"A simple counter plugin".as_ptr(),
        version: c"0.1.0".as_ptr(),
        plugin_type: PluginType::Panel,
        panel_location: PanelLocation::Left,
        dependencies: std::ptr::null(),
        num_dependencies: 0,
    },
    state: MyPlugin,
    setup: |api| MyPlugin::new(api),
    event: |state, event| state.handle_event(event),
    render: |state| state.render(),
    query: |state, method, args| state.handle_query(method, args),
);
```

Build with `cargo build` and the `.dylib`/`.so` is discovered automatically.

### Required Exports

Every native plugin must export six C-ABI functions:

| Symbol | Signature | Purpose |
|--------|-----------|---------|
| `conch_plugin_info` | `() -> PluginInfo` | Return static metadata |
| `conch_plugin_setup` | `(*const HostApi) -> *mut c_void` | Initialize plugin state |
| `conch_plugin_event` | `(*mut c_void, *const c_char, usize)` | Handle incoming events (JSON) |
| `conch_plugin_render` | `(*mut c_void) -> *const c_char` | Return widget tree (JSON) |
| `conch_plugin_teardown` | `(*mut c_void)` | Cleanup and free state |
| `conch_plugin_query` | `(*mut c_void, *const c_char, *const c_char, usize) -> *mut c_char` | Handle RPC queries |

### HostApi Reference

The `HostApi` is a `#[repr(C)]` struct of function pointers passed to `conch_plugin_setup`. See the full reference in the [HostApi section below](#hostapi-full-reference).

### Session Backends

Native plugins can provide terminal session backends (SSH, serial, telnet). The plugin provides a vtable of callbacks for write, resize, and shutdown. See the SSH plugin (`plugins/conch-ssh/`) for a complete example.

### Examples in Other Languages

Native plugins can be written in any language that produces C-compatible shared libraries. See the C and Go examples in the [full native plugin documentation](https://github.com/an0nn30/rusty_conch/blob/v2/docs/plugin-sdk-native.md).

---

## Lua Plugins

Lua plugins are single `.lua` files — no compilation, no project setup. Drop a file in the plugins directory and it's discovered immediately. Good for quick personal scripts and automation.

### Lua Quick Start

Create a file in your plugins directory (e.g., `~/.config/conch/plugins/my-script.lua`):

```lua
-- plugin-name: My Script
-- plugin-description: A quick automation script
-- plugin-type: action
-- plugin-version: 1.0.0

function setup()
    host.log(2, "My script loaded!")
    host.register_menu_item("Tools", "Run My Script", "run_script")
end

function on_event(event_json)
    if string.find(event_json, "run_script") then
        host.log(2, "Script executed!")
    end
end

function render()
    return "[]"
end

function teardown()
    host.log(2, "My script unloaded")
end
```

Metadata is declared in comments at the top of the file. No build step — the host reads and executes the Lua source directly.

---

## Widget System

All plugin tiers share the same widget system. Plugins return a JSON array of widget objects, and the host renders them using egui.

### Layout Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `horizontal` | `id?`, `children`, `spacing?` | Horizontal layout |
| `vertical` | `id?`, `children`, `spacing?` | Vertical layout |
| `split_pane` | `id`, `direction`, `ratio`, `resizable`, `left`, `right` | Resizable split |
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
| `path_bar` | `id`, `segments` | Breadcrumb path |
| `tree_view` | `id`, `nodes`, `selected?` | Hierarchical tree |
| `table` | `id`, `columns`, `rows`, `sort_column?`, `sort_ascending?`, `selected_row?` | Data table |
| `drop_zone` | `id`, `label`, `children` | Drag-and-drop target |
| `context_menu` | `child`, `items` | Right-click menu wrapper |

---

## Widget Events

When a user interacts with a widget, the plugin receives an event:

| Event | Fields | Trigger |
|-------|--------|---------|
| `button_click` | `id` | Button pressed |
| `text_input_changed` | `id`, `value` | Text input changed |
| `text_input_submit` | `id`, `value` | Enter pressed |
| `checkbox_changed` | `id`, `checked` | Checkbox toggled |
| `combo_box_changed` | `id`, `value` | Dropdown changed |
| `tree_select` | `id`, `node_id` | Tree node selected |
| `tree_activate` | `id`, `node_id` | Tree node double-clicked |
| `tree_context_menu` | `id`, `node_id`, `action` | Tree context menu action |
| `table_select` | `id`, `row_id` | Table row selected |
| `table_sort` | `id`, `column`, `ascending` | Column header clicked |
| `tab_changed` | `id`, `active` | Tab switched |
| `path_bar_navigate` | `id`, `segment_index` | Path segment clicked |

---

## Plugin Events

The top-level event envelope delivered to plugins:

```json
// Menu action (user clicked a registered menu item)
{ "kind": "menu_action", "action": "do_something" }

// Widget interaction
{ "kind": "widget", "type": "button_click", "id": "my_button" }

// Bus event from another plugin
{ "kind": "bus_event", "event_type": "ssh.connected", "data": { "host": "..." } }

// Theme changed
{ "kind": "theme_changed", "theme_json": "{...}" }
```

---

## Inter-Plugin Communication

### Pub/Sub Events

Plugins can publish events that other plugins subscribe to:

```java
// Java — subscribe in setup(), receive in onEvent()
// Native — api.subscribe("ssh.connected"), api.publish_event(...)
// Lua — host.subscribe("ssh.connected"), host.publish_event(...)
```

### RPC Queries

Plugins can send direct queries to other plugins:

```java
// Native only (not yet available in Java/Lua)
char* result = api->query_plugin("SSH Manager", "get_sessions", "{}", len);
```

---

## Plugin Search Paths

The host scans these directories (in order):

1. `target/debug` and `target/release` (development)
2. `examples/plugins`
3. Executable directory and `plugins/` subdirectory
4. macOS: `Conch.app/Contents/Plugins/`
5. Linux: `/opt/conch/lib`, `/usr/lib/conch/plugins`
6. User config directory (`~/.config/conch/plugins/` or `~/Library/Application Support/conch/plugins/`)
7. Custom paths from `config.toml`: `[conch.plugins] search_paths = ["~/my-plugins"]`

**File extensions:**
- Native: `.dylib` (macOS), `.so` (Linux), `.dll` (Windows)
- Java: `.jar` (must have `Plugin-Class` in `META-INF/MANIFEST.MF`)
- Lua: `.lua` (must have metadata comments at the top)
