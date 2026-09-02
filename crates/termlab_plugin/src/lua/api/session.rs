//! `session.*` Lua table — platform info, command execution, PTY write, new tab.

use mlua::prelude::*;
use serde_json::Value as JsonValue;

use crate::lua::convert::merge_json_into_table;

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
                    if let Ok(value @ JsonValue::Object(_)) =
                        serde_json::from_str::<JsonValue>(&json)
                    {
                        merge_json_into_table(lua, &tbl, &value)?;
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
                && let Ok(value @ JsonValue::Object(_)) = serde_json::from_str::<JsonValue>(&json)
            {
                merge_json_into_table(lua, &tbl, &value)?;
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

#[cfg(test)]
mod tests {
    use super::register_session_table;
    use crate::lua::api::HostApiBridge;
    use crate::lua::convert::merge_json_into_table;
    use mlua::prelude::*;
    use std::sync::Arc;
    use termlab_plugin_sdk::PanelLocation;

    #[test]
    fn merge_preserves_caller_defaults_and_adds_nested_values() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        tbl.set("platform", "macos").unwrap();
        // A key the caller pre-set that also appears in the host response —
        // the actual risk the "defaults survive" claim needs to cover.
        tbl.set("port", 9999).unwrap();

        merge_json_into_table(
            &lua,
            &tbl,
            &serde_json::json!({
                "type": "ssh",
                "host": "example.com",
                "port": 22,
                "forwards": [{"local_port": 8080}]
            }),
        )
        .unwrap();

        // Untouched pre-set key survives.
        assert_eq!(tbl.get::<String>("platform").unwrap(), "macos");
        assert_eq!(tbl.get::<String>("type").unwrap(), "ssh");
        // Colliding key: the incoming value wins over the pre-set default.
        assert_eq!(tbl.get::<i64>("port").unwrap(), 22);

        // Nested values used to be dropped entirely.
        let forwards: LuaTable = tbl.get("forwards").unwrap();
        let first: LuaTable = forwards.get(1).unwrap();
        assert_eq!(first.get::<i64>("local_port").unwrap(), 8080);
    }

    /// Minimal `HostApi` stub for driving `session.*` Lua functions in
    /// tests. Every method besides `get_active_session` returns an inert
    /// default; see `crate::host_api::tests::MockHostApi` for the same
    /// pattern used at the trait level.
    struct MockHostApi {
        active_session_json: Option<String>,
    }

    impl crate::HostApi for MockHostApi {
        fn plugin_name(&self) -> &str {
            "mock"
        }
        fn register_panel(&self, _: PanelLocation, _: &str, _: Option<&str>) -> u64 {
            1
        }
        fn set_widgets(&self, _: u64, _: &str) {}
        fn open_docked_view(&self, _: &str) -> Option<String> {
            None
        }
        fn close_docked_view(&self, _: &str) -> bool {
            false
        }
        fn focus_docked_view(&self, _: &str) -> bool {
            false
        }
        fn log(&self, _: u8, _: &str) {}
        fn notify(&self, _: &str) {}
        fn set_status(&self, _: Option<&str>, _: u8, _: f32) {}
        fn publish_event(&self, _: &str, _: &str) {}
        fn subscribe(&self, _: &str) {}
        fn query_plugin(&self, _: &str, _: &str, _: &str) -> Option<String> {
            None
        }
        fn register_service(&self, _: &str) {}
        fn get_config(&self, _: &str) -> Option<String> {
            None
        }
        fn set_config(&self, _: &str, _: &str) {}
        fn clipboard_set(&self, _: &str) {}
        fn clipboard_get(&self) -> Option<String> {
            None
        }
        fn get_theme(&self) -> Option<String> {
            None
        }
        fn get_active_session(&self) -> Option<String> {
            self.active_session_json.clone()
        }
        fn register_menu_item(&self, _: &str, _: &str, _: &str, _: Option<&str>) {}
        fn show_form(&self, _: &str) -> Option<String> {
            None
        }
        fn show_confirm(&self, _: &str) -> bool {
            false
        }
        fn show_prompt(&self, _: &str, _: &str) -> Option<String> {
            None
        }
        fn show_alert(&self, _: &str, _: &str) {}
        fn show_error(&self, _: &str, _: &str) {}
        fn show_context_menu(&self, _: &str) -> Option<String> {
            None
        }
        fn write_to_pty(&self, _: &[u8]) {}
        fn new_tab(&self, _: Option<&str>, _: bool) {}
        fn rename_active_tab(&self, _: &str) {}
        fn rename_tab_by_id(&self, _: &str, _: &str) {}
        fn focus_tab_by_id(&self, _: &str) {}
        fn open_session(&self, _: &str) -> u64 {
            0
        }
        fn close_session(&self, _: u64) {}
        fn set_session_status(&self, _: u64, _: u8, _: Option<&str>) {}
        fn session_prompt(&self, _: u64, _: u8, _: &str, _: Option<&str>) -> Option<String> {
            None
        }
    }

    #[test]
    fn current_merges_nested_host_fields_through_the_lua_call_site() {
        // Drives `session.current()` end-to-end (JSON-string parsing, the
        // `Ok(value @ JsonValue::Object(_))` match, and the merge) through a
        // mock `HostApi`, rather than calling `merge_json_into_table` directly.
        let lua = Lua::new();
        let host_api: Arc<dyn crate::HostApi> = Arc::new(MockHostApi {
            active_session_json: Some(
                serde_json::json!({
                    "host": "example.com",
                    "forwards": [{"local_port": 8080}]
                })
                .to_string(),
            ),
        });
        lua.set_app_data(HostApiBridge::new(host_api));
        register_session_table(&lua).unwrap();

        let tbl: LuaTable = lua.load("return session.current()").eval().unwrap();

        // Nested value from the host response reaches the plugin — this is
        // what the old scalar-only helper silently dropped.
        assert_eq!(tbl.get::<String>("host").unwrap(), "example.com");
        let forwards: LuaTable = tbl.get("forwards").unwrap();
        let first: LuaTable = forwards.get(1).unwrap();
        assert_eq!(first.get::<i64>("local_port").unwrap(), 8080);

        // The post-merge `type` default-fill still applies when the host
        // response doesn't include it.
        assert_eq!(tbl.get::<String>("type").unwrap(), "local");
    }
}
