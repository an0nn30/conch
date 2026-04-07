--- Conch Plugin SDK — app.* API
--- LuaLS type definitions for autocompletion and hover docs.
--- https://github.com/an0nn30/rusty_conch
---
--- These are stubs only — do NOT require() this file in your plugin.

---@meta

---@class app
---The `app` table provides host interaction: logging, clipboard, events,
---notifications, config persistence, menus, and inter-plugin communication.
app = {}

---Write a log message (visible in app.log file).
---@param level "trace"|"debug"|"info"|"warn"|"error"
---@param message string
function app.log(level, message) end

---Copy text to the system clipboard.
---@param text string
function app.clipboard(text) end

---Read text from the system clipboard.
---@return string|nil text Clipboard contents, or nil if empty/unavailable
function app.clipboard_get() end

---Publish an event on the plugin bus.
---Other plugins that called `app.subscribe(event_type)` will receive it
---via their `on_event()` callback.
---@param event_type string Event name (e.g. "my_plugin.data_ready")
---@param data any JSON-serializable data (table, string, number, etc.)
function app.publish(event_type, data) end

---Subscribe to events on the plugin bus.
---Matching events will be delivered to your `on_event()` callback with
---`event.kind == "bus"`.
---@param event_type string Event name to subscribe to
function app.subscribe(event_type) end

---Show a toast notification in the app.
---@param title string Notification title
---@param body string Notification body text
---@param level? "info"|"success"|"warn"|"error" Notification level (default "info")
---@param duration_ms? integer How long to show in ms (default 3000, 0 = persistent)
function app.notify(title, body, level, duration_ms) end

---Register a named service that other plugins can query.
---Once registered, other plugins can call `app.query_plugin(your_name, method, args)`.
---@param name string Service name
function app.register_service(name) end

---Register a menu item in the app's menu bar.
---When clicked, your `on_event()` receives `{kind="menu_action", action=action}`.
---@param menu string Menu to add to (e.g. "Tools", "View")
---@param label string Menu item label
---@param action string Action ID sent in the menu_action event
---@param keybind? string Keyboard shortcut (e.g. "cmd+shift+t")
function app.register_menu_item(menu, label, action, keybind) end

---Register a plugin-owned settings section in the host Settings UI.
---Pass a Lua table/object with keys like `id`, `label`, `description`,
---`keywords`, `view_id`, and searchable `settings`.
---@param section table Settings section descriptor
function app.register_settings_section(section) end

---Send a query to another plugin and wait for a response.
---The target plugin must have registered as a service and must handle
---the query in its `on_query(method, args)` callback.
---@param target string Target plugin name
---@param method string Method name
---@param args? any JSON-serializable arguments
---@return string|nil response JSON response string, or nil on error/timeout
function app.query_plugin(target, method, args) end

---Read a persisted config value for this plugin.
---Values are stored as JSON files in the plugin's config directory.
---@param key string Config key
---@return string|nil value JSON string, or nil if not set
function app.get_config(key) end

---Persist a config value for this plugin.
---@param key string Config key
---@param value string JSON string to store
function app.set_config(key, value) end

---Read the effective value for a plugin setting key.
---Includes unsaved host Settings drafts when called from a settings view.
---@param key string Setting key
---@return string|nil value JSON string, or nil if not set
function app.get_setting_value(key) end

---Stage a plugin setting draft value for host Settings Apply.
---Use `nil` to clear the setting value.
---@param key string Setting key
---@param value? string JSON string to stage, or nil to clear
function app.set_setting_draft(key, value) end

return app
