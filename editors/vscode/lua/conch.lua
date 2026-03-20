--- Conch Plugin API type definitions for LuaLS.
--- This file provides autocompletion and hover docs for the Conch plugin API.
--- It is NOT executed — it only provides type annotations for the language server.

---@meta

--------------------------------------------------------------------------------
-- session — Session Interaction
--------------------------------------------------------------------------------

---@class ExecResult
---@field stdout string Standard output
---@field stderr string Standard error
---@field exit_code integer Process exit code (-1 on error)
---@field status "ok"|"error" Whether the command ran successfully

---@class SessionInfo
---@field platform string OS platform: `"macos"`, `"linux"`, `"windows"`, `"unknown"`
---@field type string Session type: `"local"`

---@class session
---Execute a command locally via `sh -c "..."`.
---
---Returns a table with `stdout`, `stderr`, `exit_code`, and `status` fields.
---@field exec fun(cmd: string): ExecResult
---Write raw text to the focused window's active terminal PTY.
---The write is queued and delivered on the next frame.
---@field write fun(text: string)
---Get the OS platform of the local session.
---
---Returns `"macos"`, `"linux"`, `"windows"`, or `"unknown"`.
---@field platform fun(): string
---Get info about the active (focused) session.
---Returns a table with `platform` and `type` fields.
---@field current fun(): SessionInfo
---Open a new local shell tab in the focused window.
---@field new_tab fun(command?: string, plain?: boolean)
session = {}

--------------------------------------------------------------------------------
-- app — Application Controls
--------------------------------------------------------------------------------

---@class app
---Log a message at the given severity level (visible in application logs).
---
---Levels: `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"`.
---@field log fun(level: string, msg: string)
---Copy text to the system clipboard.
---@field clipboard fun(text: string)
---Get the current system clipboard contents.
---@field clipboard_get fun(): string
---Show a toast notification.
---
---```lua
---app.notify("Done", "Build finished", "success", 5000)
---```
---@field notify fun(title: string, body: string, level?: string, duration_ms?: number)
---Publish an event to the plugin event bus.
---Other plugins that have subscribed to `event_type` will receive it.
---@field publish fun(event_type: string, data: any)
---Subscribe to events of a given type on the plugin event bus.
---Events will be delivered to your `on_event()` function.
---@field subscribe fun(event_type: string)
---Send a query to another plugin and get a response.
---The target plugin must have registered the service and have an `on_query()` handler.
---@field query_plugin fun(target: string, method: string, args?: any): string|nil
---Register a named service so other plugins can query this plugin.
---@field register_service fun(name: string)
---Add a menu item to the application menu bar.
---When clicked, triggers `on_event()` with a `menu_action` event.
---@field register_menu_item fun(menu: string, label: string, action: string, keybind?: string)
---Read a plugin config value from persistent storage.
---@field get_config fun(key: string): string|nil
---Write a plugin config value to persistent storage.
---@field set_config fun(key: string, value: string)
app = {}

--------------------------------------------------------------------------------
-- ui — User Interface
--------------------------------------------------------------------------------

---@class FormFieldText
---@field type "text"
---@field name string Field name (used as key in result table)
---@field label? string Display label
---@field default? string Default value

---@class FormFieldPassword
---@field type "password"
---@field name string Field name
---@field label? string Display label

---@class FormFieldCombo
---@field type "combo"
---@field name string Field name
---@field label? string Display label
---@field options string[] Dropdown options
---@field default? string Default selected value

---@class FormFieldCheckbox
---@field type "checkbox"
---@field name string Field name
---@field label? string Display label
---@field default? boolean Default checked state

---@class FormFieldSeparator
---@field type "separator"

---@class FormFieldLabel
---@field type "label"
---@field text string Static text (italic, not editable)

---@alias FormField FormFieldText|FormFieldPassword|FormFieldCombo|FormFieldCheckbox|FormFieldSeparator|FormFieldLabel

---@class ComboBoxOption
---@field value string Option value
---@field label string Display label

---@class TreeNode
---@field id string Unique node identifier
---@field label string Display text
---@field icon? string Icon name
---@field icon_color? string Icon CSS color
---@field bold? boolean Render label in bold
---@field badge? string Badge text
---@field expanded? boolean Whether child nodes are visible
---@field children? TreeNode[] Child nodes
---@field context_menu? ContextMenuItem[] Right-click menu items

---@class ContextMenuItem
---@field id string Action identifier
---@field label string Display text
---@field icon? string Icon name
---@field enabled? boolean Whether the item is clickable (default true)
---@field shortcut? string Keyboard shortcut hint text

---@class ToolbarButton
---@field type? "button" Item type (default)
---@field id string Action identifier
---@field icon? string Icon name
---@field label? string Button label
---@field tooltip? string Hover text
---@field enabled? boolean Whether the button is clickable

---@class ToolbarTextInput
---@field type "text_input"
---@field id string Input identifier
---@field value? string Current text
---@field hint? string Placeholder text

---@class ToolbarSeparator
---@field type "separator"

---@class ToolbarSpacer
---@field type "spacer"

---@alias ToolbarItem ToolbarButton|ToolbarTextInput|ToolbarSeparator|ToolbarSpacer

---@class TabPane
---@field label string Tab label
---@field icon? string Tab icon

---@class TableColumn
---@field id string Column identifier
---@field label string Column header text
---@field sortable? boolean Whether the column is sortable
---@field width? number Column width
---@field visible? boolean Whether the column is visible

---@class TableRow
---@field id? string Row identifier (auto-generated if omitted)
---@field cells string[] Cell values

---@class AdvancedTableOpts
---@field id? string Table identifier (default: `"table"`)
---@field columns TableColumn[] Column definitions
---@field rows TableRow[] Row data
---@field sort_column? string Currently sorted column id
---@field sort_ascending? boolean Sort direction
---@field selected_row? string Currently selected row id

---@class ui
--- **Dialogs** (all blocking — plugin pauses until user responds)
---Show a form dialog with multiple fields.
---Returns a table mapping field names to user input, or `nil` if cancelled.
---@field form fun(title: string, fields: FormField[]): table<string, string>|nil
---Show a text input prompt. Returns the entered string, or `nil` if cancelled.
---@field prompt fun(message: string, default?: string): string|nil
---Show a Yes/No confirmation dialog.
---@field confirm fun(message: string): boolean
---Show an informational alert with an OK button.
---@field alert fun(title: string, message: string)
---Show an error alert (red text) with an OK button.
---@field error fun(title: string, message: string)
--- **Panel Widgets** (for panel plugins — build declarative UI in `render()`)
---Clear all accumulated panel widgets.
---@field panel_clear fun()
---Add a bold section header.
---@field panel_heading fun(text: string)
---Add a monospace text block.
---@field panel_text fun(text: string)
---Add a proportional text label.
---
---Style: `"normal"`, `"secondary"`, `"muted"`, `"accent"`, `"warn"`, `"error"`.
---@field panel_label fun(text: string, style?: string)
---Add a scrollable monospace text block.
---@field panel_scroll_text fun(id: string, text: string, max_height?: number)
---Add a key-value pair row.
---@field panel_kv fun(key: string, value: string)
---Add a horizontal divider.
---@field panel_separator fun()
---Add vertical spacing.
---@field panel_spacer fun(size?: number)
---Add an icon + text label.
---
---Style: `"normal"`, `"secondary"`, `"muted"`, `"accent"`, `"warn"`, `"error"`.
---@field panel_icon_label fun(icon: string, text: string, style?: string)
---Add a colored badge.
---
---Variant: `"info"`, `"success"`, `"warn"`, `"error"`.
---@field panel_badge fun(text: string, variant: string)
---Add a progress bar. `fraction` is 0.0–1.0.
---@field panel_progress fun(id: string, fraction: number, label?: string)
---Add an image.
---@field panel_image fun(id?: string, src: string, width?: number, height?: number)
---Add a clickable button. When clicked, triggers `on_event()` with a button click event.
---@field panel_button fun(id: string, label: string, icon?: string)
---Add a single-line text input field.
---@field panel_text_input fun(id: string, value: string, hint?: string)
---Add a multi-line text editor.
---@field panel_text_edit fun(id: string, value: string, hint?: string, lines?: number)
---Add a checkbox. When toggled, triggers `on_event()`.
---@field panel_checkbox fun(id: string, label: string, checked: boolean)
---Add a dropdown combobox.
---Options can be strings or `{value=..., label=...}` tables.
---@field panel_combobox fun(id: string, selected: string, options: (string|ComboBoxOption)[])
---Add a data table.
---
---Simple form: `ui.panel_table({"Col1", "Col2"}, {{"a", "b"}, {"c", "d"}})`
---
---Advanced form: `ui.panel_table({id="t", columns={...}, rows={...}})`
---@field panel_table fun(columns: string[]|AdvancedTableOpts, rows?: string[][])
---Add a tree view with expandable nodes.
---@field panel_tree fun(id: string, nodes: TreeNode[], selected?: string)
---Add a toolbar with buttons, inputs, separators, and spacers.
---@field panel_toolbar fun(id?: string, items: ToolbarItem[])
---Add a breadcrumb-style path bar. Clicking a segment triggers `on_event()`.
---@field panel_path_bar fun(id: string, segments: string[])
---Add a tabbed container.
---@field panel_tabs fun(id: string, active: integer, tabs: TabPane[])
---Create a horizontal layout container. Call `ui.panel_*` inside the function
---to add children side-by-side.
---@field panel_horizontal fun(func: fun(), spacing?: number)
---Create a vertical layout container. Call `ui.panel_*` inside the function
---to stack children vertically.
---@field panel_vertical fun(func: fun(), spacing?: number)
---Create a scrollable container with an optional max height.
---@field panel_scroll_area fun(func: fun(), max_height?: number)
---Create a file drop zone. Optionally nest child widgets inside.
---@field panel_drop_zone fun(id: string, label: string, func?: fun())
ui = {}

--------------------------------------------------------------------------------
-- net — Networking
--------------------------------------------------------------------------------

---@class PortScanResult
---@field port integer Port number
---@field open boolean Whether the port is open (always `true` — only open ports are returned)

---@class net
---Monotonic timestamp in seconds (for measuring durations).
---@field time fun(): number
---DNS lookup. Returns a table of IP address strings.
---@field resolve fun(hostname: string): string[]
---Scan a list of TCP ports. Returns entries for open ports only.
---
---Default timeout: 1000 ms.
---@field scan fun(host: string, ports: integer[], timeout_ms?: integer): PortScanResult[]
net = {}

--------------------------------------------------------------------------------
-- Plugin Lifecycle Functions
--------------------------------------------------------------------------------

---Called once when the plugin is first activated.
---Use for one-time initialization.
---@type fun()?
setup = nil

---Called on each refresh cycle to build the panel UI.
---Use `ui.panel_*` functions inside this to describe widgets.
---@type fun()?
render = nil

---Called when any plugin event occurs: button clicks, checkbox changes,
---text input submissions, combobox changes, menu actions, bus events, etc.
---
---The event is a table with a `kind` field indicating the event type:
---`"button_click"`, `"checkbox_changed"`, `"text_input_submit"`,
---`"combobox_changed"`, `"menu_action"`, `"bus_event"`,
---`"tree_select"`, `"tree_context_menu"`, `"table_sort"`,
---`"table_header_context_menu"`, `"path_bar_navigate"`,
---`"tab_changed"`, `"drop"`, etc.
---@type fun(event: table)?
on_event = nil

---Called when another plugin sends a query via `app.query_plugin()`.
---Return a JSON string with the response.
---@type fun(method: string, args: string): string?
on_query = nil

---Called when the plugin is being unloaded / disabled.
---Use for cleanup.
---@type fun()?
teardown = nil
