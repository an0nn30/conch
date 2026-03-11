# Conch v2 Rewrite — Architecture & Plan

## Executive Summary

Conch v2 is a near-complete rewrite that transforms a monolithic terminal+SSH+files application into a minimal terminal emulator with a rich plugin ecosystem. Without plugins loaded, the app is a pure local terminal with tabbing and multi-window support. All features — SSH, file browsing, SFTP, tunnels — become plugins (3 native shared libraries + 2 Lua) shipped alongside releases. Users optionally install the plugins they want.

**Approach:** `v2` branch on existing repo. Carry forward `conch_core` and `conch_plugin` (expanded). Extract local PTY into `conch_pty`. Rewrite `conch_app` from scratch. Delete `conch_session` and `conch_terminal`. Create new `conch_plugin_sdk` crate and `plugins/` directory.

---

## Decisions

| Question | Decision |
|----------|----------|
| SSH in core or plugin? | **Plugin.** Core is purely a local terminal emulator. SSH is a native shared library plugin. |
| Native plugin rendering | **JSON widget descriptors** over C ABI. Language-agnostic (Rust, Go, C). |
| Native plugin sandboxing | **Native = trusted.** No WASM. Full system access. |
| Plugin config | **Each plugin has its own config file.** No shared namespaced config. |
| Plugin-to-plugin IPC | **Rich.** Broadcast events, direct request/response queries, service registration. Full feature parity with current monolith. |
| Repo strategy | **v2 branch** on existing repo. Delete everything except needed crates. Preserve git history. |
| Multi-window | **Required from Phase 1.** Not deferred. |
| Plugin management UI | **Core-owned dialog** accessible via menu bar. Not a panel, not a plugin. |
| Built-in plugin distribution | **Separate shared libraries** shipped alongside release. User downloads/installs optionally. Not embedded in binary. |
| Build order | **Phase 1:** minimal terminal (no plugins). **Phase 2:** plugin API + SDK. **Phase 3:** build the plugins. |
| JSON widget performance | **Not a concern.** Widget trees are small (<50KB), updated infrequently (on events, not every frame). Terminal I/O (60fps) bypasses JSON entirely. `serde_json` parses 50KB in ~100us (<1% of frame budget). Core caches parsed widget trees between plugin updates. |

---

## 1. Current Crate Map

| Crate | Lines | Purpose | Disposition |
|-------|-------|---------|-------------|
| `conch_core` | ~1,500 | Config, models, themes | **Keep** (strip SSH-specific models) |
| `conch_session` | ~1,300 | PTY, SSH, SFTP, rsync, tunnels | **Break up** — PTY → `conch_pty`, SSH/SFTP → plugins |
| `conch_plugin` | ~1,600 | Lua 5.4 plugin runtime | **Keep + major expansion** |
| `conch_app` | ~14,600 | GUI app (egui/eframe) | **Rewrite from scratch** |
| `conch_terminal` | ~1,100 | GPU renderer (iced, unused) | **Delete** |

---

## 2. What Stays in Core (Zero Plugins Loaded)

```
┌──────────────────────────────────────────────┐
│ Tab Bar: [Local Shell 1] [Local Shell 2]  [+]│
├──────────────────────────────────────────────┤
│                                              │
│            Terminal Emulator                 │
│         (alacritty_terminal PTY)             │
│                                              │
│                                              │
└──────────────────────────────────────────────┘
```

**Core responsibilities:**
- Local PTY session creation (spawn shell, bridge to alacritty_terminal)
- Terminal rendering (egui widget wrapping alacritty_terminal `Term`)
- Tab management (create, close, reorder, rename tabs)
- Multi-window support (detach tab to new OS window)
- Keyboard handling (escape sequences, tab switching, app shortcuts)
- Mouse handling (selection, copy/paste, scrolling)
- Config loading (`config.toml` — font, colors, window, terminal settings)
- Theme/color scheme application
- Plugin host runtime (load, run, dispatch commands, render panels)
- Panel layout (left, right, bottom — hidden until plugins claim them)
- Toast notification infrastructure (plugins call `app.notify()`, core renders toasts)
- Plugin management dialog (menu bar → dialog for load/unload/dependency info)

**Core does NOT include:**
- SSH connections (native plugin)
- File browser (native plugin)
- SFTP/rsync transfers (native plugin)
- SSH tunnels (Lua plugin)
- Session manager sidebar (part of SSH plugin)
- Server/connection models (owned by SSH plugin)
- Notification history viewer (Lua plugin)

---

## 3. What Becomes a Plugin

### 3.1 SSH Plugin (`conch-ssh`, native shared library)

**Current code to extract:** `conch_session/src/ssh/` (~900 lines), `conch_app/src/ssh.rs` (~480 lines), session panel UI (~730 lines), `conch_core/src/models.rs` (ServerEntry, SavedTunnel)

**Plugin responsibilities:**
- Manage saved servers (folders, entries with host/port/user/key) in its own config file (`~/.config/conch/plugins/ssh/servers.toml`)
- Connect via SSH (async, with password prompt + host key verification via dialog API)
- Provide SSH shell sessions to core as tab backends via byte-stream session backend API
- Register a panel tab (right panel by default, "Sessions") with server tree UI
- Parse `~/.ssh/config` for auto-discovered hosts
- Quick-connect (text input → parse `user@host:port` → connect)
- Publish events: `ssh.session_ready`, `ssh.session_closed`
- Register services: `ssh.connect`, `ssh.exec`, `ssh.get_sessions`, `ssh.get_handle`

### 3.2 File Explorer Plugin (`conch-files`, native shared library)

**Current code to extract:** `conch_app/src/ui/file_browser.rs` (~390 lines), `conch_app/src/ui/sidebar.rs` file browser sections (~400 lines), `sidebar_handler.rs` file actions (~200 lines)

**Plugin responsibilities:**
- Register a panel tab (left panel by default, "Files")
- Render dual-pane browser with navigation (SplitPane, Table, PathBar, Toolbar)
- Local file listing via native file system APIs
- Remote listing via SFTP plugin dependency (`query_plugin("sftp", "list", ...)`)
- File operations: copy, move, delete, rename, new folder (via context menu)
- Drag-drop between panes → triggers SFTP upload/download
- Column toggles (name, ext, size, modified)

**Dependencies:** `conch-sftp` (optional — local-only browsing if not loaded)

### 3.3 SFTP/Transfer Plugin (`conch-sftp`, native shared library)

**Current code to extract:** `conch_session/src/sftp.rs` (~600 lines), `conch_session/src/rsync.rs` (~370 lines)

**Plugin responsibilities:**
- Provide SFTP services to other plugins via IPC
- Rsync detection and fallback with zstd compression
- Register bottom panel tab for transfer progress
- Handle upload/download requests from file explorer plugin
- Subscribe to `ssh.session_ready` to auto-attach SFTP to SSH sessions
- Register services: `sftp.list`, `sftp.upload`, `sftp.download`, `sftp.mkdir`, `sftp.remove`

**Dependencies:** `conch-ssh` (requires SSH handle for SFTP channel)

### 3.4 SSH Tunnel Plugin (`conch-tunnels`, Lua)

**Current code to extract:** `conch_session/src/ssh/tunnel.rs` (~133 lines), `conch_app/src/ui/dialogs/tunnels.rs` (~447 lines)

**Plugin responsibilities:**
- UI for creating/editing/deleting port forwards
- Status display (active tunnels, forwarded ports)
- Register bottom panel tab for tunnel status
- Query SSH plugin for active sessions and tunnel setup

**Dependencies:** `conch-ssh`

### 3.5 Notification History Plugin (`conch-notifications`, Lua)

**Current code to extract:** `conch_app/src/ui/dialogs/notification_history.rs` (~226 lines)

**Plugin responsibilities:**
- Subscribe to `notification.shown` events from core
- Maintain history, render viewer as bottom panel tab
- Clear/filter/search notifications

---

## 4. Session Backend Architecture

Core needs a generic way for plugins to provide terminal session backends. The SSH plugin is the primary consumer, but this same interface supports future Telnet, serial, Mosh, or any protocol plugin.

### Byte Stream Model

```
┌──────────────────┐         ┌──────────────────┐
│   SSH Plugin     │         │     Core App     │
│                  │         │                  │
│  russh channel ──┼── output bytes ──► Term    │
│                  │         │  (VTE parser)    │
│  russh channel ◄─┼── input bytes ───┤         │
│                  │         │  egui renders    │
│  resize handler ◄┼── resize events ─┤ Term    │
└──────────────────┘         └──────────────────┘
```

The plugin provides a raw bidirectional byte stream. Core creates and owns the `Term<EventProxy>` and VTE parser — the same alacritty_terminal types used for local PTY. The plugin never touches alacritty_terminal.

**Flow:**
1. User clicks "Connect" in SSH plugin's panel → plugin establishes SSH connection
2. Plugin calls `host.open_session(meta, vtable, handle)` — core creates a new tab with `Term` + VTE parser
3. Plugin's SSH channel output → `output_callback(bytes)` → core feeds to VTE → Term updates → egui renders
4. User types in terminal → core sends input bytes → `vtable.write(handle, bytes)` → plugin writes to SSH channel
5. Terminal resized → `vtable.resize(handle, cols, rows)` → plugin sends window-change request to SSH server

### C ABI

```rust
/// Plugin-provided session backend
#[repr(C)]
pub struct SessionBackendVtable {
    /// Write input bytes (keystrokes) to the session
    pub write: extern "C" fn(handle: *mut c_void, buf: *const u8, len: usize),
    /// Resize the terminal
    pub resize: extern "C" fn(handle: *mut c_void, cols: u16, rows: u16),
    /// Shut down the session
    pub shutdown: extern "C" fn(handle: *mut c_void),
    /// Free the handle
    pub drop: extern "C" fn(handle: *mut c_void),
}

/// Host provides this callback during setup — plugin pushes output bytes through it
pub type OutputCallback = extern "C" fn(ctx: *mut c_void, buf: *const u8, len: usize);
```

### Session Metadata

Plugins provide metadata when opening a session so core can display meaningful tab titles:

```rust
#[repr(C)]
pub struct SessionMeta {
    pub title: *const c_char,       // e.g. "dustin@lab.nexxuscraft.com"
    pub short_title: *const c_char, // e.g. "lab" (for narrow tabs)
    pub session_type: *const c_char, // e.g. "ssh", "serial", "telnet"
    pub icon: *const c_char,        // optional icon path
}
```

---

## 5. Shared Library Plugin Architecture

### 5.1 Why Shared Libraries?

Lua is excellent for UI-driven plugins (forms, panel widgets, light scripting). But it breaks down for:

| Concern | Lua Limitation |
|---------|---------------|
| SSH connections | Can't link against russh, no async I/O |
| SFTP transfers | No streaming, no async I/O |
| Rsync integration | Can't link against `fast_rsync` crate |
| File system watching | No inotify/FSEvents binding |
| Large file listing | Table allocation overhead for 10k+ entries |
| Crypto operations | Pure-Lua crypto is 100-1000x slower |

The SSH plugin alone makes native plugins a requirement.

### 5.2 Architecture Overview

```
┌─────────────────────────────────────────────────┐
│                  conch_app (host)                │
│                                                 │
│  ┌───────────────┐  ┌────────────────────────┐  │
│  │ Plugin Host   │  │ Panel Layout Manager   │  │
│  │               │  │                        │  │
│  │ Lua Runtime   │  │ Left │ Center │ Right  │  │
│  │ Native Loader │  │      │        │        │  │
│  │ Event Bus     │  │ Bottom Panel           │  │
│  └───────┬───────┘  └────────────────────────┘  │
│          │                                       │
├──────────┼───────────────────────────────────────┤
│          │  conch_plugin_sdk (C ABI)             │
│          │                                       │
│  ┌───────┴──────────────────────────────────┐   │
│  │  Exported by host (plugin calls these):  │   │
│  │  conch_register_panel(loc, name, icon)   │   │
│  │  conch_set_widgets(handle, json, len)    │   │
│  │  conch_show_form(handle, json, len)      │   │
│  │  conch_notify(handle, msg)               │   │
│  │  conch_open_session(handle, meta, vtable)│   │
│  │  conch_publish_event(handle, type, data) │   │
│  │  conch_subscribe(handle, event_type)     │   │
│  │  conch_query_plugin(handle, target, ...) │   │
│  │  conch_get_config(handle, key)           │   │
│  │  conch_set_config(handle, key, value)    │   │
│  │                                          │   │
│  │  Expected symbols (host calls plugin):   │   │
│  │  conch_plugin_info() -> PluginInfo       │   │
│  │  conch_plugin_setup(host_api) -> handle  │   │
│  │  conch_plugin_render(handle) -> json     │   │
│  │  conch_plugin_event(handle, event_json)  │   │
│  │  conch_plugin_teardown(handle)           │   │
│  │  conch_plugin_query(handle, method, args)│   │
│  └──────────────────────────────────────────┘   │
└─────────────────────────────────────────────────┘
```

### 5.3 Plugin SDK Crate: `conch_plugin_sdk`

Shared library authors link against this crate:

```rust
#[repr(C)]
pub struct PluginInfo {
    pub name: *const c_char,
    pub description: *const c_char,
    pub version: *const c_char,
    pub plugin_type: PluginType,        // Action, Panel
    pub panel_location: PanelLocation,  // Left, Right, Bottom, None
    pub dependencies: *const *const c_char,
    pub num_dependencies: usize,
}

/// Host API vtable — function pointers the plugin calls
#[repr(C)]
pub struct HostApi {
    // Panel management
    pub register_panel: extern "C" fn(loc: PanelLocation, name: *const c_char, icon: *const c_char) -> PanelHandle,
    pub set_widgets: extern "C" fn(handle: PanelHandle, json: *const c_char, len: usize),

    // Session backends
    pub open_session: extern "C" fn(meta: *const SessionMeta, vtable: *const SessionBackendVtable, backend_handle: *mut c_void) -> SessionHandle,
    pub close_session: extern "C" fn(handle: SessionHandle),

    // Dialogs (blocking — run on plugin thread)
    pub show_form: extern "C" fn(json: *const c_char, len: usize) -> *mut c_char,
    pub show_confirm: extern "C" fn(msg: *const c_char) -> bool,
    pub show_prompt: extern "C" fn(msg: *const c_char) -> *mut c_char,

    // Notifications & logging
    pub notify: extern "C" fn(json: *const c_char, len: usize),
    pub log: extern "C" fn(level: u8, msg: *const c_char),

    // Plugin IPC
    pub publish_event: extern "C" fn(event_type: *const c_char, data_json: *const c_char, len: usize),
    pub subscribe: extern "C" fn(event_type: *const c_char),
    pub query_plugin: extern "C" fn(target: *const c_char, method: *const c_char, args_json: *const c_char, len: usize) -> *mut c_char,
    pub register_service: extern "C" fn(name: *const c_char),

    // Config persistence (per-plugin file)
    pub get_config: extern "C" fn(key: *const c_char) -> *mut c_char,
    pub set_config: extern "C" fn(key: *const c_char, value: *const c_char),

    // Menu registration
    pub register_menu_item: extern "C" fn(menu: *const c_char, label: *const c_char, action: *const c_char, keybind: *const c_char),

    // Clipboard
    pub clipboard_set: extern "C" fn(text: *const c_char),

    // Theme
    pub get_theme: extern "C" fn() -> *mut c_char,

    // Memory management
    pub free_string: extern "C" fn(ptr: *mut c_char),
}
```

### 5.4 C Header for Non-Rust Plugins

Distributed alongside releases:

```c
// include/conch_plugin.h
#ifndef CONCH_PLUGIN_H
#define CONCH_PLUGIN_H

typedef struct ConchPluginInfo { /* ... */ } ConchPluginInfo;
typedef struct ConchHostApi { /* ... */ } ConchHostApi;
typedef struct ConchSessionMeta { /* ... */ } ConchSessionMeta;
typedef struct ConchSessionBackendVtable { /* ... */ } ConchSessionBackendVtable;

// Plugin must export these symbols:
ConchPluginInfo conch_plugin_info(void);
void* conch_plugin_setup(const ConchHostApi* api);
const char* conch_plugin_render(void* handle);
void conch_plugin_event(void* handle, const char* event_json);
void conch_plugin_teardown(void* handle);
const char* conch_plugin_query(void* handle, const char* method, const char* args_json);

#endif
```

### 5.5 JSON Widget Descriptors

All widget communication uses JSON. Example for the SSH session manager:

```json
{
  "widgets": [
    {
      "type": "toolbar",
      "items": [
        { "type": "button", "id": "add_server", "icon": "plus.png", "tooltip": "Add Server" },
        { "type": "button", "id": "add_folder", "icon": "folder-plus.png", "tooltip": "Add Folder" },
        { "type": "text_input", "id": "search", "hint": "Quick connect...", "value": "" }
      ]
    },
    {
      "type": "tree_view",
      "id": "server_tree",
      "nodes": [
        {
          "id": "folder_1",
          "label": "Production",
          "icon": "folder.png",
          "expanded": true,
          "children": [
            { "id": "server_1", "label": "web-01", "icon": "server.png", "badge": "connected" },
            { "id": "server_2", "label": "db-01", "icon": "server.png" }
          ]
        }
      ]
    }
  ]
}
```

**Performance:** Widget trees are small (<50KB for a complex panel). `serde_json` parses 50KB in ~100 microseconds — <1% of a 16ms frame budget. Plugins only push new widgets on events (button click, navigation), not every frame. Core caches the parsed widget structs and repaints from cache. The terminal I/O hot path (60fps) never touches JSON. For edge cases (10K-row tables), incremental `patch_widgets` updates specific nodes by ID rather than replacing the entire tree.

### 5.6 Lua Wrapping Native Plugins

Lua plugins can call native plugin services via the message bus:

```lua
local files = app.query_plugin("sftp", "list", { session_id = sid, path = "/home" })
for _, f in ipairs(files) do
    ui.panel_label(f.name)
end
```

Or, for Rust native plugins that expose a higher-level Lua API via mlua UserData:

```lua
local sftp = require("conch.native.sftp")
local files = sftp.list(session_id, "/home")
```

---

## 6. Plugin API Expansion

### 6.1 Widget Types

Current widgets are text-only and flat. Plugins building a file browser or session manager need rich layout and interactive widgets:

| Widget | Purpose | Example |
|--------|---------|---------|
| `SplitPane` | Nested layout with adjustable ratio | File browser dual-pane |
| `TreeView` | Collapsible tree with icons, badges, context menus | Server folder tree |
| `Table` (enhanced) | Sortable, selectable, icons per cell, row context menu | File listing |
| `IconLabel` | Label with icon | File type display |
| `PathBar` | Clickable breadcrumb path segments | File browser path |
| `ContextMenu` | Right-click menu on any widget | File operations |
| `TextInput` | Single-line text input | Path editor, search, quick connect |
| `TextEdit` | Multi-line text edit | Notes plugin |
| `DropZone` | Drag-drop target area | File transfer |
| `Image` | Inline image by path or data | Plugin icons, previews |
| `Tabs` | Sub-tabs within a panel | Local/Remote in file browser |
| `Toolbar` | Horizontal button bar with icons | Back/Forward/Up/Home |
| `Checkbox` | Toggle control | Column visibility, settings |
| `ComboBox` | Dropdown selection | Protocol picker, sort order |
| `Badge` | Small status indicator on other widgets | "connected", "error" |
| `Spacer` | Flexible/fixed space | Layout control |
| `Horizontal` / `Vertical` | Layout containers | Group widgets in rows/columns |
| `ScrollArea` | Scrollable container | Long lists |
| `Heading` | Section heading | Panel sections |
| `Text` | Monospace text | Log output |
| `Label` | Standard label | Descriptions |
| `Separator` | Visual divider | Section breaks |
| `Button` | Clickable button | Actions |
| `KeyValue` | Key-value pair display | Status info |
| `Progress` | Progress bar with label | Transfer progress |
| `ScrollText` | Scrollable monospace text (stick to bottom) | Live log output |

### 6.2 Widget Events

Interactive widgets generate events sent to the plugin:

```json
{ "type": "button_click", "id": "add_server" }
{ "type": "tree_select", "id": "server_tree", "node_id": "server_1" }
{ "type": "tree_expand", "id": "server_tree", "node_id": "folder_1", "expanded": true }
{ "type": "tree_context_menu", "id": "server_tree", "node_id": "server_1", "action": "delete" }
{ "type": "text_input_changed", "id": "search", "value": "web-" }
{ "type": "text_input_submit", "id": "search", "value": "dustin@lab:22" }
{ "type": "table_select", "id": "file_list", "row": 3 }
{ "type": "table_sort", "id": "file_list", "column": "size", "ascending": false }
{ "type": "table_context_menu", "id": "file_list", "row": 3, "action": "download" }
{ "type": "drop", "id": "remote_pane", "source": "local_pane", "items": ["/path/to/file"] }
{ "type": "tab_changed", "id": "browser_tabs", "active": 1 }
{ "type": "checkbox_changed", "id": "show_hidden", "checked": true }
{ "type": "combobox_changed", "id": "sort_by", "value": "size" }
```

### 6.3 Host Commands (Both Lua and Native)

| Command | Purpose |
|---------|---------|
| `RegisterPanel { location, name, icon }` | Register a panel tab at left/right/bottom |
| `RegisterMenuItem { menu, label, action, keybind }` | Add item to app menu bar |
| `OpenSession { meta, backend }` | Open a new tab with plugin-provided session backend |
| `CloseSession { session_id }` | Close a session tab |
| `GetActiveSession` | Get active session metadata |
| `GetAllSessions` | List all open sessions |
| `SubscribeEvent { event_type }` | Subscribe to app/plugin events |
| `PublishEvent { event_type, data }` | Broadcast event to all subscribers |
| `QueryPlugin { target, method, args }` | Direct request to another plugin's service |
| `RegisterService { name }` | Declare a queryable service |
| `GetConfig { key }` / `SetConfig { key, value }` | Plugin-scoped persistent config (per-plugin file) |
| `ReadFile { path }` / `WriteFile { path, data }` | File I/O |
| `WatchPath { path }` | File system change notifications |
| `HttpRequest { url, method, headers, body }` | HTTP client for REST APIs |
| `ShowContextMenu { items }` | Context menu at cursor position |
| `GetTheme` | Current color theme for consistent widget styling |
| `RegisterKeybind { action, binding, description }` | Register keyboard shortcut |
| `SetClipboard { text }` / `GetClipboard` | Clipboard access |
| `Notify { title, body, level, duration, buttons }` | Toast notification |
| `ShowForm { title, fields }` | Blocking form dialog |
| `ShowConfirm { message }` | Blocking yes/no dialog |
| `ShowPrompt { message }` | Blocking text input dialog |
| `ShowAlert { title, message }` / `ShowError { title, message }` | Info/error alerts |
| `Log { level, message }` | Log message |

### 6.4 Plugin-to-Plugin Communication

```
┌──────────┐    events (broadcast)     ┌──────────┐
│ SSH      │───"ssh.session_ready"────►│  Files   │
│ Plugin   │   {session_id, host}      │  Plugin  │
│          │◄──"files.upload_request"──│          │
└─────┬────┘   {local_path, remote}    └────┬─────┘
      │                                     │
      │ service: "ssh.get_handle"           │ query: "sftp.list"
      ▼                                     ▼
┌──────────────────────────────────────────────────┐
│                  SFTP Plugin                     │
│  services: sftp.list, sftp.upload, sftp.download │
│  subscribes: ssh.session_ready                   │
└──────────────────────────────────────────────────┘
```

**Three IPC patterns:**

1. **Broadcast events:** Plugin publishes, any subscriber receives
   - `ssh.session_ready { session_id, host, user }` — SSH plugin connected
   - `ssh.session_closed { session_id }` — SSH session ended
   - `sftp.transfer_complete { id, path, direction }` — transfer done
   - `app.tab_changed { session_id }` — user switched terminal tabs (from core)
   - `app.theme_changed { theme_json }` — theme updated (from core)
   - `notification.shown { title, body, level }` — notification displayed (from core)

2. **Direct queries:** Plugin calls another plugin's registered service, gets response
   - `query_plugin("sftp", "list", { session_id: "...", path: "/home" })` → file listing JSON
   - `query_plugin("ssh", "get_sessions", {})` → list of active SSH sessions
   - `query_plugin("ssh", "exec", { session_id: "...", command: "ls" })` → command output

3. **Service registration:** Plugin declares capabilities other plugins can query
   - SSH plugin registers: `ssh.connect`, `ssh.exec`, `ssh.get_sessions`
   - SFTP plugin registers: `sftp.list`, `sftp.upload`, `sftp.download`, `sftp.mkdir`
   - Core validates that `query_plugin` targets a registered service

**Dependency declaration:**

In Lua plugin headers:
```lua
-- plugin-requires: ssh, sftp
```

In native plugins:
```rust
PluginInfo { dependencies: &["ssh", "sftp"], .. }
```

Plugin won't load unless dependencies are loaded first. Load order is a topological sort of the dependency graph. Circular dependencies are rejected at discovery time.

---

## 7. Panel Layout System

### 7.1 Layout Architecture

```
┌─────────────────────────────────────────────────────────┐
│                      Tab Bar                            │
├───────────┬─────────────────────────────┬───────────────┤
│           │                             │               │
│   Left    │                             │    Right      │
│   Panel   │      Terminal Center        │    Panel      │
│           │                             │               │
│  [Files]  │                             │  [Sessions]   │
│  [Git]    │                             │  [Notes]      │
│           │                             │               │
├───────────┴─────────────────────────────┴───────────────┤
│                    Bottom Panel                         │
│  [Transfers] [Output] [Tunnels]                         │
└─────────────────────────────────────────────────────────┘
```

### 7.2 Panel Behavior

- **Empty panels are hidden.** With no plugins, the user sees only the tab bar + terminal.
- **Panels appear when plugins register tabs.** If the SSH plugin registers a right panel tab, the right panel becomes visible.
- **Multiple plugins on same panel → tabbed.** If two plugins both target "left", they share the left panel as tabs.
- **Panels are resizable and hideable.** User can drag borders, toggle visibility with keyboard shortcuts.
- **User can override plugin placement** in config:

```toml
[conch.plugins.layout]
"file-explorer" = "right"   # move files to right panel
"ssh-manager" = "left"      # move sessions to left panel
```

### 7.3 Panel Registration

Via plugin header:
```lua
-- plugin-type: panel
-- plugin-panel: left
-- plugin-panel-icon: files.png
```

Via runtime API:
```lua
function setup()
    app.register_panel("left", "Files", { icon = "files.png" })
end
```

Native:
```c
host_api->register_panel(PANEL_LEFT, "Sessions", "server.png");
```

---

## 8. New Crate Structure

```
conch/
├── crates/
│   ├── conch_core/              # Config, themes, terminal settings (KEEP, strip SSH models)
│   │   ├── src/
│   │   │   ├── config.rs        # UserConfig (window, font, colors, terminal, plugin paths)
│   │   │   ├── color_scheme.rs  # Alacritty-compatible themes
│   │   │   └── lib.rs
│   │   └── Cargo.toml
│   │
│   ├── conch_pty/               # Local PTY only (EXTRACTED from conch_session)
│   │   ├── src/
│   │   │   ├── lib.rs           # LocalSession, EventProxy
│   │   │   └── pty.rs           # PTY spawning, alacritty_terminal bridge
│   │   └── Cargo.toml           # depends on alacritty_terminal, libc
│   │
│   ├── conch_plugin/            # Lua runtime + native loader + plugin API (MAJOR EXPANSION)
│   │   ├── src/
│   │   │   ├── api/             # Lua API tables (expanded with all new commands)
│   │   │   │   ├── mod.rs       # PluginCommand, PluginResponse, PanelWidget (expanded)
│   │   │   │   ├── ui.rs        # All ui.* functions
│   │   │   │   ├── app.rs       # All app.* functions (open_session, events, config, etc.)
│   │   │   │   ├── session.rs   # session.* (active session info, send/write)
│   │   │   │   ├── crypto.rs    # crypto.* (keep)
│   │   │   │   └── net.rs       # net.* (keep)
│   │   │   ├── native/          # Shared library loader (NEW)
│   │   │   │   ├── mod.rs       # dlopen, symbol resolution, lifecycle
│   │   │   │   └── bridge.rs    # HostApi vtable construction
│   │   │   ├── bus.rs           # Plugin message bus (NEW)
│   │   │   ├── runner.rs        # Lua runner (keep, expand for new commands)
│   │   │   ├── manager.rs       # Discovery + dependency resolution (expand)
│   │   │   └── checker.rs       # Static analysis (keep)
│   │   └── Cargo.toml           # mlua, tokio, libloading
│   │
│   ├── conch_plugin_sdk/        # C ABI SDK for native plugin authors (NEW)
│   │   ├── src/
│   │   │   ├── lib.rs           # Types, HostApi vtable, SessionBackendVtable, macros
│   │   │   └── widgets.rs       # Widget JSON builder helpers
│   │   ├── include/
│   │   │   └── conch_plugin.h   # C header for Go/C plugin authors
│   │   └── Cargo.toml           # minimal deps (serde_json for widget builders)
│   │
│   └── conch_app/               # GUI host (REWRITE FROM SCRATCH)
│       ├── src/
│       │   ├── main.rs          # CLI args, window setup, eframe launch
│       │   ├── app.rs           # Minimal ConchApp: tabs, terminal, plugin host, panel layout
│       │   ├── terminal/        # Terminal rendering (carry forward, clean up)
│       │   │   ├── widget.rs    # egui terminal renderer
│       │   │   ├── color.rs     # Color scheme bridge
│       │   │   └── size_info.rs # Cell size math
│       │   ├── layout/          # Panel layout manager (NEW)
│       │   │   ├── mod.rs
│       │   │   ├── panel.rs     # Left/Right/Bottom panel containers with tab strips
│       │   │   └── widget_renderer.rs  # JSON/struct widget tree → egui rendering
│       │   ├── host/            # Plugin hosting (NEW)
│       │   │   ├── mod.rs
│       │   │   ├── lua_host.rs  # Lua plugin lifecycle management
│       │   │   ├── native_host.rs  # Shared lib loading + lifecycle
│       │   │   ├── bus.rs       # Event bus dispatch (broadcast + query routing)
│       │   │   └── session_backend.rs  # Plugin-provided session backend bridge (byte stream → Term)
│       │   ├── dialogs/         # Core-owned dialogs (NEW)
│       │   │   ├── plugin_manager.rs  # Plugin install/load/unload dialog
│       │   │   └── plugin_dialog.rs   # Form/prompt/confirm/alert rendering for plugins
│       │   ├── input.rs         # Keyboard translation (carry forward)
│       │   ├── mouse.rs         # Terminal mouse handling (carry forward)
│       │   ├── icons.rs         # Icon cache (carry forward)
│       │   ├── ipc.rs           # Inter-process communication (carry forward)
│       │   ├── watcher.rs       # File watching for config hot-reload (carry forward)
│       │   ├── notifications.rs # Toast notification rendering (keep in core)
│       │   └── platform/        # OS-specific (carry forward)
│       │       ├── macos.rs
│       │       ├── linux.rs
│       │       └── windows.rs
│       └── Cargo.toml
│
├── plugins/                      # Built-in plugins (ship with release as separate downloads)
│   ├── conch-ssh/               # Native: SSH connections + session manager
│   │   ├── src/
│   │   │   ├── lib.rs           # Plugin entry points (conch_plugin_info, setup, render, etc.)
│   │   │   ├── client.rs        # SSH client (from conch_session/src/ssh/)
│   │   │   ├── session.rs       # SSH session backend (byte stream provider)
│   │   │   ├── server_tree.rs   # Widget JSON for server tree UI
│   │   │   └── config.rs        # servers.toml (ServerEntry, ServerFolder, SavedTunnel)
│   │   └── Cargo.toml           # conch_plugin_sdk, russh, russh-keys
│   │
│   ├── conch-sftp/              # Native: SFTP + rsync transfers
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── sftp.rs          # SFTP operations (from conch_session/src/sftp.rs)
│   │   │   ├── rsync.rs         # Rsync fallback (from conch_session/src/rsync.rs)
│   │   │   └── transfer.rs      # Transfer progress tracking + panel widgets
│   │   └── Cargo.toml           # conch_plugin_sdk, russh-sftp
│   │
│   ├── conch-files/             # Native: File explorer
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── browser.rs       # Dual-pane browser logic
│   │   │   ├── local.rs         # Local file system operations
│   │   │   └── widgets.rs       # Widget JSON for browser UI
│   │   └── Cargo.toml           # conch_plugin_sdk
│   │
│   ├── conch-tunnels/           # Lua: SSH tunnel management
│   │   └── tunnels.lua
│   │
│   └── conch-notifications/     # Lua: Notification history
│       └── notification-history.lua
│
├── examples/plugins/             # Community/example plugins
│   ├── notes.lua
│   ├── port-scanner.lua
│   ├── system-info.lua
│   └── encrypt-decrypt.lua
│
└── docs/
    ├── rewrite-analysis.md      # This document
    ├── plugin-guide.md          # How to write Lua plugins
    ├── native-plugin-guide.md   # How to write native plugins
    └── plugin-api-reference.md  # Full API reference
```

### Key changes from current structure:

1. **`conch_session` → split**: PTY extracted to `conch_pty` (core dep). SSH/SFTP/rsync/tunnel code moves into `plugins/conch-ssh/` and `plugins/conch-sftp/`.
2. **`conch_terminal` → deleted**: Unused GPU renderer.
3. **`conch_core` → slimmed**: Remove `models.rs` (ServerEntry, SavedTunnel, SessionsConfig) and `ssh_config.rs` — those belong to the SSH plugin.
4. **`conch_plugin` → expanded**: Native loader, message bus, dependency resolution, richer widget types, all new commands.
5. **`conch_plugin_sdk` → new**: C ABI types, macros, widget builders, C header for non-Rust authors.
6. **`conch_app` → rewritten**: ~4.5K lines instead of ~14.6K. Focused on terminal + plugin hosting + panel layout.
7. **`plugins/` → new**: Built-in native + Lua plugins shipped separately alongside release.

---

## 9. What Gets Deleted from Core

### From `conch_app` (entire crate rewritten):

| Module | Lines | Where it goes |
|--------|-------|---------------|
| `ui/sidebar.rs` | 1,223 | File browser → `conch-files` plugin; plugin list → core plugin manager dialog |
| `ui/session_panel.rs` | 734 | → `conch-ssh` plugin |
| `ui/file_browser.rs` | 389 | → `conch-files` plugin |
| `ui/bottom_panel.rs` | 229 | → generic panel layout in core |
| `ui/session_panel_plugins.rs` | 187 | → generic panel layout in core |
| `ui/dialogs/tunnels.rs` | 447 | → `conch-tunnels` Lua plugin |
| `ui/dialogs/new_connection.rs` | 392 | → `conch-ssh` plugin |
| `ui/dialogs/notification_history.rs` | 226 | → `conch-notifications` Lua plugin |
| `sidebar_handler.rs` | 448 | Actions split across plugins |
| `ssh.rs` | 482 | → `conch-ssh` plugin |
| `plugins.rs` | 973 | Rewritten as `host/` module |
| `extra_window.rs` | 1,673 | Rewritten — generic, data-driven, no feature duplication |
| `macos_menu.rs` | 268 | Rebuilt from plugin menu registrations |
| **Total moved out** | **~7,671** | |

### From `conch_session` (crate broken up):

| Module | Lines | Where it goes |
|--------|-------|---------------|
| `ssh/client.rs` | 481 | → `conch-ssh` plugin |
| `ssh/session.rs` | 339 | → `conch-ssh` plugin |
| `ssh/tunnel.rs` | 133 | → `conch-ssh` plugin (exposed to tunnels Lua plugin via service) |
| `ssh/proxy.rs` | 68 | → `conch-ssh` plugin |
| `sftp.rs` | 606 | → `conch-sftp` plugin |
| `rsync.rs` | 370 | → `conch-sftp` plugin |
| **Stays as `conch_pty`** | **~290** | `pty.rs` + `connector.rs` |

### From `conch_core`:

| What | Where it goes |
|------|---------------|
| `ServerEntry`, `ServerFolder` | → `conch-ssh` plugin config |
| `SavedTunnel` | → `conch-ssh` plugin config |
| `SessionsConfig` | → `conch-ssh` plugin config |
| `ssh_config.rs` | → `conch-ssh` plugin |
| `PersistentState.loaded_plugins` | Stays (core needs this) |

---

## 10. Estimated Size

### Core

| Component | Est. Lines | What |
|-----------|-----------|------|
| `conch_core` | ~800 | Config (window, font, colors, terminal, plugin paths) |
| `conch_pty` | ~300 | Local PTY + EventProxy |
| `conch_plugin` | ~3,000 | Lua runtime + native loader + bus + expanded API |
| `conch_plugin_sdk` | ~500 | C ABI types + macros + widget builders |
| `conch_app` | ~4,500 | Terminal host, panel layout, plugin hosting, input, multi-window |
| **Total core** | **~9,100** | Down from ~19K (current codebase including conch_session) |

### Plugins

| Plugin | Est. Lines | Type |
|--------|-----------|------|
| `conch-ssh` | ~2,000 | Native (Rust shared lib) |
| `conch-sftp` | ~1,200 | Native (Rust shared lib) |
| `conch-files` | ~1,500 | Native (Rust shared lib) |
| `conch-tunnels` | ~200 | Lua |
| `conch-notifications` | ~100 | Lua |
| **Total plugins** | **~5,000** | |

**Grand total: ~14,100 lines** — comparable to current ~16K, but with clean separation.

---

## 11. Implementation Plan

### Phase 1: Minimal Terminal App

**Goal:** A working terminal emulator with no plugin system. The foundation everything else builds on.

**Branch:** `v2` off `main`. Delete all feature code, keep only what's needed.

**Steps:**
1. Create `v2` branch, restructure workspace:
   - Keep `conch_core` (strip SSH models: `ServerEntry`, `SavedTunnel`, `SessionsConfig`, `ssh_config.rs`)
   - Extract `conch_pty` from `conch_session` (just `pty.rs` + `connector.rs`, ~290 lines)
   - Delete `conch_session` (SSH/SFTP/rsync/tunnels — will live in plugins later)
   - Delete `conch_terminal` (unused GPU renderer)
   - Gut `conch_app` — rewrite from scratch
2. Build minimal `conch_app`:
   - `main.rs` — CLI args, eframe launch
   - `app.rs` — ConchApp struct: sessions (local PTY only), tab bar, active tab, config
   - `terminal/` — carry forward terminal widget, color.rs, size_info.rs (clean up)
   - `input.rs` — keyboard handling (carry forward, strip plugin keybind logic)
   - `mouse.rs` — terminal mouse handling (carry forward)
   - `icons.rs` — icon cache (carry forward)
   - `notifications.rs` — toast notification rendering (carry forward, simplified)
   - `ipc.rs` — inter-process new-window messaging (carry forward)
   - `watcher.rs` — file watching for config hot-reload (carry forward, strip plugin watching)
   - `platform/` — macOS/Linux/Windows stubs (carry forward)
   - Multi-window support (simplified — each window owns sessions + tabs, shares config)
3. Core layout infrastructure (no panels yet, but the bones):
   - Left/Right/Bottom panel containers exist in code but are hidden (no plugins to fill them)
   - Panel show/hide toggle shortcuts wired up
4. **Verify:** App launches, local shells work, tabs work, multi-window works, config loads, themes apply.

**Deliverable:** A clean, fast, local-only terminal emulator.

### Phase 2: Plugin API + SDK

**Goal:** Full plugin infrastructure — Lua runtime, native plugin loader, message bus, session backend bridge, widget renderer, plugin management dialog. Designed with core plugins (SSH, SFTP, Files) in mind so we don't miss APIs.

**Important:** Before finalizing the API, write pseudocode/stubs for all five core plugins to validate the API surface covers every feature they need. Work backwards from plugin requirements.

**Steps:**
1. Expand `conch_plugin`:
   - Rich `PanelWidget` types (all widgets from Section 6.1 — `TreeView`, `SplitPane`, `Toolbar`, enhanced `Table`, `ContextMenu`, `TextInput`, `ComboBox`, `Checkbox`, `PathBar`, `DropZone`, etc.)
   - Widget event model (Section 6.2 — `button_click`, `tree_select`, `text_input_submit`, `table_sort`, `drop`, `checkbox_changed`, etc.)
   - Panel registration API (`RegisterPanel { location, name, icon }`)
   - Plugin-to-plugin IPC: message bus (`PublishEvent`, `SubscribeEvent`, `QueryPlugin`, `RegisterService`)
   - Plugin config persistence (`GetConfig`/`SetConfig` with per-plugin files)
   - Plugin dependency declaration + topological sort load order
   - Menu item registration (`RegisterMenuItem`)
   - Session backend bridge API (`OpenSession` with byte-stream vtable)
   - Lua API expansions for all new commands
2. Create `conch_plugin_sdk`:
   - C ABI types (`PluginInfo`, `HostApi` vtable, `SessionBackendVtable`, `SessionMeta`)
   - Rust convenience macros (`declare_plugin!`)
   - JSON widget builder helpers
   - C header file (`conch_plugin.h`)
3. Implement in `conch_app`:
   - `layout/` — panel layout manager (left/right/bottom containers with dynamic tab strips)
   - `layout/widget_renderer.rs` — JSON/struct widget tree → egui rendering (all widget types)
   - `host/lua_host.rs` — Lua plugin lifecycle (expand from current `conch_plugin` runner)
   - `host/native_host.rs` — shared library discovery, `dlopen`, symbol resolution, lifecycle
   - `host/bus.rs` — event bus dispatch (broadcast + direct query routing)
   - `host/session_backend.rs` — byte-stream bridge: plugin output → VTE parser → Term → render
   - `dialogs/plugin_manager.rs` — plugin management dialog (menu bar → dialog showing installed plugins, load/unload toggles, dependency info)
   - `dialogs/plugin_dialog.rs` — form/prompt/confirm/alert rendering for plugin-requested dialogs
4. **Test with trivial plugins:**
   - Lua plugin that registers a panel, renders widgets, responds to button clicks
   - Native plugin (Rust `.dylib`) that registers a panel, renders a tree view
   - Two plugins that communicate via the message bus
   - Native plugin that opens a session backend (mock echo server — proves the byte-stream path works)

**Deliverable:** A terminal emulator that can load, run, and manage both Lua and native plugins with full panel/widget/IPC/session-backend support.

### Phase 3: Core Plugins

**Goal:** Rebuild all current Conch functionality as plugins. Feature parity with v1.

**Steps:**
1. **`conch-ssh`** (native shared library):
   - Move SSH client code from old `conch_session/src/ssh/`
   - Implement session backend (SSH channel → byte-stream → host creates Term)
   - Server tree UI via JSON widgets (TreeView, Toolbar, ContextMenu)
   - Password prompt + host key verification via dialog API
   - Saved servers config (`~/.config/conch/plugins/ssh/servers.toml`)
   - `~/.ssh/config` parsing (moved from `conch_core`)
   - Quick-connect (text input → parse `user@host:port` → connect)
   - Publish events: `ssh.session_ready`, `ssh.session_closed`
   - Register services: `ssh.connect`, `ssh.exec`, `ssh.get_sessions`, `ssh.get_handle`

2. **`conch-sftp`** (native shared library):
   - Move SFTP code from old `conch_session/src/sftp.rs` + `rsync.rs`
   - Subscribe to `ssh.session_ready` to auto-attach SFTP to SSH sessions
   - Register services: `sftp.list`, `sftp.upload`, `sftp.download`, `sftp.mkdir`, `sftp.remove`
   - Transfer progress bottom panel (Progress widgets, Table for active transfers)
   - Rsync detection + fallback with zstd compression

3. **`conch-files`** (native shared library):
   - Dual-pane file browser (SplitPane, Table, PathBar, Toolbar)
   - Local file listing via native fs APIs
   - Remote file listing via `query_plugin("sftp", "list", ...)`
   - File operations: copy, move, delete, rename, new folder (via context menu)
   - Drag-drop between panes → triggers SFTP upload/download
   - Column toggles (name, ext, size, modified)
   - Depends on: `conch-sftp` (optional — local-only if not loaded)

4. **`conch-tunnels`** (Lua plugin):
   - UI for creating/editing/deleting port forwards
   - Queries SSH plugin for active sessions and tunnel setup
   - Bottom panel tab for tunnel status

5. **`conch-notifications`** (Lua plugin):
   - Subscribes to `notification.shown` events from core
   - Maintains history, renders viewer as bottom panel tab

6. **Verify:** Full feature parity with v1 Conch, all functionality working through plugins.

**Deliverable:** Complete Conch v2 with all features restored via plugin architecture.

---

## 12. Risk Assessment

| Risk | Severity | Mitigation |
|------|----------|------------|
| Session backend ABI complexity | **High** | Byte stream model keeps it simple — just `read`/`write`/`resize`. Test with mock echo server in Phase 2 before building SSH plugin. |
| Widget renderer completeness | **High** | Phase 2 must implement ALL widget types needed by core plugins before Phase 3 starts. Design the widget set by working backwards from SSH/Files plugin mockups. Write plugin pseudocode first. |
| JSON widget performance on large data | **Low** | Widget trees are small (<50KB). Terminal I/O bypasses JSON entirely. Add incremental `patch_widgets` if profiling shows issues. |
| Plugin load order / dependency cycles | **Medium** | Topological sort on load; reject cycles at discovery time. |
| C ABI stability across versions | **Medium** | Version the ABI; plugins declare minimum host version. |
| Blocking dialog calls from plugin threads | **Medium** | Current pattern works (channel send + recv); keep it. |
| Multi-window + plugins | **Medium** | Panels are data-driven (widgets cached centrally). Extra windows share plugin state, render from same widget cache. No UI logic duplication. |
| Missing APIs discovered during Phase 3 | **Medium** | Phase 2 explicitly designs API by working backwards from core plugin requirements. Write plugin pseudocode/stubs before finalizing SDK. |
| Distribution of shared library plugins | **Low** | Ship `.dylib`/`.so`/`.dll` alongside release. Users download core plugin pack separately. Document install paths. |

---

## 13. Summary

Conch v2 transforms a monolithic terminal+SSH+files app into a minimal terminal emulator with a rich plugin ecosystem. The core shrinks from ~19K to ~9K lines. All SSH, file browsing, SFTP, and tunnel functionality moves to plugins (3 native + 2 Lua) that ship alongside the release as optional downloads.

The key architectural innovations are:
1. **Byte-stream session backends** — plugins provide terminal I/O without touching alacritty_terminal internals, enabling SSH, Telnet, serial, Mosh, or any protocol
2. **JSON widget descriptors** — language-agnostic UI rendering across C ABI, performant for panel-sized widget trees
3. **Plugin message bus** — rich IPC (broadcast events + direct queries + service registration) enabling plugin-to-plugin collaboration at full feature parity
4. **Dynamic panel layout** — plugins register tabs at left/right/bottom, users override placement in config, panels auto-hide when empty
5. **Dual plugin runtime** — Lua for lightweight UI plugins, native shared libraries for heavy I/O (SSH, SFTP, file systems), with Lua able to call native plugin services
