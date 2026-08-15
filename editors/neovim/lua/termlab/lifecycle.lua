--- TermLab Plugin SDK — Plugin Lifecycle & Event Types
--- LuaLS type definitions for autocompletion and hover docs.
--- https://github.com/an0nn30/conch
---
--- These are stubs only — do NOT require() this file in your plugin.

---@meta

-- ═══════════════════════════════════════════════════════════════════════
-- Plugin Lifecycle Functions
-- ═══════════════════════════════════════════════════════════════════════
-- Define these as global functions in your plugin file.

---Called once when the plugin starts, after the source file is loaded.
---Use this to subscribe to events, register menu items, initialize state, etc.
---
---```lua
---function setup()
---  app.subscribe("app.tab_changed")
---  app.register_menu_item("Tools", "My Action", "my_action", "cmd+shift+m")
---end
---```
---@type fun()
setup = nil

---Called repeatedly to build the plugin's panel UI (panel plugins only).
---Must call `ui.panel_clear()` first, then add widgets with `ui.*` functions.
---The host calls this at a regular interval (typically every 1–2 seconds).
---
---```lua
---function render()
---  ui.panel_clear()
---  ui.panel_heading("My Plugin")
---  ui.panel_button("btn1", "Click Me")
---end
---```
---@type fun()
render = nil

---Called when the plugin receives an event.
---Events come from widget interactions, menu actions, bus subscriptions,
---and other plugin queries.
---
---```lua
---function on_event(event)
---  if event.kind == "widget" and event.type == "button_click" then
---    app.log("info", "Button clicked: " .. event.id)
---  elseif event.kind == "menu_action" then
---    app.log("info", "Menu action: " .. event.action)
---  elseif event.kind == "bus" then
---    app.log("info", "Bus event: " .. event.event_type)
---  end
---end
---```
---@type fun(event: TermLabEvent)
on_event = nil

---Called when another plugin sends a query via `app.query_plugin()`.
---Return a JSON string as the response.
---
---```lua
---function on_query(method, args)
---  if method == "get_status" then
---    return '{"status": "ok"}'
---  end
---end
---```
---@type fun(method: string, args: string): string|nil
on_query = nil

-- ═══════════════════════════════════════════════════════════════════════
-- Event Types
-- ═══════════════════════════════════════════════════════════════════════

---@alias TermLabEvent TermLabWidgetEvent|TermLabMenuActionEvent|TermLabBusEvent

---Widget interaction event (button click, text change, tree select, etc.).
---@class TermLabWidgetEvent
---@field kind "widget"
---@field type "button_click"|"text_changed"|"text_submit"|"checkbox_changed"|"combobox_changed"|"tree_select"|"context_menu_click"|"table_row_click"|"table_sort"|"path_segment_click"|"tab_changed"|"drop"|"toolbar_button_click"|"toolbar_text_changed"|"toolbar_text_submit"
---@field id string Widget ID that triggered the event
---@field value? string New value (for text_changed, combobox_changed, etc.)
---@field checked? boolean New state (for checkbox_changed)
---@field row_id? string Row ID (for table_row_click)
---@field column? string Column ID (for table_sort)
---@field ascending? boolean Sort direction (for table_sort)
---@field node_id? string Node ID (for tree_select)
---@field index? integer Segment or tab index (0-based)
---@field action_id? string Context menu action ID

---Menu action event (from app menu bar or registered keybinding).
---@class TermLabMenuActionEvent
---@field kind "menu_action"
---@field action string The action ID registered via `app.register_menu_item()`

---Plugin bus event (from `app.publish()` by another plugin or the host).
---@class TermLabBusEvent
---@field kind "bus"
---@field event_type string Event type string
---@field data any JSON-decoded event payload
