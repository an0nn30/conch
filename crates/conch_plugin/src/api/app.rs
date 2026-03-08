use mlua::{Lua, Result as LuaResult};

use super::{NotificationLevel, NotificationRequest, PluginCommand, PluginContext, PluginResponse};

/// Register the `app` table into the Lua state.
pub fn register(lua: &Lua, ctx: PluginContext) -> LuaResult<()> {
    let app = lua.create_table()?;

    // app.open_session(name) — open a session by server name or host
    let ctx_open = ctx.clone();
    app.set(
        "open_session",
        lua.create_function(move |_lua, name: String| {
            ctx_open.send_fire_and_forget(PluginCommand::OpenSession { name });
            Ok(())
        })?,
    )?;

    // app.clipboard(text) — copy text to clipboard
    let ctx_clip = ctx.clone();
    app.set(
        "clipboard",
        lua.create_function(move |_lua, text: String| {
            ctx_clip.send_fire_and_forget(PluginCommand::Clipboard(text));
            Ok(())
        })?,
    )?;

    // app.notify(msg_or_table) — show notification
    // Simple: app.notify("hello")
    // Rich:   app.notify({ title="Done", body="Scan complete", level="success", duration=5, buttons={"Yes","No"} })
    // When buttons are provided, blocks and returns the clicked button label.
    let ctx_notify = ctx.clone();
    app.set(
        "notify",
        lua.create_async_function(move |_lua, arg: mlua::Value| {
            let ctx = ctx_notify.clone();
            async move {
                let (request, has_buttons) = match arg {
                    mlua::Value::String(s) => {
                        let msg = s.to_string_lossy().to_string();
                        (NotificationRequest {
                            title: None,
                            body: msg,
                            level: NotificationLevel::Info,
                            duration_secs: None,
                            buttons: Vec::new(),
                        }, false)
                    }
                    mlua::Value::Table(tbl) => {
                        let title: Option<String> = tbl.get("title").ok();
                        let body: String = tbl.get("body").unwrap_or_default();
                        let level_str: String = tbl.get("level").unwrap_or_default();
                        let level = match level_str.as_str() {
                            "success" => NotificationLevel::Success,
                            "warning" | "warn" => NotificationLevel::Warning,
                            "error" | "err" => NotificationLevel::Error,
                            _ => NotificationLevel::Info,
                        };
                        let duration_secs: Option<f32> = tbl.get("duration").ok();
                        let buttons: Vec<String> = if let Ok(btns) = tbl.get::<mlua::Table>("buttons") {
                            btns.sequence_values::<String>().filter_map(|r| r.ok()).collect()
                        } else {
                            Vec::new()
                        };
                        let has_buttons = !buttons.is_empty();
                        (NotificationRequest {
                            title,
                            body,
                            level,
                            duration_secs,
                            buttons,
                        }, has_buttons)
                    }
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "app.notify() expects a string or table argument".into(),
                        ));
                    }
                };

                if has_buttons {
                    // Blocking: wait for the user to click a button
                    let resp = ctx.send_command(PluginCommand::Notify(request)).await;
                    match resp {
                        PluginResponse::Output(label) => Ok(mlua::Value::String(
                            _lua.create_string(&label)?,
                        )),
                        _ => Ok(mlua::Value::Nil),
                    }
                } else {
                    // Fire-and-forget
                    ctx.send_fire_and_forget(PluginCommand::Notify(request));
                    Ok(mlua::Value::Nil)
                }
            }
        })?,
    )?;

    // app.log(msg) — log a message
    let ctx_log = ctx.clone();
    app.set(
        "log",
        lua.create_function(move |_lua, msg: String| {
            ctx_log.send_fire_and_forget(PluginCommand::Log(msg));
            Ok(())
        })?,
    )?;

    // app.servers() — get list of configured server names
    let ctx_servers = ctx.clone();
    app.set(
        "servers",
        lua.create_async_function(move |lua, ()| {
            let ctx = ctx_servers.clone();
            async move {
                let resp = ctx.send_command(PluginCommand::GetServers).await;
                match resp {
                    PluginResponse::ServerList(names) => {
                        let result = lua.create_table()?;
                        for (i, name) in names.into_iter().enumerate() {
                            result.set(i + 1, name)?;
                        }
                        Ok(mlua::Value::Table(result))
                    }
                    _ => Ok(mlua::Value::Nil),
                }
            }
        })?,
    )?;

    // app.server_details() — get list of configured servers with name and host
    let ctx_details = ctx.clone();
    app.set(
        "server_details",
        lua.create_async_function(move |lua, ()| {
            let ctx = ctx_details.clone();
            async move {
                let resp = ctx.send_command(PluginCommand::GetServerDetails).await;
                match resp {
                    PluginResponse::ServerDetailList(pairs) => {
                        let result = lua.create_table()?;
                        for (i, (name, host)) in pairs.into_iter().enumerate() {
                            let entry = lua.create_table()?;
                            entry.set("name", name)?;
                            entry.set("host", host)?;
                            result.set(i + 1, entry)?;
                        }
                        Ok(mlua::Value::Table(result))
                    }
                    _ => Ok(mlua::Value::Nil),
                }
            }
        })?,
    )?;

    // app.set_icon(path) — set the plugin's icon from a file path
    let ctx_icon = ctx.clone();
    app.set(
        "set_icon",
        lua.create_async_function(move |_lua, path: String| {
            let ctx = ctx_icon.clone();
            async move {
                let resp = ctx
                    .send_command(PluginCommand::SetIcon { path })
                    .await;
                match resp {
                    PluginResponse::Ok => Ok(true),
                    PluginResponse::Error(_) => Ok(false),
                    _ => Ok(false),
                }
            }
        })?,
    )?;

    // app.register_keybind(action, binding, description) — register a keybinding at runtime
    let ctx_kb = ctx.clone();
    app.set(
        "register_keybind",
        lua.create_async_function(move |_lua, (action, binding, description): (String, String, Option<String>)| {
            let ctx = ctx_kb.clone();
            async move {
                let resp = ctx
                    .send_command(PluginCommand::RegisterKeybind {
                        action,
                        binding,
                        description: description.unwrap_or_default(),
                    })
                    .await;
                match resp {
                    PluginResponse::Ok => Ok(true),
                    PluginResponse::Error(_) => Ok(false),
                    _ => Ok(false),
                }
            }
        })?,
    )?;

    lua.globals().set("app", app)?;
    Ok(())
}
