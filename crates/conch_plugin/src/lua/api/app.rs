//! `app.*` Lua table — logging, clipboard, events, config, notifications, menu.

use mlua::prelude::*;

use super::ui::lua_value_to_json;
use super::with_host_api;

// ---------------------------------------------------------------------------
// app.* table
// ---------------------------------------------------------------------------

pub(super) fn register_app_table(lua: &Lua) -> LuaResult<()> {
    let app = lua.create_table()?;

    app.set(
        "log",
        lua.create_function(|lua, (level, msg): (String, String)| {
            let level_num = match level.as_str() {
                "trace" => 0u8,
                "debug" => 1,
                "info" => 2,
                "warn" => 3,
                "error" => 4,
                _ => 2,
            };
            with_host_api(lua, |api| api.log(level_num, &msg))?;
            Ok(())
        })?,
    )?;

    app.set(
        "clipboard",
        lua.create_function(|lua, text: String| {
            with_host_api(lua, |api| api.clipboard_set(&text))?;
            Ok(())
        })?,
    )?;

    app.set(
        "clipboard_get",
        lua.create_function(|lua, ()| {
            let result = with_host_api(lua, |api| api.clipboard_get())?;
            Ok(result)
        })?,
    )?;

    app.set(
        "get_theme",
        lua.create_function(|lua, ()| {
            let result = with_host_api(lua, |api| api.get_theme())?;
            Ok(result)
        })?,
    )?;

    app.set(
        "publish",
        lua.create_function(|lua, (event_type, data): (String, LuaValue)| {
            let data_json = serde_json::to_string(&lua_value_to_json(data)?)
                .unwrap_or_else(|_| "{}".to_string());
            with_host_api(lua, |api| api.publish_event(&event_type, &data_json))?;
            Ok(())
        })?,
    )?;

    app.set(
        "subscribe",
        lua.create_function(|lua, event_type: String| {
            with_host_api(lua, |api| api.subscribe(&event_type))?;
            Ok(())
        })?,
    )?;

    app.set(
        "notify",
        lua.create_function(
            |lua, (title, body, level, duration_ms): (String, String, Option<String>, Option<u64>)| {
                let notif = serde_json::json!({
                    "title": title,
                    "body": body,
                    "level": level.unwrap_or_else(|| "info".into()),
                    "duration_ms": duration_ms.unwrap_or(3000),
                });
                let json = notif.to_string();
                with_host_api(lua, |api| api.notify(&json))?;
                Ok(())
            },
        )?,
    )?;

    app.set(
        "set_status",
        lua.create_function(
            |lua, (text, level, progress): (Option<String>, Option<String>, Option<f32>)| {
                let level_num = match level.as_deref().unwrap_or("info") {
                    "info" => 0u8,
                    "warn" | "warning" => 1u8,
                    "error" => 2u8,
                    "success" => 3u8,
                    _ => 0u8,
                };
                with_host_api(lua, |api| {
                    api.set_status(text.as_deref(), level_num, progress.unwrap_or(-1.0));
                })?;
                Ok(())
            },
        )?,
    )?;

    app.set(
        "register_service",
        lua.create_function(|lua, name: String| {
            with_host_api(lua, |api| api.register_service(&name))?;
            Ok(())
        })?,
    )?;

    app.set(
        "register_menu_item",
        lua.create_function(
            |lua, (menu, label, action, keybind): (String, String, String, Option<String>)| {
                with_host_api(lua, |api| {
                    api.register_menu_item(&menu, &label, &action, keybind.as_deref());
                })?;
                Ok(())
            },
        )?,
    )?;

    // Convenience alias for register_menu_item(...):
    //   register_command(menu, label, action, keybind?)
    //   register_command(label, action, keybind?)   -- defaults menu to "Tools"
    app.set(
        "register_command",
        lua.create_function(|lua, args: mlua::Variadic<LuaValue>| {
            let (menu, label, action, keybind) = match args.len() {
                2 => {
                    let label = string_arg(&args, 0)?;
                    let action = string_arg(&args, 1)?;
                    ("Tools".to_string(), label, action, None)
                }
                3 => {
                    let a0 = string_arg(&args, 0)?;
                    let a1 = string_arg(&args, 1)?;
                    let a2 = opt_string_arg(&args, 2)?;
                    ("Tools".to_string(), a0, a1, a2)
                }
                4 => {
                    let menu = string_arg(&args, 0)?;
                    let label = string_arg(&args, 1)?;
                    let action = string_arg(&args, 2)?;
                    let keybind = opt_string_arg(&args, 3)?;
                    (menu, label, action, keybind)
                }
                _ => {
                    return Err(LuaError::RuntimeError(
                        "register_command expects 2-4 args".into(),
                    ));
                }
            };

            with_host_api(lua, |api| {
                api.register_menu_item(&menu, &label, &action, keybind.as_deref());
            })?;
            Ok(())
        })?,
    )?;

    app.set(
        "register_settings_section",
        lua.create_function(|lua, section: LuaValue| {
            let section_json_value = lua_value_to_json(section)?;
            if !section_json_value.is_object() {
                return Err(LuaError::RuntimeError(
                    "register_settings_section expects a table/object".into(),
                ));
            }
            let section_json = serde_json::to_string(&section_json_value)
                .map_err(|e| LuaError::RuntimeError(format!("encode settings section: {e}")))?;
            with_host_api(lua, |api| api.register_settings_section(&section_json))?;
            Ok(())
        })?,
    )?;

    app.set(
        "query_plugin",
        lua.create_function(
            |lua, (target, method, args): (String, String, Option<LuaValue>)| {
                let args_json = match args {
                    Some(v) => serde_json::to_string(&lua_value_to_json(v)?)
                        .unwrap_or_else(|_| "null".to_string()),
                    None => "null".to_string(),
                };
                let result =
                    with_host_api(lua, |api| api.query_plugin(&target, &method, &args_json))?;
                Ok(result)
            },
        )?,
    )?;

    app.set(
        "get_config",
        lua.create_function(|lua, key: String| {
            let result = with_host_api(lua, |api| api.get_config(&key))?;
            Ok(result)
        })?,
    )?;

    app.set(
        "set_config",
        lua.create_function(|lua, (key, value): (String, String)| {
            with_host_api(lua, |api| api.set_config(&key, &value))?;
            Ok(())
        })?,
    )?;

    app.set(
        "get_setting_value",
        lua.create_function(|lua, key: String| {
            let result = with_host_api(lua, |api| api.get_setting_value(&key))?;
            Ok(result)
        })?,
    )?;

    app.set(
        "set_setting_draft",
        lua.create_function(|lua, (key, value): (String, Option<String>)| {
            with_host_api(lua, |api| api.set_setting_draft(&key, value.as_deref()))?;
            Ok(())
        })?,
    )?;

    lua.globals().set("app", app)?;
    Ok(())
}

fn string_arg(args: &[LuaValue], idx: usize) -> LuaResult<String> {
    match args.get(idx) {
        Some(LuaValue::String(s)) => Ok(s.to_str()?.to_string()),
        _ => Err(LuaError::RuntimeError(format!(
            "register_command arg {} must be a string",
            idx + 1
        ))),
    }
}

fn opt_string_arg(args: &[LuaValue], idx: usize) -> LuaResult<Option<String>> {
    match args.get(idx) {
        Some(LuaValue::Nil) | None => Ok(None),
        Some(LuaValue::String(s)) => Ok(Some(s.to_str()?.to_string())),
        _ => Err(LuaError::RuntimeError(format!(
            "register_command arg {} must be a string or nil",
            idx + 1
        ))),
    }
}
