--- Conch Plugin SDK — ui.* API
--- LuaLS type definitions for autocompletion and hover docs.
--- https://github.com/an0nn30/rusty_conch
---
--- These are stubs only — do NOT require() this file in your plugin.

---@meta

---@class ui
---The `ui` table provides widget functions for building plugin panels,
---layout containers, and modal dialogs.
ui = {}

-- ═══════════════════════════════════════════════════════════════════════
-- Accumulator
-- ═══════════════════════════════════════════════════════════════════════

---Clear all accumulated widgets. Call at the start of `render()`.
function ui.panel_clear() end

-- ═══════════════════════════════════════════════════════════════════════
-- Data Display
-- ═══════════════════════════════════════════════════════════════════════

---Render a heading.
---@param text string
function ui.panel_heading(text) end

---Render a styled label.
---@param text string
---@param style? "normal"|"secondary"|"muted"|"accent"|"warn"|"error"
function ui.panel_label(text, style) end

---Render plain text.
---@param text string
function ui.panel_text(text) end

---Render scrollable text area.
---@param id string Unique widget ID
---@param text string Content to display
---@param max_height? number Maximum height in pixels
function ui.panel_scroll_text(id, text, max_height) end

---Render a key-value pair (label: value).
---@param key string
---@param value string
function ui.panel_kv(key, value) end

---Render a horizontal separator line.
function ui.panel_separator() end

---Render vertical spacing.
---@param size? number Spacing in pixels (default: theme spacing)
function ui.panel_spacer(size) end

---Render an icon followed by a label.
---@param icon string Icon name (e.g. "folder", "server")
---@param text string Label text
---@param style? "normal"|"secondary"|"muted"|"accent"|"warn"|"error"
function ui.panel_icon_label(icon, text, style) end

---Render a colored badge.
---@param text string Badge text
---@param variant "info"|"success"|"warn"|"error"
function ui.panel_badge(text, variant) end

---Render a progress bar.
---@param id string Unique widget ID
---@param fraction number Progress from 0.0 to 1.0
---@param label? string Optional text label
function ui.panel_progress(id, fraction, label) end

---Render an image.
---@param id? string Unique widget ID
---@param src string Image path or URL
---@param width? number Display width in pixels
---@param height? number Display height in pixels
function ui.panel_image(id, src, width, height) end

-- ═══════════════════════════════════════════════════════════════════════
-- Interactive Widgets
-- ═══════════════════════════════════════════════════════════════════════

---Render a clickable button. Fires `button_click` event with `id`.
---@param id string Unique widget ID (received in on_event)
---@param label string Button text
---@param icon? string Optional icon name
function ui.panel_button(id, label, icon) end

---Render a single-line text input. Fires `text_changed` / `text_submit` events.
---@param id string Unique widget ID
---@param value string Current value
---@param hint? string Placeholder text
function ui.panel_text_input(id, value, hint) end

---Render a multi-line text editor.
---@param id string Unique widget ID
---@param value string Current value
---@param hint? string Placeholder text
---@param lines? integer Number of visible lines
function ui.panel_text_edit(id, value, hint, lines) end

---Render a checkbox. Fires `checkbox_changed` event.
---@param id string Unique widget ID
---@param label string Checkbox label
---@param checked boolean Current checked state
function ui.panel_checkbox(id, label, checked) end

---Render a dropdown combobox. Fires `combobox_changed` event.
---@param id string Unique widget ID
---@param selected string Currently selected value
---@param options (string|ConchComboBoxOption)[] List of options
function ui.panel_combobox(id, selected, options) end

-- ═══════════════════════════════════════════════════════════════════════
-- Complex Widgets
-- ═══════════════════════════════════════════════════════════════════════

---Render a data table.
---
---Simple form:
---```lua
---ui.panel_table({"Name", "Value"}, {{"foo", "42"}, {"bar", "7"}})
---```
---
---Advanced form:
---```lua
---ui.panel_table({
---  id = "my_table",
---  columns = {{id="name", label="Name", sortable=true}},
---  rows = {{id="r1", cells={"foo", "42"}}},
---  sort_column = "name",
---  sort_ascending = true,
---  selected_row = "r1",
---})
---```
---@param columns string[]|ConchAdvancedTable Column names (simple) or config table (advanced)
---@param rows? string[][] Row data (simple form only)
function ui.panel_table(columns, rows) end

---Render a tree view. Fires `tree_select` and `context_menu_click` events.
---@param id string Unique widget ID
---@param nodes ConchTreeNode[] Tree node hierarchy
---@param selected? string ID of the selected node
function ui.panel_tree(id, nodes, selected) end

---Render a toolbar with buttons, inputs, separators, and spacers.
---@param id? string Optional toolbar ID
---@param items ConchToolbarItem[] Toolbar items
function ui.panel_toolbar(id, items) end

---Render a breadcrumb path bar. Fires `path_segment_click` event.
---@param id string Unique widget ID
---@param segments string[] Path segments (e.g. {"home", "user", "docs"})
function ui.panel_path_bar(id, segments) end

---Render a tabbed container.
---@param id string Unique widget ID
---@param active integer 0-based index of the active tab
---@param tabs ConchTabPane[] Tab definitions
function ui.panel_tabs(id, active, tabs) end

---Render raw HTML in a Shadow DOM with CSS isolation.
---Theme variables (--bg, --fg, --green, etc.) are forwarded into the shadow root.
---Elements with `data-action="id"` emit `button_click` events when clicked.
---@param content string Raw HTML to render
---@param css? string Optional CSS injected into the shadow root
function ui.panel_html(content, css) end

-- ═══════════════════════════════════════════════════════════════════════
-- Layout Containers
-- ═══════════════════════════════════════════════════════════════════════

---Horizontal layout container. Widgets added inside `fn` are laid out in a row.
---@param fn fun() Builder function — call ui.* inside to add children
---@param spacing? number Pixel spacing between children
function ui.panel_horizontal(fn, spacing) end

---Vertical layout container. Widgets added inside `fn` are stacked.
---@param fn fun() Builder function — call ui.* inside to add children
---@param spacing? number Pixel spacing between children
function ui.panel_vertical(fn, spacing) end

---Scrollable area container.
---@param fn fun() Builder function — call ui.* inside to add children
---@param max_height? number Maximum height in pixels before scrolling
function ui.panel_scroll_area(fn, max_height) end

---Drop zone container for drag-and-drop. Fires `drop` event.
---@param id string Unique widget ID
---@param label string Drop zone label
---@param fn? fun() Optional builder function for child widgets
function ui.panel_drop_zone(id, label, fn) end

-- ═══════════════════════════════════════════════════════════════════════
-- Render Control
-- ═══════════════════════════════════════════════════════════════════════

---Push the current accumulated widgets to the frontend immediately.
---Use this to show loading/progress states during blocking operations
---like `session.exec_local()`.
---
---```lua
---switching = true
---render()              -- rebuild widgets with loading state
---ui.request_render()   -- push to frontend now
---session.exec_local(cmd)   -- blocks
---switching = false
---render()
---ui.request_render()   -- push final state
---```
function ui.request_render() end

-- ═══════════════════════════════════════════════════════════════════════
-- Dialogs (blocking — pauses plugin until user responds)
-- ═══════════════════════════════════════════════════════════════════════

---Show a form dialog with multiple fields. Blocks until submitted or cancelled.
---
---```lua
---local result = ui.form("New Connection", {
---  {id="host", label="Host", type="text", default="localhost"},
---  {id="port", label="Port", type="text", default="22"},
---  {id="auth", label="Auth", type="combo", options={"password","key"}},
---})
---if result then
---  print(result.host, result.port)
---end
---```
---@param title string Dialog title
---@param fields ConchFormField[] Form field definitions
---@return table<string, string>|nil results Field values keyed by id, or nil if cancelled
function ui.form(title, fields) end

---Show an informational alert dialog. Blocks until dismissed.
---@param title string Dialog title
---@param message string Alert message
function ui.alert(title, message) end

---Show an error dialog. Blocks until dismissed.
---@param title string Dialog title
---@param message string Error message
function ui.error(title, message) end

---Show a yes/no confirmation dialog. Blocks until user responds.
---@param message string Confirmation prompt
---@return boolean confirmed true if user clicked Yes
function ui.confirm(message) end

---Show a text input prompt dialog. Blocks until user responds.
---@param message string Prompt message
---@param default? string Default input value
---@return string|nil result User input, or nil if cancelled
function ui.prompt(message, default) end

-- ═══════════════════════════════════════════════════════════════════════
-- Types
-- ═══════════════════════════════════════════════════════════════════════

---@class ConchComboBoxOption
---@field value string Option value
---@field label string Display label

---@class ConchTreeNode
---@field id string Unique node ID
---@field label string Display text
---@field icon? string Icon name
---@field icon_color? string Icon color (hex or named)
---@field bold? boolean Render label in bold
---@field badge? string Badge text shown after label
---@field expanded? boolean Whether child nodes are visible
---@field children? ConchTreeNode[] Child nodes
---@field context_menu? ConchContextMenuItem[] Right-click menu items

---@class ConchContextMenuItem
---@field id string Action ID (sent in context_menu_click event)
---@field label string Menu item text
---@field icon? string Icon name
---@field enabled? boolean Whether the item is clickable (default true)
---@field shortcut? string Shortcut hint text (display only)

---@class ConchToolbarItem
---@field type "button"|"separator"|"spacer"|"text_input" Item type (default "button")
---@field id? string Widget ID (required for button and text_input)
---@field icon? string Button icon name
---@field label? string Button label
---@field tooltip? string Hover tooltip
---@field enabled? boolean Whether the button is clickable (default true)
---@field value? string Text input current value
---@field hint? string Text input placeholder

---@class ConchTabPane
---@field label string Tab title
---@field icon? string Tab icon

---@class ConchFormField
---@field id string Field ID (key in the result table)
---@field label string Display label
---@field type "text"|"password"|"combo"|"checkbox" Field type
---@field default? string Default value
---@field options? string[] Options for combo fields

---@class ConchAdvancedTable
---@field id? string Table ID
---@field columns ConchTableColumn[] Column definitions
---@field rows ConchTableRow[] Row definitions
---@field sort_column? string Column ID to sort by
---@field sort_ascending? boolean Sort direction
---@field selected_row? string ID of the selected row

---@class ConchTableColumn
---@field id string Column ID
---@field label string Column header text
---@field sortable? boolean Whether the column is sortable
---@field width? number Column width in pixels
---@field visible? boolean Whether the column is visible (default true)

---@class ConchTableRow
---@field id string Row ID
---@field cells string[] Cell values (one per column)
---@field context_menu? ConchContextMenuItem[] Right-click menu items

return ui
