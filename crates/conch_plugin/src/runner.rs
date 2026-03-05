use std::path::Path;

use anyhow::Result;
use mlua::Lua;

use crate::api::{self, PluginContext};

/// Execute a Lua plugin script with the full API available.
pub async fn run_plugin(path: &Path, ctx: PluginContext) -> Result<()> {
    let lua = Lua::new();

    // Sandbox: remove dangerous modules
    sandbox(&lua)?;

    // Configure package paths for LuaRocks modules and local requires
    configure_package_paths(&lua, path)?;

    // Register API tables
    api::session::register(&lua, ctx.clone())?;
    api::app::register(&lua, ctx.clone())?;
    api::ui::register(&lua, ctx)?;
    api::crypto::register(&lua)?;

    let script = std::fs::read_to_string(path)?;
    lua.load(&script).exec_async().await?;

    Ok(())
}

/// Remove dangerous Lua standard library functions for sandboxing.
fn sandbox(lua: &Lua) -> Result<()> {
    let globals = lua.globals();

    // Remove os module (file ops, process exec, etc.)
    globals.set("os", mlua::Value::Nil)?;
    // Remove io module (file I/O)
    globals.set("io", mlua::Value::Nil)?;
    // Remove loadfile/dofile (arbitrary file execution)
    globals.set("loadfile", mlua::Value::Nil)?;
    globals.set("dofile", mlua::Value::Nil)?;

    Ok(())
}

/// Configure `package.path` and `package.cpath` so plugins can:
/// 1. `require()` local helper files from the plugin's own directory
/// 2. `require()` LuaRocks-installed modules from `~/.config/conch/lua_modules/`
fn configure_package_paths(lua: &Lua, plugin_path: &Path) -> Result<()> {
    let luarocks_base = conch_core::config::config_dir()
        .join("lua_modules")
        .to_string_lossy()
        .into_owned();

    // Plugin's own directory for local requires
    let plugin_dir = plugin_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_string_lossy();

    lua.load(format!(
        r#"
        local plugin_dir = "{plugin_dir}"
        local luarocks = "{luarocks_base}"

        package.path = plugin_dir .. "/?.lua;"
            .. plugin_dir .. "/?/init.lua;"
            .. luarocks .. "/share/lua/5.4/?.lua;"
            .. luarocks .. "/share/lua/5.4/?/init.lua;"
            .. package.path

        package.cpath = luarocks .. "/lib/lua/5.4/?.so;"
            .. luarocks .. "/lib/lua/5.4/?.dylib;"
            .. package.cpath
        "#
    ))
    .exec()?;

    Ok(())
}
