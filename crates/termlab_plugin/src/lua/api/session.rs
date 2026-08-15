//! `session.*` Lua table — platform info, command execution, PTY write, new tab.

use mlua::prelude::*;
use serde_json::Value as JsonValue;

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

    // Execute a command on the host shell (local only).
    session.set(
        "exec_local",
        lua.create_function(|lua, cmd: String| -> LuaResult<LuaTable> {
            exec_local_impl(lua, &cmd)
        })?,
    )?;

    // Backward-compatible alias of `session.exec_local()`.
    session.set(
        "exec",
        lua.create_function(|lua, cmd: String| -> LuaResult<LuaTable> {
            exec_local_impl(lua, &cmd)
        })?,
    )?;

    // Execute a command on the currently active session:
    // - active SSH pane: remote exec over SSH
    // - active local pane: local host shell exec
    session.set(
        "exec_active",
        lua.create_function(|lua, cmd: String| -> LuaResult<LuaTable> {
            let allowed = with_host_api(lua, |api| api.check_permission("session.exec"))?;
            if !allowed {
                return build_exec_error(lua, "permission denied: session.exec");
            }

            let tbl = lua.create_table()?;
            match with_host_api(lua, |api| api.exec_active_session(&cmd))? {
                Some(json) => {
                    if let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(&json) {
                        set_lua_table_from_json_map(&tbl, map)?;
                        if tbl.get::<Option<String>>("status")?.is_none() {
                            tbl.set("status", "ok")?;
                        }
                        if tbl.get::<Option<i64>>("exit_code")?.is_none() {
                            tbl.set("exit_code", 0)?;
                        }
                        if tbl.get::<Option<String>>("stdout")?.is_none() {
                            tbl.set("stdout", "")?;
                        }
                        if tbl.get::<Option<String>>("stderr")?.is_none() {
                            tbl.set("stderr", "")?;
                        }
                        return Ok(tbl);
                    }
                    build_exec_error(lua, "host returned invalid exec response")
                }
                None => build_exec_error(lua, "active session execution unavailable"),
            }
        })?,
    )?;

    // Get info about the currently active session.
    session.set(
        "current",
        lua.create_function(|lua, ()| -> LuaResult<LuaTable> {
            let tbl = lua.create_table()?;
            let platform = if cfg!(target_os = "macos") {
                "macos"
            } else if cfg!(target_os = "linux") {
                "linux"
            } else if cfg!(target_os = "windows") {
                "windows"
            } else {
                "unknown"
            };

            if let Some(json) = with_host_api(lua, |api| api.get_active_session())?
                && let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(&json)
            {
                set_lua_table_from_json_map(&tbl, map)?;
            }

            tbl.set("platform", platform)?;
            if tbl.get::<Option<String>>("type")?.is_none() {
                tbl.set("type", "local")?;
            }
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

    // Open a new tab and set its title once created.
    // Args: (command?, plain?, title?)
    session.set(
        "new_tab_with_title",
        lua.create_function(
            |lua, (command, plain, title): (Option<String>, Option<bool>, Option<String>)| {
                let tab_id = with_host_api(lua, |api| {
                    api.new_tab_with_title(
                        command.as_deref(),
                        plain.unwrap_or(false),
                        title.as_deref(),
                    )
                })?;
                Ok(tab_id)
            },
        )?,
    )?;

    // Convenience helper: open a new plain shell tab.
    // Args: (command?)
    session.set(
        "new_plain_tab",
        lua.create_function(|lua, command: Option<String>| {
            with_host_api(lua, |api| api.new_tab(command.as_deref(), true))?;
            Ok(())
        })?,
    )?;

    // Rename the active tab in the focused window.
    // Args: (title)
    session.set(
        "rename_tab",
        lua.create_function(|lua, title: String| {
            with_host_api(lua, |api| api.rename_active_tab(&title))?;
            Ok(())
        })?,
    )?;

    // Rename a specific tab by id.
    // Args: (tab_id, title)
    session.set(
        "rename_tab_by_id",
        lua.create_function(|lua, (tab_id, title): (String, String)| {
            with_host_api(lua, |api| api.rename_tab_by_id(&tab_id, &title))?;
            Ok(())
        })?,
    )?;

    // Focus a specific tab by id.
    // Args: (tab_id)
    session.set(
        "focus_tab_by_id",
        lua.create_function(|lua, tab_id: String| {
            with_host_api(lua, |api| api.focus_tab_by_id(&tab_id))?;
            Ok(())
        })?,
    )?;

    lua.globals().set("session", session)?;
    Ok(())
}

fn build_exec_error(lua: &Lua, msg: &str) -> LuaResult<LuaTable> {
    let result = lua.create_table()?;
    result.set("stdout", "")?;
    result.set("stderr", msg)?;
    result.set("exit_code", -1)?;
    result.set("status", "error")?;
    Ok(result)
}

fn exec_local_impl(lua: &Lua, cmd: &str) -> LuaResult<LuaTable> {
    let allowed = with_host_api(lua, |api| api.check_permission("session.exec"))?;
    if !allowed {
        return build_exec_error(lua, "permission denied: session.exec");
    }
    // Use a login shell so the user's PATH (from .zshrc / .bashrc / .profile)
    // is available.  Fall back to `sh -c` if SHELL is not set.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    match std::process::Command::new(&shell)
        .arg("-lc")
        .arg(cmd)
        .output()
    {
        Ok(output) => {
            let result = lua.create_table()?;
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
            Ok(result)
        }
        Err(e) => build_exec_error(lua, &e.to_string()),
    }
}

fn set_lua_table_from_json_map(
    tbl: &LuaTable,
    map: serde_json::Map<String, JsonValue>,
) -> LuaResult<()> {
    for (k, v) in map {
        match v {
            JsonValue::String(s) => {
                tbl.set(k, s)?;
            }
            JsonValue::Bool(b) => {
                tbl.set(k, b)?;
            }
            JsonValue::Number(n) => {
                if let Some(i) = n.as_i64() {
                    tbl.set(k, i)?;
                } else if let Some(f) = n.as_f64() {
                    tbl.set(k, f)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
