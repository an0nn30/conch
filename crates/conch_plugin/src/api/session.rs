use mlua::{Lua, Result as LuaResult};

use super::{PluginCommand, PluginContext, PluginResponse, SessionTarget};

/// Register the `session` table into the Lua state.
pub fn register(lua: &Lua, ctx: PluginContext) -> LuaResult<()> {
    let session = lua.create_table()?;

    // session.exec(cmd) — execute command on active session, return output
    let ctx_exec = ctx.clone();
    session.set(
        "exec",
        lua.create_async_function(move |_lua, cmd: String| {
            let ctx = ctx_exec.clone();
            async move {
                let resp = ctx
                    .send_command(PluginCommand::Exec {
                        target: SessionTarget::Current,
                        command: cmd,
                    })
                    .await;
                match resp {
                    PluginResponse::Output(s) => Ok(s),
                    PluginResponse::Error(e) => Err(mlua::Error::runtime(e)),
                    _ => Ok(String::new()),
                }
            }
        })?,
    )?;

    // session.send(text) — send raw text to active session
    let ctx_send = ctx.clone();
    session.set(
        "send",
        lua.create_async_function(move |_lua, text: String| {
            let ctx = ctx_send.clone();
            async move {
                ctx.send_fire_and_forget(PluginCommand::Send {
                    target: SessionTarget::Current,
                    text,
                });
                Ok(())
            }
        })?,
    )?;

    // session.run(cmd) — send command + newline to active session
    let ctx_run = ctx.clone();
    session.set(
        "run",
        lua.create_async_function(move |_lua, cmd: String| {
            let ctx = ctx_run.clone();
            async move {
                ctx.send_fire_and_forget(PluginCommand::Send {
                    target: SessionTarget::Current,
                    text: format!("{}\n", cmd),
                });
                Ok(())
            }
        })?,
    )?;

    // session.current() — get info about the active session
    let ctx_current = ctx.clone();
    session.set(
        "current",
        lua.create_async_function(move |lua, ()| {
            let ctx = ctx_current.clone();
            async move {
                let resp = ctx.send_command(PluginCommand::GetCurrentSession).await;
                match resp {
                    PluginResponse::SessionInfo(Some(info)) => {
                        let t = lua.create_table()?;
                        t.set("id", info.id)?;
                        t.set("title", info.title)?;
                        t.set("type", info.session_type)?;
                        Ok(mlua::Value::Table(t))
                    }
                    _ => Ok(mlua::Value::Nil),
                }
            }
        })?,
    )?;

    // session.all() — get info about all sessions
    let ctx_all = ctx.clone();
    session.set(
        "all",
        lua.create_async_function(move |lua, ()| {
            let ctx = ctx_all.clone();
            async move {
                let resp = ctx.send_command(PluginCommand::GetAllSessions).await;
                match resp {
                    PluginResponse::SessionList(list) => {
                        let result = lua.create_table()?;
                        for (i, info) in list.into_iter().enumerate() {
                            let t = lua.create_table()?;
                            t.set("id", info.id)?;
                            t.set("title", info.title)?;
                            t.set("type", info.session_type)?;
                            result.set(i + 1, t)?;
                        }
                        Ok(mlua::Value::Table(result))
                    }
                    _ => Ok(mlua::Value::Nil),
                }
            }
        })?,
    )?;

    // session.named(name) — get a handle table for a named session
    // The returned table has bound exec(cmd), send(text), run(cmd) that target
    // SessionTarget::Named(name).
    let ctx_named = ctx.clone();
    session.set(
        "named",
        lua.create_async_function(move |lua, name: String| {
            let ctx = ctx_named.clone();
            async move {
                // Verify the session exists
                let resp = ctx
                    .send_command(PluginCommand::GetNamedSession { name: name.clone() })
                    .await;
                match resp {
                    PluginResponse::SessionInfo(Some(info)) => {
                        let handle = lua.create_table()?;
                        handle.set("id", info.id)?;
                        handle.set("title", info.title)?;
                        handle.set("type", info.session_type)?;

                        // handle.exec(cmd)
                        let ctx_exec = ctx.clone();
                        let name_exec = name.clone();
                        handle.set(
                            "exec",
                            lua.create_async_function(move |_lua, cmd: String| {
                                let ctx = ctx_exec.clone();
                                let name = name_exec.clone();
                                async move {
                                    let resp = ctx
                                        .send_command(PluginCommand::Exec {
                                            target: SessionTarget::Named(name),
                                            command: cmd,
                                        })
                                        .await;
                                    match resp {
                                        PluginResponse::Output(s) => Ok(s),
                                        PluginResponse::Error(e) => {
                                            Err(mlua::Error::runtime(e))
                                        }
                                        _ => Ok(String::new()),
                                    }
                                }
                            })?,
                        )?;

                        // handle.send(text)
                        let ctx_send = ctx.clone();
                        let name_send = name.clone();
                        handle.set(
                            "send",
                            lua.create_async_function(move |_lua, text: String| {
                                let ctx = ctx_send.clone();
                                let name = name_send.clone();
                                async move {
                                    ctx.send_fire_and_forget(PluginCommand::Send {
                                        target: SessionTarget::Named(name),
                                        text,
                                    });
                                    Ok(())
                                }
                            })?,
                        )?;

                        // handle.run(cmd)
                        let ctx_run = ctx.clone();
                        let name_run = name.clone();
                        handle.set(
                            "run",
                            lua.create_async_function(move |_lua, cmd: String| {
                                let ctx = ctx_run.clone();
                                let name = name_run.clone();
                                async move {
                                    ctx.send_fire_and_forget(PluginCommand::Send {
                                        target: SessionTarget::Named(name),
                                        text: format!("{}\n", cmd),
                                    });
                                    Ok(())
                                }
                            })?,
                        )?;

                        Ok(mlua::Value::Table(handle))
                    }
                    _ => Ok(mlua::Value::Nil),
                }
            }
        })?,
    )?;

    lua.globals().set("session", session)?;
    Ok(())
}
