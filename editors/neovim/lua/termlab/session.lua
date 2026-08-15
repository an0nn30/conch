--- TermLab Plugin SDK — session.* API
--- LuaLS type definitions for autocompletion and hover docs.
--- https://github.com/an0nn30/conch
---
--- These are stubs only — do NOT require() this file in your plugin.

---@meta

---@class session
---The `session` table provides access to the terminal session, local
---command execution, and system information.
session = {}

---Get the current platform.
---@return "macos"|"linux"|"windows"|"unknown"
function session.platform() end

---Execute a shell command locally (not in the terminal PTY).
---For SSH session commands, use `app.query_plugin("ssh", "exec", {...})`.
---
---```lua
---local result = session.exec("hostname")
---print(result.stdout)
---```
---@param cmd string Shell command to execute (run via `sh -c`)
---@return TermLabExecResult
function session.exec(cmd) end

---Get information about the currently active terminal session.
---@return TermLabSessionInfo
function session.current() end

---Write text to the focused window's active terminal PTY.
---The write is queued and delivered on the next frame.
---@param text string Text/bytes to write to the terminal
function session.write(text) end

---Open a new local shell tab in the focused window.
---@param command? string Optional command to write to the new tab's PTY
---@param plain? boolean If true, use OS default shell ignoring terminal.shell config
function session.new_tab(command, plain) end

-- ═══════════════════════════════════════════════════════════════════════
-- Types
-- ═══════════════════════════════════════════════════════════════════════

---@class TermLabExecResult
---@field stdout string Standard output
---@field stderr string Standard error
---@field exit_code integer Process exit code (-1 on spawn error)
---@field status "ok"|"error" Whether the command launched successfully

---@class TermLabSessionInfo
---@field platform "macos"|"linux"|"windows"|"unknown" Current platform
---@field type "local" Session type

return session
