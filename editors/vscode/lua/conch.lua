--- Conch Plugin API type definitions for LuaLS.
--- This file provides autocompletion and hover docs for the Conch plugin API.
--- It is NOT executed — it only provides type annotations for the language server.

---@meta

--------------------------------------------------------------------------------
-- session — Session Interaction
--------------------------------------------------------------------------------

---@class SessionInfo
---@field id string Unique session identifier
---@field title string Session display title
---@field type string Session type: `"local"` or `"ssh"`

---@class SessionHandle : SessionInfo
---@field exec fun(cmd: string): string Execute a command silently on this session and return stdout
---@field send fun(text: string) Send raw text to this session (no trailing newline)
---@field run fun(cmd: string) Send a command + newline to this session's terminal

---@class session
---Execute a command silently on the active session and return stdout.
---
---For SSH sessions, opens a separate channel — the terminal PTY is untouched.
---For local sessions, runs via `sh -c "..."`.
---@field exec fun(cmd: string): string
---Send raw text to the active terminal (no trailing newline).
---@field send fun(text: string)
---Send a command + newline to the active terminal.
---@field run fun(cmd: string)
---Get the OS platform of the active session.
---
---Returns `"macos"`, `"linux"`, `"freebsd"`, etc.
---For SSH sessions, runs `uname -s` on the remote host.
---@field platform fun(): string
---Get info about the active (focused) session.
---@field current fun(): SessionInfo|nil
---Get info about all open sessions.
---@field all fun(): SessionInfo[]
---Get a handle to a named session. The returned handle has bound
---`exec()`, `send()`, and `run()` methods that target that specific session.
---
---Returns `nil` if no session with the given name exists.
---@field named fun(name: string): SessionHandle|nil
session = {}

--------------------------------------------------------------------------------
-- app — Application Controls
--------------------------------------------------------------------------------

---@class NotificationOptions
---@field body string Main notification text (required)
---@field title? string Optional bold heading
---@field level? "info"|"success"|"warning"|"error" Severity level (default: `"info"`)
---@field duration? number Seconds before auto-dismiss. `0` = persistent (default: `5`)
---@field buttons? string[] List of button labels — makes the call blocking

---@class app
---Open a saved SSH connection by server name or host.
---@field open_session fun(name: string)
---Copy text to the system clipboard.
---@field clipboard fun(text: string)
---Show a toast notification.
---
---**Simple form:** `app.notify("message")` — fire-and-forget info toast.
---
---**Rich form:** `app.notify({ title="Done", body="...", level="success" })`
---
---**With buttons:** `local answer = app.notify({ body="Delete?", buttons={"Yes","No"} })`
---When buttons are provided, blocks until the user clicks one and returns the label.
---@field notify fun(msg_or_opts: string|NotificationOptions): string|nil
---Log a message (visible in application logs).
---@field log fun(msg: string)
---Get a list of all configured server names.
---@field servers fun(): string[]
---Get all configured servers with `name` and `host` fields.
---@field server_details fun(): {name: string, host: string}[]
---Set the plugin's icon from a file path. The path is validated.
---Returns `true` on success, `false` on failure.
---@field set_icon fun(path: string): boolean
---Register a keybinding at runtime.
---Returns `true` on success, `false` if it conflicts with an app shortcut.
---@field register_keybind fun(action: string, binding: string, description?: string): boolean
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

---@class ui
--- **Output Panel**
---Append a line to the plugin output panel in the sidebar.
---@field append fun(text: string)
---Clear the plugin output panel.
---@field clear fun()
--- **Dialogs** (all blocking — plugin pauses until user responds)
---Show a form dialog with multiple fields.
---Returns a table mapping field names to user input, or `nil` if cancelled.
---@field form fun(title: string, fields: FormField[]): table<string, string>|nil
---Show a text input prompt. Returns the entered string, or `nil` if cancelled.
---@field prompt fun(message: string): string|nil
---Show a Yes/No confirmation dialog.
---@field confirm fun(message: string): boolean
---Show an informational alert with an OK button.
---@field alert fun(title: string, message: string)
---Show an error alert (red text) with an OK button.
---@field error fun(title: string, message: string)
---Show a read-only text viewer with a Copy button.
---@field show fun(title: string, text: string)
---Show a table viewer dialog.
---@field table fun(title: string, columns: string[], rows: string[][])
--- **Progress Indicator**
---Show a progress spinner with a message.
---@field progress fun(message: string)
---Hide the progress spinner.
---@field hide_progress fun()
--- **Panel Widgets** (for panel plugins — build declarative UI)
---Clear all accumulated panel widgets.
---@field panel_clear fun()
---Add a bold section header.
---@field panel_heading fun(text: string)
---Add a monospace text block.
---@field panel_text fun(text: string)
---Add a proportional text label.
---@field panel_label fun(text: string)
---Add a horizontal divider.
---@field panel_separator fun()
---Add a data table with columns and rows.
---@field panel_table fun(columns: string[], rows: string[][])
---Add a progress bar. `fraction` is 0.0–1.0.
---@field panel_progress fun(label: string, fraction: number, text: string)
---Add a clickable button. When clicked, triggers `on_click(id)`.
---@field panel_button fun(id: string, label: string)
---Add a key-value pair row.
---@field panel_kv fun(key: string, value: string)
---Render raw HTML in a Shadow DOM with CSS isolation.
---Theme variables (--bg, --fg, etc.) are forwarded. Elements with
---`data-action="id"` emit `button_click` events when clicked.
---@field panel_html fun(content: string, css?: string)
---Push current accumulated widgets to the frontend immediately.
---Use to show loading states during blocking operations.
---@field request_render fun()
---Set the panel auto-refresh interval in seconds. Default is 10.
---Use `0` for manual refresh only.
---@field set_refresh fun(seconds: number)
ui = {}

--------------------------------------------------------------------------------
-- crypto — Cryptography
--------------------------------------------------------------------------------

---@class crypto
---Encrypt text using AES with PBKDF2 key derivation.
---Returns a base64-encoded string containing `salt || iv || ciphertext`.
---
---Supported algorithms: `"AES-128-CBC"`, `"AES-256-CBC"`,
---`"AES-128-GCM"`, `"AES-256-GCM"`, `"AES-128-ECB"`, `"AES-256-ECB"`.
---@field encrypt fun(plaintext: string, passphrase: string, algorithm: string): string
---Decrypt a base64-encoded ciphertext produced by `crypto.encrypt()`.
---Returns the original plaintext string.
---@field decrypt fun(encoded: string, passphrase: string, algorithm: string): string
---List all supported algorithm strings.
---@field algorithms fun(): string[]
crypto = {}

--------------------------------------------------------------------------------
-- net — Networking
--------------------------------------------------------------------------------

---@class PortScanResult
---@field port integer Port number
---@field open boolean Whether the port is open

---@class net
---Check if a single TCP port is open.
---@field check_port fun(host: string, port: integer, timeout_ms?: integer): boolean
---Scan a list of ports. Returns `{port=N, open=bool}` entries.
---
---Default timeout: 1000ms. Default concurrency: 50 (max 500).
---@field scan fun(host: string, ports: integer[], timeout_ms?: integer, concurrency?: integer): PortScanResult[]
---Scan a contiguous port range. Returns only the open port numbers.
---
---Default timeout: 1000ms. Default concurrency: 100 (max 500).
---@field scan_range fun(host: string, start_port: integer, end_port: integer, timeout_ms?: integer, concurrency?: integer): integer[]
---DNS lookup. Returns a table of unique IP address strings.
---@field resolve fun(hostname: string): string[]
---Monotonic timestamp in seconds (for measuring durations).
---@field time fun(): number
net = {}

--------------------------------------------------------------------------------
-- Plugin Lifecycle Functions (Panel Plugins)
--------------------------------------------------------------------------------

---Called once when the panel plugin is first activated.
---Use for one-time initialization.
---@type fun()?
setup = nil

---Called on each refresh cycle to build the panel UI.
---Use `ui.panel_*` functions inside this to describe widgets.
---@type fun()?
render = nil

---Called when a panel button is clicked.
---@type fun(button_id: string)?
on_click = nil

---Called when a custom keybinding is triggered.
---@type fun(action: string)?
on_keybind = nil
