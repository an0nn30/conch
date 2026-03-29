//! `session.*` Lua table — platform info, command execution, PTY write, new tab.

use mlua::prelude::*;

use super::with_host_api;

// ---------------------------------------------------------------------------
// session.* table
// ---------------------------------------------------------------------------

pub(super) fn register_session_table(lua: &Lua) -> LuaResult<()> {
    let session = lua.create_table()?;

    session.set(
        "platform",
        lua.create_function(|_lua, ()| {
            let platform = if cfg!(target_os = "macos") {
                "macos"
            } else if cfg!(target_os = "linux") {
                "linux"
            } else if cfg!(target_os = "windows") {
                "windows"
            } else {
                "unknown"
            };
            Ok(platform.to_string())
        })?,
    )?;

    // Execute a command locally (not in the terminal PTY).
    // For SSH session exec, use app.query_plugin("ssh", "exec", {...}).
    session.set(
        "exec",
        lua.create_function(|_lua, cmd: String| -> LuaResult<LuaTable> {
            let result = _lua.create_table()?;
            match std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .output()
            {
                Ok(output) => {
                    result.set(
                        "stdout",
                        String::from_utf8_lossy(&output.stdout).to_string(),
                    )?;
                    result.set(
                        "stderr",
                        String::from_utf8_lossy(&output.stderr).to_string(),
                    )?;
                    result.set("exit_code", output.status.code().unwrap_or(-1))?;
                    result.set("status", "ok")?;
                }
                Err(e) => {
                    result.set("stdout", "")?;
                    result.set("stderr", e.to_string())?;
                    result.set("exit_code", -1)?;
                    result.set("status", "error")?;
                }
            }
            Ok(result)
        })?,
    )?;

    // Get info about the currently active session.
    // Returns a table with basic session info. For detailed info about SSH
    // sessions, use app.query_plugin("ssh", "get_sessions").
    session.set(
        "current",
        lua.create_function(|_lua, ()| -> LuaResult<LuaTable> {
            let tbl = _lua.create_table()?;
            let platform = if cfg!(target_os = "macos") {
                "macos"
            } else if cfg!(target_os = "linux") {
                "linux"
            } else if cfg!(target_os = "windows") {
                "windows"
            } else {
                "unknown"
            };
            tbl.set("platform", platform)?;
            tbl.set("type", "local")?;
            Ok(tbl)
        })?,
    )?;

    // Write bytes to the focused window's active terminal session (PTY).
    // The write is queued and delivered on the next frame.
    session.set(
        "write",
        lua.create_function(|lua, text: String| {
            with_host_api(lua, |api| api.write_to_pty(text.as_bytes()))?;
            Ok(())
        })?,
    )?;

    // Open a new local shell tab in the focused window.
    // Args: (command?, plain?)
    //   command: optional string to write to the new tab's PTY
    //   plain: if true, use OS default shell ignoring terminal.shell config
    session.set(
        "new_tab",
        lua.create_function(|lua, (command, plain): (Option<String>, Option<bool>)| {
            with_host_api(lua, |api| {
                api.new_tab(command.as_deref(), plain.unwrap_or(false))
            })?;
            Ok(())
        })?,
    )?;

    lua.globals().set("session", session)?;
    Ok(())
}
