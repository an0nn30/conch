# Conch Plugin SDK

The Conch Plugin SDK enables third-party native plugins compiled as dynamic shared libraries (`.dylib` on macOS, `.so` on Linux, `.dll` on Windows). Plugins communicate with the host application exclusively through a C ABI, making the SDK language-agnostic — any language that can produce a C-compatible shared library can be used.

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Quick Start (Rust)](#quick-start-rust)
- [Plugin Lifecycle](#plugin-lifecycle)
- [Required Exports](#required-exports)
- [PluginInfo Metadata](#plugininfo-metadata)
- [HostApi Reference](#hostapi-reference)
- [Widget System](#widget-system)
- [Widget Events](#widget-events)
- [Plugin Events](#plugin-events)
- [Session Backends](#session-backends)
- [Inter-Plugin Communication](#inter-plugin-communication)
- [Memory Management](#memory-management)
- [Building Plugins](#building-plugins)
- [Examples in Other Languages](#examples-in-other-languages)

---

## Architecture Overview

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

Each plugin runs on its own thread. The host sends messages (render requests, events, queries) to the plugin via a channel, and the plugin responds through the exported functions. The `HostApi` vtable provides callback function pointers for the plugin to call back into the host.

---

## Quick Start (Rust)

The fastest way to create a Conch plugin in Rust:

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
        // Log a startup message.
        let msg = CString::new("My plugin loaded!").unwrap();
        (api.log)(2, msg.as_ptr()); // 2 = info

        // Register a panel on the left sidebar.
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

Build with `cargo build` and the resulting `.dylib`/`.so` will be discovered automatically from `target/debug` or `target/release`.

---

## Plugin Lifecycle

1. **Discovery** — On launch, the host scans configured directories for shared libraries matching the platform extension (`.dylib`, `.so`, `.dll`). It loads each library, calls `conch_plugin_info()` to read metadata, then unloads it.

2. **Loading** — When a user enables a plugin (or it's auto-loaded from persisted state), the host:
   - Loads the shared library
   - Reads metadata via `conch_plugin_info()`
   - Spawns a dedicated thread for the plugin
   - Calls `conch_plugin_setup(host_api)` on that thread
   - The plugin returns an opaque state pointer

3. **Running** — The plugin thread enters a message loop. The host sends:
   - **Render requests** → plugin returns JSON widget tree via `conch_plugin_render()`
   - **Events** → widget interactions, bus events, menu actions via `conch_plugin_event()`
   - **Queries** → RPC calls from other plugins via `conch_plugin_query()`

4. **Unloading** — The host sends a shutdown message, calls `conch_plugin_teardown()`, and joins the thread.

---

## Required Exports

Every plugin must export exactly six C-ABI functions:

| Symbol | Signature | Purpose |
|--------|-----------|---------|
| `conch_plugin_info` | `() -> PluginInfo` | Return static metadata |
| `conch_plugin_setup` | `(*const HostApi) -> *mut c_void` | Initialize plugin state |
| `conch_plugin_event` | `(*mut c_void, *const c_char, usize)` | Handle incoming events (JSON) |
| `conch_plugin_render` | `(*mut c_void) -> *const c_char` | Return widget tree (JSON, null-terminated) |
| `conch_plugin_teardown` | `(*mut c_void)` | Cleanup and free state |
| `conch_plugin_query` | `(*mut c_void, *const c_char, *const c_char, usize) -> *mut c_char` | Handle RPC queries |

### Function Details

**`conch_plugin_info`**
Called during discovery and loading. Must return a `PluginInfo` struct with `'static` string pointers.

**`conch_plugin_setup`**
Called once on the plugin's dedicated thread. Receives a pointer to the `HostApi` vtable (valid for the plugin's lifetime). Returns an opaque pointer to plugin state that will be passed to all subsequent calls.

**`conch_plugin_event`**
Called when the host delivers an event. The second and third arguments are a UTF-8 JSON string and its byte length (not null-terminated). The JSON deserializes to a `PluginEvent`.

**`conch_plugin_render`**
Called when the host needs the current widget tree. Must return a null-terminated UTF-8 JSON string that the plugin owns. The pointer must remain valid until the next call to `conch_plugin_render` or `conch_plugin_teardown`.

**`conch_plugin_teardown`**
Called once during shutdown. The plugin must free the state pointer and any associated resources.

**`conch_plugin_query`**
Called when another plugin (or the host) sends an RPC query. Arguments:
- `state` — opaque state pointer
- `method` — null-terminated method name
- `args_json` + `len` — UTF-8 JSON arguments (not null-terminated)

Returns a heap-allocated null-terminated JSON string. The **caller is responsible for freeing** the returned pointer (the host calls `free_string`).

---

## PluginInfo Metadata

```c
// C layout
struct PluginInfo {
    const char* name;           // Human-readable name
    const char* description;    // Short description
    const char* version;        // Semver string, e.g. "1.0.0"
    uint32_t    plugin_type;    // 0 = Action, 1 = Panel
    uint32_t    panel_location; // 0 = None, 1 = Left, 2 = Right, 3 = Bottom
    const char** dependencies;  // Null-terminated array of plugin names (or NULL)
    size_t      num_dependencies;
};
```

### Plugin Types

| Type | Value | Description |
|------|-------|-------------|
| `Action` | `0` | Run-once plugin triggered by menu item or keybinding |
| `Panel` | `1` | Persistent panel that renders a widget tree continuously |

### Panel Locations

| Location | Value | Description |
|----------|-------|-------------|
| `None` | `0` | No default location (action plugins, or runtime-registered) |
| `Left` | `1` | Left sidebar |
| `Right` | `2` | Right sidebar |
| `Bottom` | `3` | Bottom panel |

When multiple plugins register at the same location, the host automatically groups them with a tabbed interface.

---

## HostApi Reference

The `HostApi` is a `#[repr(C)]` struct of function pointers passed to `conch_plugin_setup`. All string parameters are null-terminated UTF-8 unless noted otherwise. JSON string parameters include a `len` parameter and are **not** null-terminated.

### Panel Management

```c
// Register a panel. Returns a handle for subsequent updates.
PanelHandle register_panel(
    PanelLocation location,
    const char* name,       // Display name
    const char* icon        // Optional icon (NULL for none)
);

// Update the widget tree for a panel.
void set_widgets(
    PanelHandle handle,
    const char* json,       // UTF-8 JSON (NOT null-terminated)
    size_t len
);
```

### Dialogs

All dialog functions **block** the plugin thread until the user responds.

```c
// Show a form dialog. Returns JSON response or NULL if cancelled.
// Caller must free with free_string().
char* show_form(const char* json, size_t len);

// Show a yes/no confirmation dialog.
bool show_confirm(const char* message);

// Show a text input prompt. Returns input or NULL if cancelled.
// Caller must free with free_string().
char* show_prompt(const char* message, const char* default_value);

// Show an informational alert.
void show_alert(const char* title, const char* message);

// Show an error dialog.
void show_error(const char* title, const char* message);
```

### Notifications & Logging

```c
// Show a toast notification.
// JSON: { "title": "...", "body": "...", "level": "info|warn|error", "duration_ms": 3000 }
void notify(const char* json, size_t len);

// Log a message. Levels: 0=trace, 1=debug, 2=info, 3=warn, 4=error
void log(uint8_t level, const char* message);
```

### Inter-Plugin Communication

```c
// Publish an event on the message bus.
void publish_event(const char* event_type, const char* data_json, size_t len);

// Subscribe to events of a given type.
void subscribe(const char* event_type);

// Send an RPC query to another plugin. Blocks until response.
// Caller must free with free_string().
char* query_plugin(
    const char* target_plugin,
    const char* method,
    const char* args_json,
    size_t len
);

// Register this plugin as a service provider.
void register_service(const char* service_name);
```

### Configuration Persistence

```c
// Read a config value. Returns JSON string or NULL if not found.
// Caller must free with free_string().
char* get_config(const char* key);

// Write a config value. Pass NULL for value to delete the key.
void set_config(const char* key, const char* value);
```

### Menu Registration

```c
// Add a menu item to the application menu bar.
void register_menu_item(
    const char* menu,      // Menu name, e.g. "Tools"
    const char* label,     // Item label, e.g. "New SSH Connection..."
    const char* action,    // Action ID sent back as MenuAction event
    const char* keybind    // Optional keybind string, e.g. "cmd+shift+s" (or NULL)
);
```

### Clipboard

```c
void clipboard_set(const char* text);

// Returns clipboard contents or NULL. Caller must free with free_string().
char* clipboard_get();
```

### Theme

```c
// Returns the current theme as JSON. Caller must free with free_string().
char* get_theme();
```

### Context Menu

```c
// Show a context menu and return the selected action ID.
// JSON input: [{ "id": "...", "label": "...", "enabled": true }, ...]
// Returns selected action ID or NULL if dismissed.
// Caller must free with free_string().
char* show_context_menu(const char* json, size_t len);
```

### Memory

```c
// Free a string allocated by the host (returned from show_form, query_plugin, etc.)
void free_string(char* ptr);
```

---

## Widget System

Plugins return a widget tree as a JSON array of `Widget` objects. The host renders them using egui.

### Layout Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `Horizontal` | `id?`, `children`, `spacing?` | Horizontal layout container |
| `Vertical` | `id?`, `children`, `spacing?` | Vertical layout container |
| `SplitPane` | `id`, `direction`, `ratio`, `resizable`, `left`, `right` | Resizable split |
| `ScrollArea` | `id?`, `max_height?`, `children` | Scrollable region |
| `Tabs` | `id`, `active`, `tabs: [{label, children}]` | Tabbed container |

### Display Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `Heading` | `text` | Large heading text |
| `Label` | `text`, `style?` | Styled text label |
| `Text` | `text` | Monospace text |
| `ScrollText` | `id`, `text`, `max_height?` | Scrollable log with auto-scroll |
| `KeyValue` | `key`, `value` | Key-value display row |
| `Separator` | — | Horizontal rule |
| `Spacer` | `size?` | Spacing between elements |
| `IconLabel` | `icon`, `text`, `style?` | Icon + text row |
| `Badge` | `text`, `variant` | Colored badge (Info/Success/Warn/Error) |
| `Progress` | `id`, `fraction`, `label?` | Progress bar (0.0–1.0) |
| `Image` | `id?`, `src`, `width?`, `height?` | Image display |

### Interactive Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `Button` | `id`, `label`, `icon?`, `enabled?` | Clickable button |
| `TextInput` | `id`, `value`, `hint?`, `submit_on_enter?` | Single-line text input |
| `TextEdit` | `id`, `value`, `hint?`, `lines?` | Multi-line text editor |
| `Checkbox` | `id`, `label`, `checked` | Toggle checkbox |
| `ComboBox` | `id`, `selected`, `options: [{value, label}]` | Dropdown selector |

### Complex Widgets

| Widget | Fields | Description |
|--------|--------|-------------|
| `Toolbar` | `id?`, `items` | Toolbar with buttons, separators, inputs |
| `PathBar` | `id`, `segments` | Breadcrumb-style path navigation |
| `TreeView` | `id`, `nodes`, `selected?` | Hierarchical tree with expand/collapse |
| `Table` | `id`, `columns`, `rows`, `sort_column?`, `sort_ascending?`, `selected_row?` | Sortable data table |
| `DropZone` | `id`, `label`, `children` | Drag-and-drop target |
| `ContextMenu` | `child`, `items` | Wraps a widget with a right-click menu |

### TextStyle

```json
{
    "bold": true,
    "italic": false,
    "color": "#ff0000",
    "size": 14.0
}
```

### Widget JSON Example

```json
[
    { "Heading": { "text": "Server List" } },
    { "Separator": null },
    { "TreeView": {
        "id": "servers",
        "nodes": [
            {
                "id": "prod",
                "label": "Production",
                "icon": "folder",
                "expanded": true,
                "children": [
                    { "id": "web1", "label": "web-1.example.com", "icon": "server" },
                    { "id": "web2", "label": "web-2.example.com", "icon": "server" }
                ]
            }
        ],
        "selected": "web1"
    }},
    { "Separator": null },
    { "Button": { "id": "connect", "label": "Connect", "icon": "link" } }
]
```

---

## Widget Events

When a user interacts with a widget, the host sends a `PluginEvent::Widget(event)` to the plugin. All events are JSON-serialized.

| Event | Fields | Trigger |
|-------|--------|---------|
| `ButtonClick` | `id` | Button pressed |
| `TextInputChanged` | `id`, `value` | Text input changed (debounced) |
| `TextInputSubmit` | `id`, `value` | Enter pressed in text input |
| `TextEditChanged` | `id`, `value` | Multi-line text changed |
| `CheckboxChanged` | `id`, `checked` | Checkbox toggled |
| `ComboBoxChanged` | `id`, `value` | Dropdown selection changed |
| `TreeSelect` | `id`, `node_id` | Tree node selected |
| `TreeActivate` | `id`, `node_id` | Tree node double-clicked |
| `TreeToggle` | `id`, `node_id`, `expanded` | Tree node expanded/collapsed |
| `TreeContextMenu` | `id`, `node_id`, `action` | Tree context menu action |
| `TableSelect` | `id`, `row_id` | Table row selected |
| `TableActivate` | `id`, `row_id` | Table row double-clicked |
| `TableSort` | `id`, `column`, `ascending` | Column header clicked |
| `TableContextMenu` | `id`, `row_id`, `action` | Table context menu action |
| `TabChanged` | `id`, `active` | Tab switched |
| `PathBarNavigate` | `id`, `segment_index` | Path segment clicked |
| `Drop` | `id`, `source?`, `items` | Items dropped |
| `ContextMenuAction` | `action` | Context menu item selected |
| `ToolbarInputSubmit` | `id`, `value` | Toolbar input submitted |
| `ToolbarInputChanged` | `id`, `value` | Toolbar input changed |

---

## Plugin Events

The top-level event envelope sent to `conch_plugin_event`:

```json
// Widget interaction
{ "type": "widget", "ButtonClick": { "id": "my_button" } }

// Menu action
{ "type": "menu_action", "action": "do_something" }

// Bus event from another plugin
{ "type": "bus_event", "event_type": "ssh.connected", "data": { "host": "..." } }

// RPC query from another plugin
{ "type": "bus_query", "request_id": "abc123", "method": "get_status", "args": {} }

// Theme changed
{ "type": "theme_changed", "theme_json": "{...}" }

// Shutdown signal
{ "type": "shutdown" }
```

---

## Session Backends

Plugins can register terminal session backends (e.g., SSH, serial, telnet). The plugin provides a vtable of callbacks that the host calls to write data, resize, and shutdown.

### SessionMeta

```c
struct SessionMeta {
    const char* title;          // Full title: "user@host.example.com"
    const char* short_title;    // Tab title: "host"
    const char* session_type;   // Type identifier: "ssh", "serial"
    const char* icon;           // Optional icon name
};
```

### SessionBackendVtable

```c
struct SessionBackendVtable {
    void (*write)(void* handle, const uint8_t* buf, size_t len);
    void (*resize)(void* handle, uint16_t cols, uint16_t rows);
    void (*shutdown)(void* handle);
    void (*drop)(void* handle);
};
```

### Opening a Session

```c
struct OpenSessionResult {
    SessionHandle handle;
    OutputCallback output_cb;   // void (*)(void* ctx, const uint8_t* buf, size_t len)
    void* output_ctx;
};

OpenSessionResult open_session(
    const SessionMeta* meta,
    const SessionBackendVtable* vtable,
    void* backend_handle
);
```

After opening, the plugin:
- Receives user keystrokes via `vtable.write()`
- Pushes terminal output via `output_cb(output_ctx, buf, len)`
- Handles terminal resize via `vtable.resize()`

---

## Inter-Plugin Communication

### Pub/Sub Events

```c
// Publisher
api->publish_event("ssh.session_ready", "{\"host\":\"example.com\"}", len);

// Subscriber (in setup)
api->subscribe("ssh.session_ready");
// Events arrive as PluginEvent::BusEvent in conch_plugin_event()
```

### RPC Queries

```c
// Caller
char* result = api->query_plugin("SSH Manager", "connect", "{\"host\":\"...\"}", len);
// ... use result ...
api->free_string(result);

// Service provider (in setup)
api->register_service("connect");
// Queries arrive as PluginEvent::BusQuery in conch_plugin_event()
// Return value from conch_plugin_query() is sent back to caller
```

---

## Memory Management

**Critical rules:**

1. **Strings returned by the host** (from `show_form`, `show_prompt`, `query_plugin`, `clipboard_get`, `get_config`, `get_theme`, `show_context_menu`) **must be freed** by calling `free_string()`.

2. **Strings returned by the plugin** (from `conch_plugin_render`) must remain valid until the next call to that function or `conch_plugin_teardown`. The host does not free them.

3. **Strings returned by `conch_plugin_query`** are heap-allocated by the plugin. The host frees them via `free_string`.

4. **JSON data parameters** (`*const c_char` + `len`) are borrowed — the plugin does not need to free them. They are valid only for the duration of the function call.

---

## Building Plugins

### Rust

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
conch_plugin_sdk = { path = "path/to/crates/conch_plugin_sdk" }
serde_json = "1"
```

The `declare_plugin!` macro handles all exports. Build with `cargo build`.

### Other Languages

Compile as a shared library exporting the six required C symbols. See the [examples below](#examples-in-other-languages) for Go and C implementations.

### Plugin Search Paths

The host scans these directories (in order):
1. `target/debug` and `target/release` (development)
2. `examples/plugins`
3. Executable directory (installed builds)
4. `$XDG_CONFIG_HOME/conch/plugins` or `~/Library/Application Support/conch/plugins`
5. Custom paths from `config.toml` under `[conch.plugins] search_paths = [...]`

---

## Examples in Other Languages

### Go

```go
package main

/*
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <stdbool.h>

// Plugin types matching the Conch ABI.

typedef enum {
    PluginType_Action = 0,
    PluginType_Panel  = 1,
} PluginType;

typedef enum {
    PanelLocation_None   = 0,
    PanelLocation_Left   = 1,
    PanelLocation_Right  = 2,
    PanelLocation_Bottom = 3,
} PanelLocation;

typedef struct {
    const char*        name;
    const char*        description;
    const char*        version;
    PluginType         plugin_type;
    PanelLocation      panel_location;
    const char* const* dependencies;
    size_t             num_dependencies;
} PluginInfo;

typedef uint64_t PanelHandle;

// HostApi — only the function pointers we use in this example.
// The full struct has 22 function pointers; pad unused ones as void*.
typedef struct {
    PanelHandle (*register_panel)(PanelLocation location, const char* name, const char* icon);
    void        (*set_widgets)(PanelHandle handle, const char* json, size_t len);
    // ... (remaining 20 function pointers omitted for brevity — must still be present as void*)
    void* _pad[20];
} HostApi;
*/
import "C"

import (
	"encoding/json"
	"fmt"
	"sync"
	"unsafe"
)

// Plugin state.
type MyPlugin struct {
	mu      sync.Mutex
	api     *C.HostApi
	panel   C.PanelHandle
	counter int
}

var (
	pluginName = C.CString("Go Counter")
	pluginDesc = C.CString("A counter plugin written in Go")
	pluginVer  = C.CString("0.1.0")
)

//export conch_plugin_info
func conch_plugin_info() C.PluginInfo {
	return C.PluginInfo{
		name:             pluginName,
		description:      pluginDesc,
		version:          pluginVer,
		plugin_type:      C.PluginType_Panel,
		panel_location:   C.PanelLocation_Left,
		dependencies:     nil,
		num_dependencies: 0,
	}
}

//export conch_plugin_setup
func conch_plugin_setup(api *C.HostApi) unsafe.Pointer {
	name := C.CString("Go Counter")
	defer C.free(unsafe.Pointer(name))

	panel := C.HostApi_register_panel(api, C.PanelLocation_Left, name, nil)

	plugin := &MyPlugin{
		api:   api,
		panel: panel,
	}

	// Pin the plugin so GC doesn't move it.
	pluginRegistry.Store(plugin, true)
	return unsafe.Pointer(plugin)
}

//export conch_plugin_event
func conch_plugin_event(state unsafe.Pointer, jsonPtr *C.char, jsonLen C.size_t) {
	plugin := (*MyPlugin)(state)
	data := C.GoBytes(unsafe.Pointer(jsonPtr), C.int(jsonLen))

	var event map[string]interface{}
	if err := json.Unmarshal(data, &event); err != nil {
		return
	}

	if event["type"] == "widget" {
		if click, ok := event["ButtonClick"].(map[string]interface{}); ok {
			if click["id"] == "increment" {
				plugin.mu.Lock()
				plugin.counter++
				plugin.mu.Unlock()
			}
		}
	}
}

// renderBuf holds the last rendered JSON to keep the pointer valid.
var renderBuf *C.char

//export conch_plugin_render
func conch_plugin_render(state unsafe.Pointer) *C.char {
	plugin := (*MyPlugin)(state)
	plugin.mu.Lock()
	count := plugin.counter
	plugin.mu.Unlock()

	widgets := []map[string]interface{}{
		{"Heading": map[string]string{"text": "Go Counter"}},
		{"KeyValue": map[string]string{
			"key":   "Count",
			"value": fmt.Sprintf("%d", count),
		}},
		{"Button": map[string]interface{}{
			"id":    "increment",
			"label": "Add One",
		}},
	}

	jsonBytes, _ := json.Marshal(widgets)

	// Free previous buffer, allocate new one.
	if renderBuf != nil {
		C.free(unsafe.Pointer(renderBuf))
	}
	renderBuf = C.CString(string(jsonBytes))
	return renderBuf
}

//export conch_plugin_teardown
func conch_plugin_teardown(state unsafe.Pointer) {
	plugin := (*MyPlugin)(state)
	pluginRegistry.Delete(plugin)
	if renderBuf != nil {
		C.free(unsafe.Pointer(renderBuf))
		renderBuf = nil
	}
}

//export conch_plugin_query
func conch_plugin_query(state unsafe.Pointer, method *C.char, argsJson *C.char, argsLen C.size_t) *C.char {
	m := C.GoString(method)
	switch m {
	case "get_count":
		plugin := (*MyPlugin)(state)
		plugin.mu.Lock()
		count := plugin.counter
		plugin.mu.Unlock()
		result := fmt.Sprintf(`{"count":%d}`, count)
		return C.CString(result)
	default:
		return C.CString(`{"error":"unknown method"}`)
	}
}

// Prevent GC from collecting pinned plugin instances.
var pluginRegistry sync.Map

func main() {} // Required by cgo but never called.
```

Build with:
```bash
CGO_ENABLED=1 go build -buildmode=c-shared -o my_plugin.dylib .
```

> **Note:** The Go example simplifies the HostApi struct. In a real plugin, you must define all 22 function pointers in the correct order (or use a byte-padded struct) to match the ABI layout. Calling host functions from Go requires cgo wrapper functions since Go cannot call C function pointers directly from struct fields.

---

### C

```c
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <stdbool.h>

/* ------------------------------------------------------------------ */
/* Conch Plugin ABI types                                             */
/* ------------------------------------------------------------------ */

typedef enum { PLUGIN_ACTION = 0, PLUGIN_PANEL = 1 } PluginType;

typedef enum {
    PANEL_NONE   = 0,
    PANEL_LEFT   = 1,
    PANEL_RIGHT  = 2,
    PANEL_BOTTOM = 3,
} PanelLocation;

typedef struct {
    const char*        name;
    const char*        description;
    const char*        version;
    PluginType         plugin_type;
    PanelLocation      panel_location;
    const char* const* dependencies;
    size_t             num_dependencies;
} PluginInfo;

typedef uint64_t PanelHandle;
typedef uint64_t SessionHandle;

/* Forward declare opaque types for unused vtable entries. */
typedef struct SessionMeta SessionMeta;
typedef struct SessionBackendVtable SessionBackendVtable;
typedef struct OpenSessionResult OpenSessionResult;

typedef struct {
    /* Panel management */
    PanelHandle       (*register_panel)(PanelLocation, const char*, const char*);
    void              (*set_widgets)(PanelHandle, const char*, size_t);

    /* Sessions */
    OpenSessionResult (*open_session)(const SessionMeta*, const SessionBackendVtable*, void*);
    void              (*close_session)(SessionHandle);

    /* Dialogs */
    char* (*show_form)(const char*, size_t);
    bool  (*show_confirm)(const char*);
    char* (*show_prompt)(const char*, const char*);
    void  (*show_alert)(const char*, const char*);
    void  (*show_error)(const char*, const char*);

    /* Notifications */
    void (*notify)(const char*, size_t);
    void (*log_msg)(uint8_t, const char*);

    /* IPC */
    void  (*publish_event)(const char*, const char*, size_t);
    void  (*subscribe)(const char*);
    char* (*query_plugin)(const char*, const char*, const char*, size_t);
    void  (*register_service)(const char*);

    /* Config */
    char* (*get_config)(const char*);
    void  (*set_config)(const char*, const char*);

    /* Menu */
    void (*register_menu_item)(const char*, const char*, const char*, const char*);

    /* Clipboard */
    void  (*clipboard_set)(const char*);
    char* (*clipboard_get)(void);

    /* Theme */
    char* (*get_theme)(void);

    /* Context menu */
    char* (*show_context_menu)(const char*, size_t);

    /* Memory */
    void (*free_string)(char*);
} HostApi;

/* ------------------------------------------------------------------ */
/* Plugin state                                                       */
/* ------------------------------------------------------------------ */

typedef struct {
    const HostApi* api;
    PanelHandle    panel;
    uint64_t       counter;
} MyPlugin;

/* Static render buffer (valid until next render call). */
static char render_buf[4096];

/* ------------------------------------------------------------------ */
/* Required exports                                                   */
/* ------------------------------------------------------------------ */

PluginInfo conch_plugin_info(void) {
    PluginInfo info = {
        .name             = "C Counter",
        .description      = "A counter plugin written in C",
        .version          = "0.1.0",
        .plugin_type      = PLUGIN_PANEL,
        .panel_location   = PANEL_LEFT,
        .dependencies     = NULL,
        .num_dependencies = 0,
    };
    return info;
}

void* conch_plugin_setup(const HostApi* api) {
    MyPlugin* plugin = calloc(1, sizeof(MyPlugin));
    plugin->api = api;

    api->log_msg(2, "C Counter plugin loaded");
    plugin->panel = api->register_panel(PANEL_LEFT, "C Counter", NULL);

    return plugin;
}

void conch_plugin_event(void* state, const char* json, size_t len) {
    MyPlugin* plugin = (MyPlugin*)state;

    /*
     * Minimal JSON parsing: check for ButtonClick with id "increment".
     * A real plugin should use a JSON library (cJSON, yyjson, etc.).
     */
    if (len > 0 && strstr(json, "\"ButtonClick\"") && strstr(json, "\"increment\"")) {
        plugin->counter++;
    }
}

const char* conch_plugin_render(void* state) {
    MyPlugin* plugin = (MyPlugin*)state;

    snprintf(render_buf, sizeof(render_buf),
        "["
            "{\"Heading\":{\"text\":\"C Counter\"}},"
            "{\"KeyValue\":{\"key\":\"Count\",\"value\":\"%llu\"}},"
            "{\"Button\":{\"id\":\"increment\",\"label\":\"Add One\"}}"
        "]",
        (unsigned long long)plugin->counter
    );

    return render_buf;
}

void conch_plugin_teardown(void* state) {
    free(state);
}

char* conch_plugin_query(void* state, const char* method, const char* args, size_t args_len) {
    MyPlugin* plugin = (MyPlugin*)state;

    if (strcmp(method, "get_count") == 0) {
        char* result = malloc(64);
        snprintf(result, 64, "{\"count\":%llu}", (unsigned long long)plugin->counter);
        return result;
    }

    char* err = malloc(32);
    snprintf(err, 32, "{\"error\":\"unknown\"}");
    return err;
}
```

Build with:
```bash
# macOS
cc -shared -o libmy_plugin.dylib my_plugin.c

# Linux
cc -shared -fPIC -o libmy_plugin.so my_plugin.c
```

---

## Tips for Non-Rust Plugins

1. **Match the struct layout exactly.** The `HostApi` has 22 function pointers in a specific order. Missing or misordered entries will cause crashes. Use the C example as a reference for the complete layout.

2. **Return stable pointers from `conch_plugin_render`.** The returned string must remain valid until the next render or teardown call. Use a static buffer or module-level allocation.

3. **Free host-allocated strings.** Any `char*` returned by a HostApi function (except `register_panel` and session handles) must be freed with `free_string()`.

4. **JSON encoding matters.** Widget trees, events, and queries all use JSON. Use a proper JSON library — hand-rolled JSON is fragile.

5. **Thread safety.** Your plugin runs on a dedicated thread. The HostApi functions are safe to call from that thread. If you spawn additional threads, synchronize access to your state.

6. **Library naming.** The host discovers plugins by file extension:
   - macOS: `lib*.dylib` or `*.dylib`
   - Linux: `lib*.so` or `*.so`
   - Windows: `*.dll`
