use mlua::{Lua, Result as LuaResult};

use super::{PluginCommand, PluginContext, PluginResponse};

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

    // app.notify(msg) — show notification
    let ctx_notify = ctx.clone();
    app.set(
        "notify",
        lua.create_function(move |_lua, msg: String| {
            ctx_notify.send_fire_and_forget(PluginCommand::Notify(msg));
            Ok(())
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

    lua.globals().set("app", app)?;
    Ok(())
}
