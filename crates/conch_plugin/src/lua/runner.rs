//! Lua plugin runner — lifecycle management for Lua plugins.
//!
//! Each Lua plugin runs on its own OS thread (same model as native plugins).
//! The runner creates a Lua VM, registers the API tables, loads the plugin
//! source, and enters a mailbox loop dispatching events and render requests.

use std::path::{Path, PathBuf};

use conch_plugin_sdk::widgets::{PluginEvent, Widget};
use conch_plugin_sdk::HostApi;
use mlua::prelude::*;
use tokio::sync::mpsc;

use crate::bus::PluginMail;
use crate::lua::api;
use crate::lua::metadata::{self, LuaPluginMeta};

/// A discovered Lua plugin (not yet running).
#[derive(Debug, Clone)]
pub struct DiscoveredLuaPlugin {
    pub path: PathBuf,
    pub source: String,
    pub meta: LuaPluginMeta,
}

/// A running Lua plugin.
pub struct RunningLuaPlugin {
    pub meta: LuaPluginMeta,
    pub sender: mpsc::Sender<PluginMail>,
    pub thread: Option<std::thread::JoinHandle<()>>,
}

/// Discover Lua plugins in a directory.
pub fn discover(dir: &Path) -> Vec<DiscoveredLuaPlugin> {
    metadata::discover_lua_plugins(dir)
        .into_iter()
        .map(|(path, source)| {
            let meta = metadata::parse_lua_metadata(&source);
            DiscoveredLuaPlugin { path, source, meta }
        })
        .collect()
}

/// Spawn a Lua plugin on a dedicated OS thread.
///
/// Returns the running plugin handle. The plugin's setup() function is
/// called on the thread. The mailbox is used for event/render/shutdown
/// communication.
pub fn spawn_lua_plugin(
    plugin: &DiscoveredLuaPlugin,
    host_api: *const HostApi,
    mailbox_tx: mpsc::Sender<PluginMail>,
    mailbox_rx: mpsc::Receiver<PluginMail>,
) -> Result<RunningLuaPlugin, String> {
    let meta = plugin.meta.clone();
    let source = plugin.source.clone();
    let path = plugin.path.clone();
    let plugin_name = meta.name.clone();

    // Cast the host_api pointer to usize for Send across thread boundary.
    let host_api_addr = host_api as usize;

    let thread = std::thread::Builder::new()
        .name(format!("lua-plugin:{}", plugin_name))
        .spawn(move || {
            let api = host_api_addr as *const HostApi;
            lua_plugin_thread(api, &source, &path, mailbox_rx);
        })
        .map_err(|e| format!("Failed to spawn Lua plugin thread: {e}"))?;

    Ok(RunningLuaPlugin {
        meta,
        sender: mailbox_tx,
        thread: Some(thread),
    })
}

/// The main function running on a Lua plugin's dedicated thread.
fn lua_plugin_thread(
    host_api: *const HostApi,
    source: &str,
    path: &Path,
    mut mailbox: mpsc::Receiver<PluginMail>,
) {
    // Create Lua VM.
    let lua = match Lua::new() {
        lua => lua,
    };

    // Register API tables.
    if let Err(e) = api::register_all(&lua, host_api) {
        log::error!("Failed to register Lua API: {e}");
        return;
    }

    // Load and execute the plugin source.
    let chunk_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if let Err(e) = lua.load(source).set_name(&chunk_name).exec() {
        log::error!("Failed to load Lua plugin {chunk_name}: {e}");
        return;
    }

    // Call setup() if it exists.
    if let Ok(setup_fn) = lua.globals().get::<LuaFunction>("setup") {
        if let Err(e) = setup_fn.call::<()>(()) {
            log::error!("Lua plugin setup() failed: {e}");
            return;
        }
    }

    log::info!("Lua plugin '{}' started", chunk_name);

    // Enter mailbox loop.
    loop {
        let mail = match mailbox.blocking_recv() {
            Some(m) => m,
            None => break, // Channel closed.
        };

        match mail {
            PluginMail::BusEvent(msg) => {
                handle_bus_event(&lua, &msg.event_type, &msg.data);
            }

            PluginMail::BusQuery(req) => {
                let result = handle_query(&lua, &req.method, &req.args);
                let _ = req.reply.send(crate::bus::QueryResponse {
                    result: Ok(result.unwrap_or(serde_json::Value::Null)),
                });
            }

            PluginMail::RenderRequest { reply } => {
                let widgets = handle_render(&lua);
                let json = serde_json::to_string(&widgets).unwrap_or_else(|_| "[]".into());
                let _ = reply.send(json);
            }

            PluginMail::WidgetEvent { json } => {
                // Parse and dispatch as a PluginEvent to on_event().
                if let Ok(event) = serde_json::from_str::<conch_plugin_sdk::PluginEvent>(&json) {
                    dispatch_event(&lua, &event);
                }
            }

            PluginMail::Shutdown => {
                // Call teardown() if it exists.
                if let Ok(teardown_fn) = lua.globals().get::<LuaFunction>("teardown") {
                    if let Err(e) = teardown_fn.call::<()>(()) {
                        log::warn!("Lua plugin teardown() error: {e}");
                    }
                }
                log::info!("Lua plugin '{}' shutting down", chunk_name);
                break;
            }
        }
    }
}

/// Call the Lua `on_event()` function with a bus event.
fn handle_bus_event(lua: &Lua, event_type: &str, data: &serde_json::Value) {
    let event = PluginEvent::BusEvent {
        event_type: event_type.to_string(),
        data: data.clone(),
    };
    dispatch_event(lua, &event);
}

/// Dispatch a PluginEvent to the Lua `on_event()` function.
fn dispatch_event(lua: &Lua, event: &PluginEvent) {
    let Ok(on_event) = lua.globals().get::<LuaFunction>("on_event") else {
        return;
    };

    let json = match serde_json::to_string(event) {
        Ok(j) => j,
        Err(_) => return,
    };

    // Parse the JSON into a Lua table so the plugin gets a native table.
    let Ok(tbl) = lua.load(&format!("return {}", json_to_lua_literal(&json))).eval::<LuaTable>()
    else {
        // Fallback: pass as string.
        let _ = on_event.call::<()>(json);
        return;
    };

    let _ = on_event.call::<()>(tbl);
}

/// Handle a render request by calling the Lua `render()` function.
fn handle_render(lua: &Lua) -> Vec<Widget> {
    // Clear the accumulator before calling render.
    api::with_acc_pub(lua, |acc| acc.clear());

    if let Ok(render_fn) = lua.globals().get::<LuaFunction>("render") {
        if let Err(e) = render_fn.call::<()>(()) {
            log::error!("Lua render() error: {e}");
            return vec![];
        }
    }

    api::take_widgets(lua)
}

/// Handle a direct query by calling `on_query()` if it exists.
fn handle_query(lua: &Lua, method: &str, args: &serde_json::Value) -> Option<serde_json::Value> {
    let on_query = lua.globals().get::<LuaFunction>("on_query").ok()?;
    let args_str = serde_json::to_string(args).unwrap_or_else(|_| "null".into());
    let result: String = on_query
        .call((method.to_string(), args_str))
        .unwrap_or_else(|_| "null".into());
    serde_json::from_str(&result).ok()
}

/// Convert a JSON string to a Lua table literal.
///
/// This is a simple approach for passing structured data to Lua.
/// For production, you'd use mlua's serde integration.
fn json_to_lua_literal(json: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return format!("{{}}"),
    };
    json_value_to_lua_literal(&value)
}

fn json_value_to_lua_literal(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "nil".into(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => {
            // Escape special characters for Lua string literal.
            let escaped = s
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\0', "");
            format!("\"{escaped}\"")
        }
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(json_value_to_lua_literal).collect();
            format!("{{{}}}", items.join(", "))
        }
        serde_json::Value::Object(map) => {
            let items: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let key = if k.chars().all(|c| c.is_alphanumeric() || c == '_')
                        && !k.is_empty()
                        && !k.starts_with(|c: char| c.is_ascii_digit())
                    {
                        k.clone()
                    } else {
                        format!("[\"{}\"]", k.replace('"', "\\\""))
                    };
                    format!("{} = {}", key, json_value_to_lua_literal(v))
                })
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_to_lua_literal_object() {
        let lua_str = json_to_lua_literal(r#"{"type":"button_click","id":"btn1"}"#);
        // Should produce a valid Lua table literal.
        assert!(lua_str.contains("type"));
        assert!(lua_str.contains("button_click"));
        assert!(lua_str.contains("id"));
    }

    #[test]
    fn json_to_lua_literal_array() {
        let lua_str = json_to_lua_literal(r#"[1, 2, 3]"#);
        assert!(lua_str.contains("1"));
        assert!(lua_str.contains("3"));
    }

    #[test]
    fn json_to_lua_literal_nested() {
        let lua_str = json_to_lua_literal(r#"{"data":{"nested":true},"count":5}"#);
        assert!(lua_str.contains("data"));
        assert!(lua_str.contains("nested"));
    }

    #[test]
    fn json_to_lua_literal_string_escaping() {
        let lua_str = json_to_lua_literal(r#"{"msg":"hello \"world\""}"#);
        assert!(lua_str.contains("hello"));
    }

    #[test]
    fn json_to_lua_literal_null() {
        let lua_str = json_to_lua_literal(r#"{"x":null}"#);
        assert!(lua_str.contains("nil"));
    }

    #[test]
    fn discover_returns_empty_for_nonexistent() {
        let plugins = discover(Path::new("/nonexistent"));
        assert!(plugins.is_empty());
    }
}
