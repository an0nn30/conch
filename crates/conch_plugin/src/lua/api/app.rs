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

    lua.globals().set("app", app)?;
    Ok(())
}
