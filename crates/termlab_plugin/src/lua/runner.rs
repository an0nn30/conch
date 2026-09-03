//! Lua plugin runner — lifecycle management for Lua plugins.
//!
//! Each Lua plugin runs on its own OS thread (same model as native plugins).
//! The runner creates a Lua VM, registers the API tables, loads the plugin
//! source, and enters a mailbox loop dispatching events and render requests.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mlua::prelude::*;
use mlua::{HookTriggers, StdLib};
use termlab_plugin_sdk::widgets::{PluginEvent, Widget};
use tokio::sync::mpsc;

use crate::HostApi;
use crate::bus::PluginMail;
use crate::lua::api;
use crate::lua::convert;
use crate::lua::metadata::{self, LuaPluginMeta};

/// Maximum Lua instructions per callback invocation (render, on_event, on_query).
/// Roughly corresponds to a few seconds of CPU time. Prevents infinite loops.
const LUA_INSTRUCTION_LIMIT: u32 = 1_000_000;

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
    host_api: Arc<dyn HostApi>,
    mailbox_tx: mpsc::Sender<PluginMail>,
    mailbox_rx: mpsc::Receiver<PluginMail>,
) -> Result<RunningLuaPlugin, String> {
    let meta = plugin.meta.clone();
    let source = plugin.source.clone();
    let path = plugin.path.clone();
    let plugin_name = meta.name.clone();

    let thread_meta = meta.clone();
    let thread = std::thread::Builder::new()
        .name(format!("lua-plugin:{}", plugin_name))
        .spawn(move || {
            lua_plugin_thread(host_api, &source, &path, &thread_meta, mailbox_rx);
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
    host_api: Arc<dyn HostApi>,
    source: &str,
    path: &Path,
    meta: &LuaPluginMeta,
    mut mailbox: mpsc::Receiver<PluginMail>,
) {
    let lua = match Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE,
        LuaOptions::default(),
    ) {
        Ok(vm) => vm,
        Err(e) => {
            log::error!("Failed to create sandboxed Lua VM: {e}");
            return;
        }
    };

    // Remove dangerous base globals that survive stdlib restriction.
    // loadfile/dofile can read arbitrary files; require can load C modules.
    for name in &["loadfile", "dofile", "require"] {
        if let Err(e) = lua.globals().set(*name, mlua::Value::Nil) {
            log::warn!("Failed to remove global '{name}': {e}");
        }
    }

    // Register API tables with the safe trait-based HostApi.
    if let Err(e) = api::register_all(&lua, Arc::clone(&host_api)) {
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

    // If this is a tool-window plugin, register its panel with the host.
    if matches!(meta.plugin_type, termlab_plugin_sdk::PluginType::ToolWindow) {
        let handle = host_api.register_panel(meta.panel_location, &meta.name, None);
        if let Some(store) = lua.app_data_ref::<std::cell::RefCell<api::PanelHandleStore>>() {
            store.borrow_mut().handle = Some(handle);
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

            PluginMail::RenderRequest { view_id, reply } => {
                let widgets = handle_render(&lua, view_id.as_deref());
                let json = serde_json::to_string(&widgets).unwrap_or_else(|_| "[]".into());
                let _ = reply.send(json);
            }

            PluginMail::WidgetEvent { json } => {
                log::debug!("[lua:{chunk_name}] dispatching event: {json}");
                dispatch_event_json_raw(&lua, &json);
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

/// Run a closure with an instruction-count hook that aborts runaway Lua code.
///
/// Sets a Lua hook that fires every [`LUA_INSTRUCTION_LIMIT`] instructions and
/// returns an error, causing the Lua call to fail with a runtime error. The
/// hook is removed after the closure completes (whether it succeeded or not).
fn with_instruction_limit<F, T>(lua: &Lua, f: F) -> T
where
    F: FnOnce() -> T,
{
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(LUA_INSTRUCTION_LIMIT),
        |_lua, _debug| {
            Err(LuaError::RuntimeError(
                "execution limit exceeded".to_string(),
            ))
        },
    );
    let result = f();
    lua.remove_hook();
    result
}

/// Call the Lua `on_event()` function with a bus event.
fn handle_bus_event(lua: &Lua, event_type: &str, data: &serde_json::Value) {
    let event = PluginEvent::BusEvent {
        event_type: event_type.to_string(),
        data: data.clone(),
    };
    dispatch_event(lua, &event);
    push_tool_window_render(lua);
}

/// Dispatch a PluginEvent to the Lua `on_event()` function.
fn dispatch_event(lua: &Lua, event: &PluginEvent) {
    let Ok(on_event) = lua.globals().get::<LuaFunction>("on_event") else {
        log::debug!("dispatch_event: no on_event function");
        return;
    };

    let value = match serde_json::to_value(event) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("dispatch_event: failed to encode event: {e}");
            return;
        }
    };

    let payload = match convert::json_to_lua(lua, &value) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("dispatch_event: failed to convert event payload: {e}");
            return;
        }
    };

    with_instruction_limit(lua, || {
        if let Err(e) = on_event.call::<()>(payload) {
            log::warn!("dispatch_event: on_event error: {e}");
        }
    });
}

fn dispatch_event_json_raw(lua: &Lua, json: &str) {
    let Ok(on_event) = lua.globals().get::<LuaFunction>("on_event") else {
        log::debug!("dispatch_event_json_raw: no on_event function");
        return;
    };

    let payload = match convert::json_str_to_lua(lua, json) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("dispatch_event_json_raw: failed to convert event payload: {e}");
            return;
        }
    };

    with_instruction_limit(lua, || {
        if let Err(e) = on_event.call::<()>(payload) {
            log::warn!("dispatch_event_json_raw: on_event error: {e}");
        }
    });
}

/// Handle a render request by calling the Lua `render()` function.
fn handle_render(lua: &Lua, view_id: Option<&str>) -> Vec<Widget> {
    // Clear the accumulator before calling render.
    if let Err(e) = api::with_acc_pub(lua, |acc| acc.clear()) {
        log::error!("Lua render: failed to clear accumulator: {e}");
        return vec![];
    }

    if let Some(view_id) = view_id {
        if let Ok(render_view_fn) = lua.globals().get::<LuaFunction>("render_view") {
            let result =
                with_instruction_limit(lua, || render_view_fn.call::<()>(view_id.to_string()));
            if let Err(e) = result {
                log::error!("Lua render_view({view_id}) error: {e}");
                return vec![];
            }
            return api::take_widgets(lua).unwrap_or_default();
        }
    }

    if let Ok(render_fn) = lua.globals().get::<LuaFunction>("render") {
        let result = with_instruction_limit(lua, || render_fn.call::<()>(()));
        if let Err(e) = result {
            log::error!("Lua render() error: {e}");
            return vec![];
        }
    }

    api::take_widgets(lua).unwrap_or_default()
}

fn push_tool_window_render(lua: &Lua) {
    let handle = lua
        .app_data_ref::<std::cell::RefCell<api::PanelHandleStore>>()
        .and_then(|store| store.borrow().handle);
    let Some(handle) = handle else {
        return;
    };

    let widgets = handle_render(lua, None);
    let json = serde_json::to_string(&widgets).unwrap_or_else(|_| "[]".into());
    let _ = api::with_host_api(lua, |host| host.set_widgets(handle, &json));
}

/// Handle a direct query by calling `on_query()` if it exists.
fn handle_query(lua: &Lua, method: &str, args: &serde_json::Value) -> Option<serde_json::Value> {
    let on_query = lua.globals().get::<LuaFunction>("on_query").ok()?;
    let args_str = serde_json::to_string(args).unwrap_or_else(|_| "null".into());
    let result: String = with_instruction_limit(lua, || {
        on_query
            .call((method.to_string(), args_str.clone()))
            .unwrap_or_else(|e| {
                log::warn!("handle_query: on_query error: {e}");
                "null".into()
            })
    });
    serde_json::from_str(&result).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_action_event_reaches_on_event() {
        let lua = sandboxed_lua();
        lua.load(
            r#"
            seen_kind = nil
            seen_action = nil
            function on_event(e)
                seen_kind = e.kind
                seen_action = e.action
            end
        "#,
        )
        .exec()
        .unwrap();

        let event = PluginEvent::MenuAction {
            action: "trigger_notification".into(),
        };
        dispatch_event(&lua, &event);

        assert_eq!(
            lua.globals().get::<String>("seen_kind").unwrap(),
            "menu_action"
        );
        assert_eq!(
            lua.globals().get::<String>("seen_action").unwrap(),
            "trigger_notification"
        );
    }

    #[test]
    fn bus_event_key_with_backslash_is_delivered_intact() {
        let lua = sandboxed_lua();
        lua.load(
            r#"
            received_type = nil
            received_key = nil
            function on_event(e)
                received_type = type(e.data)
                if type(e.data) == 'table' then
                    for k, _ in pairs(e.data) do received_key = k end
                end
            end
        "#,
        )
        .exec()
        .unwrap();

        // A key ending in a backslash breaks the generated Lua literal: the
        // escape consumes the closing quote of the bracketed key.
        let event = PluginEvent::BusEvent {
            event_type: "test".into(),
            data: serde_json::json!({ "danger\\": 1 }),
        };
        dispatch_event(&lua, &event);

        assert_eq!(
            lua.globals().get::<String>("received_type").unwrap(),
            "table",
            "event payload must arrive as a table, not fall back to a raw string"
        );
        assert_eq!(
            lua.globals().get::<String>("received_key").unwrap(),
            "danger\\",
            "key must arrive byte-identical"
        );
    }

    #[test]
    fn bus_event_key_with_quotes_and_newlines_is_delivered_intact() {
        let lua = sandboxed_lua();
        lua.load(
            r#"
            keys = {}
            function on_event(e)
                for k, _ in pairs(e.data) do keys[#keys+1] = k end
                table.sort(keys)
            end
        "#,
        )
        .exec()
        .unwrap();

        let event = PluginEvent::BusEvent {
            event_type: "test".into(),
            data: serde_json::json!({ "a\"b": 1, "c\nd": 2 }),
        };
        dispatch_event(&lua, &event);

        let keys: Vec<String> = lua
            .globals()
            .get::<LuaTable>("keys")
            .unwrap()
            .sequence_values()
            .collect::<LuaResult<_>>()
            .unwrap();
        assert_eq!(keys, vec!["a\"b".to_string(), "c\nd".to_string()]);
    }

    #[test]
    fn bus_event_null_value_arrives_as_nil() {
        let lua = sandboxed_lua();
        lua.load(
            r#"
            was_nil = nil
            function on_event(e) was_nil = (e.data.maybe == nil) end
        "#,
        )
        .exec()
        .unwrap();

        let event = PluginEvent::BusEvent {
            event_type: "test".into(),
            data: serde_json::json!({ "maybe": null }),
        };
        dispatch_event(&lua, &event);

        assert!(lua.globals().get::<bool>("was_nil").unwrap());
    }

    #[test]
    fn dispatch_event_json_raw_drops_malformed_json_without_calling_on_event() {
        // The widget/Java-plugin event path (`PluginMail::WidgetEvent`) forwards
        // `event_json` straight from frontend JavaScript with no validation, so
        // malformed JSON is reachable here. A conversion failure must log and
        // drop the event rather than deliver a raw JSON string to `on_event`.
        let lua = sandboxed_lua();
        lua.load(
            r#"
            on_event_called = false
            function on_event(e) on_event_called = true end
        "#,
        )
        .exec()
        .unwrap();

        dispatch_event_json_raw(&lua, "{not json");

        assert!(
            !lua.globals().get::<bool>("on_event_called").unwrap(),
            "on_event must not be invoked when the event payload fails to parse"
        );
    }

    #[test]
    fn discover_returns_empty_for_nonexistent() {
        let plugins = discover(Path::new("/nonexistent"));
        assert!(plugins.is_empty());
    }

    /// Helper: create a sandboxed Lua VM identical to what plugins get.
    fn sandboxed_lua() -> Lua {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE,
            LuaOptions::default(),
        )
        .expect("Failed to create sandboxed Lua VM");
        for name in &["loadfile", "dofile", "require"] {
            lua.globals().set(*name, mlua::Value::Nil).unwrap();
        }
        lua
    }

    #[test]
    fn sandbox_os_not_available() {
        let lua = sandboxed_lua();
        let result: LuaValue = lua.load("return os").eval().unwrap();
        assert_eq!(result, LuaValue::Nil, "os global must be nil in sandbox");
    }

    #[test]
    fn sandbox_io_not_available() {
        let lua = sandboxed_lua();
        let result: LuaValue = lua.load("return io").eval().unwrap();
        assert_eq!(result, LuaValue::Nil, "io global must be nil in sandbox");
    }

    #[test]
    fn sandbox_debug_not_available() {
        let lua = sandboxed_lua();
        let result: LuaValue = lua.load("return debug").eval().unwrap();
        assert_eq!(result, LuaValue::Nil, "debug global must be nil in sandbox");
    }

    #[test]
    fn sandbox_loadfile_not_available() {
        let lua = sandboxed_lua();
        let result: LuaValue = lua.load("return loadfile").eval().unwrap();
        assert_eq!(
            result,
            LuaValue::Nil,
            "loadfile global must be nil in sandbox"
        );
    }

    #[test]
    fn sandbox_require_not_available() {
        let lua = sandboxed_lua();
        let result: LuaValue = lua.load("return require").eval().unwrap();
        assert_eq!(
            result,
            LuaValue::Nil,
            "require global must be nil in sandbox"
        );
    }

    #[test]
    fn sandbox_string_available() {
        let lua = sandboxed_lua();
        let result: String = lua.load(r#"return string.upper("hello")"#).eval().unwrap();
        assert_eq!(result, "HELLO", "string library must be available");
    }

    #[test]
    fn sandbox_table_available() {
        let lua = sandboxed_lua();
        let result: i64 = lua
            .load("local t = {3,1,2}; table.sort(t); return t[1]")
            .eval()
            .unwrap();
        assert_eq!(result, 1, "table library must be available");
    }

    #[test]
    fn sandbox_math_available() {
        let lua = sandboxed_lua();
        let result: f64 = lua.load("return math.abs(-42)").eval().unwrap();
        assert!(
            (result - 42.0).abs() < f64::EPSILON,
            "math library must be available"
        );
    }

    #[test]
    fn sandbox_utf8_available() {
        let lua = sandboxed_lua();
        let result: i64 = lua.load(r#"return utf8.len("hello")"#).eval().unwrap();
        assert_eq!(result, 5, "utf8 library must be available");
    }

    #[test]
    fn sandbox_coroutine_available() {
        let lua = sandboxed_lua();
        let result: String = lua.load("return type(coroutine.create)").eval().unwrap();
        assert_eq!(result, "function", "coroutine library must be available");
    }

    #[test]
    fn instruction_limit_aborts_infinite_loop() {
        let lua = sandboxed_lua();
        // Define an on_event that loops forever.
        lua.load(r#"function on_event(e) while true do end end"#)
            .exec()
            .unwrap();

        let event = PluginEvent::BusEvent {
            event_type: "test".into(),
            data: serde_json::Value::Null,
        };

        // dispatch_event uses with_instruction_limit internally, so the
        // infinite loop should be terminated and the call should return
        // without hanging.
        dispatch_event(&lua, &event);
        // If we reach here, the instruction limit worked.
    }

    /// Helper: create a sandboxed Lua VM with a WidgetAccumulator registered.
    fn sandboxed_lua_with_acc() -> Lua {
        let lua = sandboxed_lua();
        lua.set_app_data(std::cell::RefCell::new(
            crate::lua::api::WidgetAccumulator::new(),
        ));
        lua
    }

    #[test]
    fn instruction_limit_aborts_infinite_render() {
        let lua = sandboxed_lua_with_acc();
        lua.load(r#"function render() while true do end end"#)
            .exec()
            .unwrap();

        let widgets = handle_render(&lua, None);
        assert!(
            widgets.is_empty(),
            "render should return empty vec when aborted by instruction limit"
        );
    }

    #[test]
    fn instruction_limit_aborts_infinite_query() {
        let lua = sandboxed_lua();
        lua.load(r#"function on_query(method, args) while true do end end"#)
            .exec()
            .unwrap();

        let result = handle_query(&lua, "test", &serde_json::json!(null));
        // Should return None or null — not hang.
        assert!(
            result.is_none() || result == Some(serde_json::Value::Null),
            "on_query should return None/null when aborted, got {result:?}"
        );
    }

    #[test]
    fn instruction_limit_allows_normal_execution() {
        let lua = sandboxed_lua();
        // A simple function that does bounded work should complete fine.
        lua.load(
            r#"
            function on_event(e)
                local sum = 0
                for i = 1, 100 do sum = sum + i end
            end
        "#,
        )
        .exec()
        .unwrap();

        let event = PluginEvent::BusEvent {
            event_type: "test".into(),
            data: serde_json::Value::Null,
        };
        // Should complete without error.
        dispatch_event(&lua, &event);
    }
}
