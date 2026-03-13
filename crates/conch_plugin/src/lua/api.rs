//! Lua API table registration — `ui`, `app` tables with widget sugar.
//!
//! Each `ui.panel_*` function is syntactic sugar that constructs a `Widget`
//! enum variant and pushes it onto the widget accumulator. The accumulator
//! is drained after `render()` to produce the JSON widget tree.

use std::cell::RefCell;
use std::ffi::{CStr, CString};

use conch_plugin_sdk::widgets::*;
use conch_plugin_sdk::HostApi;
use mlua::prelude::*;

// ---------------------------------------------------------------------------
// Widget accumulator — thread-local stack of widget lists
// ---------------------------------------------------------------------------

/// Accumulates widgets during a `render()` call.
///
/// Uses a stack to support nested layout containers (`panel_horizontal`,
/// `panel_vertical`, etc.). The top of the stack is the current target.
pub struct WidgetAccumulator {
    stack: Vec<Vec<Widget>>,
}

impl WidgetAccumulator {
    pub fn new() -> Self {
        Self {
            stack: vec![vec![]],
        }
    }

    pub fn push_widget(&mut self, widget: Widget) {
        if let Some(top) = self.stack.last_mut() {
            top.push(widget);
        }
    }

    pub fn push_scope(&mut self) {
        self.stack.push(vec![]);
    }

    pub fn pop_scope(&mut self) -> Vec<Widget> {
        self.stack.pop().unwrap_or_default()
    }

    pub fn clear(&mut self) {
        self.stack.clear();
        self.stack.push(vec![]);
    }

    pub fn take_widgets(&mut self) -> Vec<Widget> {
        std::mem::take(self.stack.last_mut().unwrap_or(&mut vec![]))
    }
}

// ---------------------------------------------------------------------------
// Host API bridge — wraps the raw HostApi pointer for Lua access
// ---------------------------------------------------------------------------

/// Wraps a `*const HostApi` so it can be stored as Lua app data.
pub struct HostApiBridge {
    api: *const HostApi,
}

// SAFETY: HostApi function pointers are thread-safe (they dispatch to the
// host's event loop). The pointer is valid for the plugin's lifetime.
unsafe impl Send for HostApiBridge {}
unsafe impl Sync for HostApiBridge {}

impl HostApiBridge {
    pub fn new(api: *const HostApi) -> Self {
        Self { api }
    }

    fn api(&self) -> &HostApi {
        unsafe { &*self.api }
    }
}

// ---------------------------------------------------------------------------
// Registration — create and populate all Lua API tables
// ---------------------------------------------------------------------------

/// Register all API tables on the Lua VM.
///
/// Stores a `WidgetAccumulator` and `HostApiBridge` as app data.
pub fn register_all(lua: &Lua, host_api: *const HostApi) -> LuaResult<()> {
    lua.set_app_data(RefCell::new(WidgetAccumulator::new()));
    lua.set_app_data(HostApiBridge::new(host_api));

    register_ui_table(lua)?;
    register_app_table(lua)?;
    register_session_table(lua)?;
    register_net_table(lua)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// ui.* table
// ---------------------------------------------------------------------------

fn register_ui_table(lua: &Lua) -> LuaResult<()> {
    let ui = lua.create_table()?;

    // -- Widget accumulator control --

    ui.set(
        "panel_clear",
        lua.create_function(|lua, ()| {
            with_acc(lua, |acc| acc.clear());
            Ok(())
        })?,
    )?;

    // -- Data Display --

    ui.set(
        "panel_heading",
        lua.create_function(|lua, text: String| {
            with_acc(lua, |acc| acc.push_widget(Widget::Heading { text }));
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_label",
        lua.create_function(|lua, (text, style): (String, Option<String>)| {
            let style = style.and_then(|s| parse_text_style(&s));
            with_acc(lua, |acc| acc.push_widget(Widget::Label { text, style }));
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_text",
        lua.create_function(|lua, text: String| {
            with_acc(lua, |acc| acc.push_widget(Widget::Text { text }));
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_scroll_text",
        lua.create_function(|lua, (id, text, max_height): (String, String, Option<f32>)| {
            with_acc(lua, |acc| {
                acc.push_widget(Widget::ScrollText {
                    id,
                    text,
                    max_height,
                })
            });
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_kv",
        lua.create_function(|lua, (key, value): (String, String)| {
            with_acc(lua, |acc| acc.push_widget(Widget::KeyValue { key, value }));
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_separator",
        lua.create_function(|lua, ()| {
            with_acc(lua, |acc| acc.push_widget(Widget::Separator));
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_spacer",
        lua.create_function(|lua, size: Option<f32>| {
            with_acc(lua, |acc| acc.push_widget(Widget::Spacer { size }));
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_icon_label",
        lua.create_function(|lua, (icon, text, style): (String, String, Option<String>)| {
            let style = style.and_then(|s| parse_text_style(&s));
            with_acc(lua, |acc| {
                acc.push_widget(Widget::IconLabel { icon, text, style })
            });
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_badge",
        lua.create_function(|lua, (text, variant): (String, String)| {
            let variant = parse_badge_variant(&variant);
            with_acc(lua, |acc| acc.push_widget(Widget::Badge { text, variant }));
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_progress",
        lua.create_function(|lua, (id, fraction, label): (String, f32, Option<String>)| {
            with_acc(lua, |acc| {
                acc.push_widget(Widget::Progress {
                    id,
                    fraction,
                    label,
                })
            });
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_image",
        lua.create_function(
            |lua, (id, src, width, height): (Option<String>, String, Option<f32>, Option<f32>)| {
                with_acc(lua, |acc| {
                    acc.push_widget(Widget::Image {
                        id,
                        src,
                        width,
                        height,
                    })
                });
                Ok(())
            },
        )?,
    )?;

    // -- Interactive Widgets --

    ui.set(
        "panel_button",
        lua.create_function(
            |lua, (id, label, icon): (String, String, Option<String>)| {
                with_acc(lua, |acc| {
                    acc.push_widget(Widget::Button {
                        id,
                        label,
                        icon,
                        enabled: None,
                    })
                });
                Ok(())
            },
        )?,
    )?;

    ui.set(
        "panel_text_input",
        lua.create_function(|lua, (id, value, hint): (String, String, Option<String>)| {
            with_acc(lua, |acc| {
                acc.push_widget(Widget::TextInput {
                    id,
                    value,
                    hint,
                    submit_on_enter: Some(true),
                })
            });
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_text_edit",
        lua.create_function(
            |lua, (id, value, hint, lines): (String, String, Option<String>, Option<u32>)| {
                with_acc(lua, |acc| {
                    acc.push_widget(Widget::TextEdit {
                        id,
                        value,
                        hint,
                        lines,
                    })
                });
                Ok(())
            },
        )?,
    )?;

    ui.set(
        "panel_checkbox",
        lua.create_function(|lua, (id, label, checked): (String, String, bool)| {
            with_acc(lua, |acc| {
                acc.push_widget(Widget::Checkbox { id, label, checked })
            });
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_combobox",
        lua.create_function(|lua, (id, selected, options): (String, String, LuaTable)| {
            let options = lua_to_combobox_options(&options)?;
            with_acc(lua, |acc| {
                acc.push_widget(Widget::ComboBox {
                    id,
                    selected,
                    options,
                })
            });
            Ok(())
        })?,
    )?;

    // -- Complex Widgets --

    ui.set(
        "panel_table",
        lua.create_function(|lua, (columns, rows): (LuaValue, LuaValue)| {
            let widget = build_table_widget(columns, rows)?;
            with_acc(lua, |acc| acc.push_widget(widget));
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_tree",
        lua.create_function(|lua, (id, nodes, selected): (String, LuaTable, Option<String>)| {
            let nodes = lua_to_tree_nodes(&nodes)?;
            with_acc(lua, |acc| {
                acc.push_widget(Widget::TreeView {
                    id,
                    nodes,
                    selected,
                })
            });
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_toolbar",
        lua.create_function(|lua, (id, items): (Option<String>, LuaTable)| {
            let items = lua_to_toolbar_items(&items)?;
            with_acc(lua, |acc| acc.push_widget(Widget::Toolbar { id, items }));
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_path_bar",
        lua.create_function(|lua, (id, segments): (String, Vec<String>)| {
            with_acc(lua, |acc| {
                acc.push_widget(Widget::PathBar { id, segments })
            });
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_tabs",
        lua.create_function(|lua, (id, active, tabs): (String, usize, LuaTable)| {
            let tabs = lua_to_tab_panes(&tabs)?;
            with_acc(lua, |acc| {
                acc.push_widget(Widget::Tabs { id, active, tabs })
            });
            Ok(())
        })?,
    )?;

    // -- Layout Containers --

    ui.set(
        "panel_horizontal",
        lua.create_function(|lua, (func, spacing): (LuaFunction, Option<f32>)| {
            with_acc(lua, |acc| acc.push_scope());
            func.call::<()>(())?;
            let children = with_acc(lua, |acc| acc.pop_scope());
            with_acc(lua, |acc| {
                acc.push_widget(Widget::Horizontal {
                    id: None,
                    children,
                    spacing,
                })
            });
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_vertical",
        lua.create_function(|lua, (func, spacing): (LuaFunction, Option<f32>)| {
            with_acc(lua, |acc| acc.push_scope());
            func.call::<()>(())?;
            let children = with_acc(lua, |acc| acc.pop_scope());
            with_acc(lua, |acc| {
                acc.push_widget(Widget::Vertical {
                    id: None,
                    children,
                    spacing,
                })
            });
            Ok(())
        })?,
    )?;

    ui.set(
        "panel_scroll_area",
        lua.create_function(
            |lua, (func, max_height): (LuaFunction, Option<f32>)| {
                with_acc(lua, |acc| acc.push_scope());
                func.call::<()>(())?;
                let children = with_acc(lua, |acc| acc.pop_scope());
                with_acc(lua, |acc| {
                    acc.push_widget(Widget::ScrollArea {
                        id: None,
                        max_height,
                        children,
                    })
                });
                Ok(())
            },
        )?,
    )?;

    ui.set(
        "panel_drop_zone",
        lua.create_function(
            |lua, (id, label, func): (String, String, Option<LuaFunction>)| {
                let children = if let Some(f) = func {
                    with_acc(lua, |acc| acc.push_scope());
                    f.call::<()>(())?;
                    with_acc(lua, |acc| acc.pop_scope())
                } else {
                    vec![]
                };
                with_acc(lua, |acc| {
                    acc.push_widget(Widget::DropZone {
                        id,
                        label,
                        children,
                    })
                });
                Ok(())
            },
        )?,
    )?;

    // -- Dialogs (blocking, call through HostApi) --

    ui.set(
        "form",
        lua.create_function(|lua, (title, fields): (String, LuaTable)| {
            let form_json = build_form_json(&title, &fields)?;
            let result = call_show_form(lua, &form_json)?;
            Ok(result)
        })?,
    )?;

    ui.set(
        "alert",
        lua.create_function(|lua, (title, msg): (String, String)| {
            call_show_alert(lua, &title, &msg);
            Ok(())
        })?,
    )?;

    ui.set(
        "error",
        lua.create_function(|lua, (title, msg): (String, String)| {
            call_show_error(lua, &title, &msg);
            Ok(())
        })?,
    )?;

    ui.set(
        "confirm",
        lua.create_function(|lua, msg: String| {
            let result = call_show_confirm(lua, &msg);
            Ok(result)
        })?,
    )?;

    ui.set(
        "prompt",
        lua.create_function(|lua, (msg, default): (String, Option<String>)| {
            let result = call_show_prompt(lua, &msg, default.as_deref().unwrap_or(""));
            Ok(result)
        })?,
    )?;

    lua.globals().set("ui", ui)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// app.* table
// ---------------------------------------------------------------------------

fn register_app_table(lua: &Lua) -> LuaResult<()> {
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
            with_host_api(lua, |api| {
                let c_msg = CString::new(msg).unwrap_or_default();
                (api.log)(level_num, c_msg.as_ptr());
            });
            Ok(())
        })?,
    )?;

    app.set(
        "clipboard",
        lua.create_function(|lua, text: String| {
            with_host_api(lua, |api| {
                let c_text = CString::new(text).unwrap_or_default();
                (api.clipboard_set)(c_text.as_ptr());
            });
            Ok(())
        })?,
    )?;

    app.set(
        "clipboard_get",
        lua.create_function(|lua, ()| {
            let result = with_host_api(lua, |api| {
                let ptr = (api.clipboard_get)();
                if ptr.is_null() {
                    return None;
                }
                let s = unsafe { CStr::from_ptr(ptr) }
                    .to_str()
                    .unwrap_or("")
                    .to_string();
                (api.free_string)(ptr);
                Some(s)
            });
            Ok(result)
        })?,
    )?;

    app.set(
        "publish",
        lua.create_function(|lua, (event_type, data): (String, LuaValue)| {
            let data_json = serde_json::to_string(&lua_value_to_json(data)?)
                .unwrap_or_else(|_| "{}".to_string());
            with_host_api(lua, |api| {
                let c_type = CString::new(event_type).unwrap_or_default();
                let c_data = CString::new(data_json.clone()).unwrap_or_default();
                (api.publish_event)(c_type.as_ptr(), c_data.as_ptr(), data_json.len());
            });
            Ok(())
        })?,
    )?;

    app.set(
        "subscribe",
        lua.create_function(|lua, event_type: String| {
            with_host_api(lua, |api| {
                let c_type = CString::new(event_type).unwrap_or_default();
                (api.subscribe)(c_type.as_ptr());
            });
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
                with_host_api(lua, |api| {
                    let c_json = CString::new(json.clone()).unwrap_or_default();
                    (api.notify)(c_json.as_ptr(), json.len());
                });
                Ok(())
            },
        )?,
    )?;

    app.set(
        "register_service",
        lua.create_function(|lua, name: String| {
            with_host_api(lua, |api| {
                let c_name = CString::new(name).unwrap_or_default();
                (api.register_service)(c_name.as_ptr());
            });
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
                let result = with_host_api(lua, |api| {
                    let c_target = CString::new(target).unwrap_or_default();
                    let c_method = CString::new(method).unwrap_or_default();
                    let c_args = CString::new(args_json.clone()).unwrap_or_default();
                    let ptr = (api.query_plugin)(
                        c_target.as_ptr(),
                        c_method.as_ptr(),
                        c_args.as_ptr(),
                        args_json.len(),
                    );
                    if ptr.is_null() {
                        return None;
                    }
                    let s = unsafe { CStr::from_ptr(ptr) }
                        .to_str()
                        .unwrap_or("")
                        .to_string();
                    (api.free_string)(ptr);
                    Some(s)
                });
                Ok(result)
            },
        )?,
    )?;

    app.set(
        "get_config",
        lua.create_function(|lua, key: String| {
            let result = with_host_api(lua, |api| {
                let c_key = CString::new(key).unwrap_or_default();
                let ptr = (api.get_config)(c_key.as_ptr());
                if ptr.is_null() {
                    return None;
                }
                let s = unsafe { CStr::from_ptr(ptr) }
                    .to_str()
                    .unwrap_or("")
                    .to_string();
                (api.free_string)(ptr);
                Some(s)
            });
            Ok(result)
        })?,
    )?;

    app.set(
        "set_config",
        lua.create_function(|lua, (key, value): (String, String)| {
            with_host_api(lua, |api| {
                let c_key = CString::new(key).unwrap_or_default();
                let c_value = CString::new(value).unwrap_or_default();
                (api.set_config)(c_key.as_ptr(), c_value.as_ptr());
            });
            Ok(())
        })?,
    )?;

    lua.globals().set("app", app)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// session.* table (stubs — requires session backend wiring)
// ---------------------------------------------------------------------------

fn register_session_table(lua: &Lua) -> LuaResult<()> {
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

    // Execute a command locally (not in the terminal PTY).
    // For SSH session exec, use app.query_plugin("ssh", "exec", {...}).
    session.set(
        "exec",
        lua.create_function(|_lua, cmd: String| -> LuaResult<LuaTable> {
            let result = _lua.create_table()?;
            match std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .output()
            {
                Ok(output) => {
                    result.set("stdout", String::from_utf8_lossy(&output.stdout).to_string())?;
                    result.set("stderr", String::from_utf8_lossy(&output.stderr).to_string())?;
                    result.set("exit_code", output.status.code().unwrap_or(-1))?;
                    result.set("status", "ok")?;
                }
                Err(e) => {
                    result.set("stdout", "")?;
                    result.set("stderr", e.to_string())?;
                    result.set("exit_code", -1)?;
                    result.set("status", "error")?;
                }
            }
            Ok(result)
        })?,
    )?;

    // Get info about the currently active session.
    // Returns a table with basic session info. For detailed info about SSH
    // sessions, use app.query_plugin("ssh", "get_sessions").
    session.set(
        "current",
        lua.create_function(|_lua, ()| -> LuaResult<LuaTable> {
            let tbl = _lua.create_table()?;
            let platform = if cfg!(target_os = "macos") {
                "macos"
            } else if cfg!(target_os = "linux") {
                "linux"
            } else if cfg!(target_os = "windows") {
                "windows"
            } else {
                "unknown"
            };
            tbl.set("platform", platform)?;
            tbl.set("type", "local")?;
            Ok(tbl)
        })?,
    )?;

    lua.globals().set("session", session)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// net.* table (stubs — network ops would be implemented host-side)
// ---------------------------------------------------------------------------

fn register_net_table(lua: &Lua) -> LuaResult<()> {
    let net = lua.create_table()?;

    net.set(
        "time",
        lua.create_function(|_lua, ()| {
            use std::time::{SystemTime, UNIX_EPOCH};
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            Ok(secs)
        })?,
    )?;

    net.set(
        "resolve",
        lua.create_function(|_lua, host: String| -> LuaResult<Vec<String>> {
            use std::net::ToSocketAddrs;
            let addr = format!("{host}:0");
            match addr.to_socket_addrs() {
                Ok(addrs) => Ok(addrs.map(|a| a.ip().to_string()).collect()),
                Err(_) => Ok(vec![]),
            }
        })?,
    )?;

    net.set(
        "scan",
        lua.create_function(
            |_lua,
             (host, ports, timeout_ms, _concurrency): (
                String,
                Vec<u16>,
                Option<u64>,
                Option<u32>,
            )|
             -> LuaResult<Vec<LuaTable>> {
                use std::net::{TcpStream, ToSocketAddrs};
                use std::time::Duration;

                let timeout = Duration::from_millis(timeout_ms.unwrap_or(1000));
                let mut results = Vec::new();

                for port in ports {
                    let addr_str = format!("{host}:{port}");
                    let open = match addr_str.to_socket_addrs() {
                        Ok(mut addrs) => {
                            if let Some(addr) = addrs.next() {
                                TcpStream::connect_timeout(&addr, timeout).is_ok()
                            } else {
                                false
                            }
                        }
                        Err(_) => false,
                    };
                    if open {
                        let tbl = _lua.create_table()?;
                        tbl.set("port", port)?;
                        tbl.set("open", true)?;
                        results.push(tbl);
                    }
                }

                Ok(results)
            },
        )?,
    )?;

    lua.globals().set("net", net)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers — widget accumulator + host API access
// ---------------------------------------------------------------------------

/// Borrow the widget accumulator, call the closure, then release.
fn with_acc<F, R>(lua: &Lua, f: F) -> R
where
    F: FnOnce(&mut WidgetAccumulator) -> R,
{
    with_acc_pub(lua, f)
}

/// Public version of `with_acc` for use by the runner module.
pub fn with_acc_pub<F, R>(lua: &Lua, f: F) -> R
where
    F: FnOnce(&mut WidgetAccumulator) -> R,
{
    let cell = lua
        .app_data_ref::<RefCell<WidgetAccumulator>>()
        .expect("WidgetAccumulator not set");
    let mut acc = cell.borrow_mut();
    f(&mut acc)
}

/// Borrow the HostApi bridge, call the closure.
fn with_host_api<F, R>(lua: &Lua, f: F) -> R
where
    F: FnOnce(&HostApi) -> R,
{
    let bridge = lua
        .app_data_ref::<HostApiBridge>()
        .expect("HostApiBridge not set");
    f(bridge.api())
}

/// Take the accumulated widgets from the current render call.
pub fn take_widgets(lua: &Lua) -> Vec<Widget> {
    with_acc(lua, |acc| acc.take_widgets())
}

// ---------------------------------------------------------------------------
// Lua → Rust type conversions
// ---------------------------------------------------------------------------

fn parse_text_style(s: &str) -> Option<TextStyle> {
    match s {
        "normal" => Some(TextStyle::Normal),
        "secondary" => Some(TextStyle::Secondary),
        "muted" => Some(TextStyle::Muted),
        "accent" => Some(TextStyle::Accent),
        "warn" => Some(TextStyle::Warn),
        "error" => Some(TextStyle::Error),
        _ => None,
    }
}

fn parse_badge_variant(s: &str) -> BadgeVariant {
    match s {
        "info" => BadgeVariant::Info,
        "success" => BadgeVariant::Success,
        "warn" | "warning" => BadgeVariant::Warn,
        "error" => BadgeVariant::Error,
        _ => BadgeVariant::Info,
    }
}

/// Convert a Lua combobox options table to `Vec<ComboBoxOption>`.
///
/// Supports two formats:
/// - Array of `{ value = "...", label = "..." }` tables
/// - Array of strings (value and label are the same)
fn lua_to_combobox_options(tbl: &LuaTable) -> LuaResult<Vec<ComboBoxOption>> {
    let mut options = Vec::new();
    for pair in tbl.clone().sequence_values::<LuaValue>() {
        let val = pair?;
        match val {
            LuaValue::Table(t) => {
                let value: String = t.get("value")?;
                let label: String = t.get("label").unwrap_or_else(|_| value.clone());
                options.push(ComboBoxOption { value, label });
            }
            LuaValue::String(s) => {
                let s = s.to_str()?.to_string();
                options.push(ComboBoxOption {
                    label: s.clone(),
                    value: s,
                });
            }
            _ => {}
        }
    }
    Ok(options)
}

/// Build a `Widget::Table` from Lua arguments.
///
/// Simple form: `panel_table({"Col1", "Col2"}, {{"a", "b"}, {"c", "d"}})`
/// Advanced form: `panel_table({ id = "t", columns = {...}, rows = {...}, ... })`
fn build_table_widget(columns: LuaValue, rows: LuaValue) -> LuaResult<Widget> {
    match (&columns, &rows) {
        // Simple form: array of column names + array of row arrays.
        (LuaValue::Table(col_tbl), LuaValue::Table(row_tbl)) => {
            // Check if this is the advanced form (columns key exists).
            if col_tbl.contains_key("columns")? {
                return build_table_advanced(col_tbl);
            }

            let col_names: Vec<String> = col_tbl.clone().sequence_values().collect::<LuaResult<_>>()?;
            let columns: Vec<TableColumn> = col_names
                .into_iter()
                .enumerate()
                .map(|(i, label)| TableColumn {
                    id: format!("col_{i}"),
                    label,
                    sortable: None,
                    width: None,
                    visible: None,
                })
                .collect();

            let mut table_rows = Vec::new();
            for (i, row_val) in row_tbl.clone().sequence_values::<LuaTable>().enumerate() {
                let row = row_val?;
                let cells: Vec<TableCell> = row
                    .sequence_values::<String>()
                    .map(|v| v.map(TableCell::Text))
                    .collect::<LuaResult<_>>()?;
                table_rows.push(TableRow {
                    id: format!("row_{i}"),
                    cells,
                    context_menu: None,
                });
            }

            Ok(Widget::Table {
                id: "table".into(),
                columns,
                rows: table_rows,
                sort_column: None,
                sort_ascending: None,
                selected_row: None,
            })
        }
        _ => Err(LuaError::RuntimeError(
            "panel_table expects (columns, rows) tables".into(),
        )),
    }
}

fn build_table_advanced(tbl: &LuaTable) -> LuaResult<Widget> {
    let id: String = tbl.get("id").unwrap_or_else(|_| "table".into());
    let col_tbl: LuaTable = tbl.get("columns")?;
    let row_tbl: LuaTable = tbl.get("rows")?;

    let columns: Vec<TableColumn> = col_tbl
        .sequence_values::<LuaTable>()
        .map(|t| {
            let t = t?;
            Ok(TableColumn {
                id: t.get("id")?,
                label: t.get("label")?,
                sortable: t.get("sortable").ok(),
                width: t.get("width").ok(),
                visible: t.get("visible").ok(),
            })
        })
        .collect::<LuaResult<_>>()?;

    let rows: Vec<TableRow> = row_tbl
        .sequence_values::<LuaTable>()
        .map(|t| {
            let t = t?;
            let cells_tbl: LuaTable = t.get("cells")?;
            let cells: Vec<TableCell> = cells_tbl
                .sequence_values::<String>()
                .map(|v| v.map(TableCell::Text))
                .collect::<LuaResult<_>>()?;
            Ok(TableRow {
                id: t.get("id").unwrap_or_else(|_| "row".into()),
                cells,
                context_menu: None,
            })
        })
        .collect::<LuaResult<_>>()?;

    Ok(Widget::Table {
        id,
        columns,
        rows,
        sort_column: tbl.get("sort_column").ok(),
        sort_ascending: tbl.get("sort_ascending").ok(),
        selected_row: tbl.get("selected_row").ok(),
    })
}

fn lua_to_tree_nodes(tbl: &LuaTable) -> LuaResult<Vec<TreeNode>> {
    tbl.clone()
        .sequence_values::<LuaTable>()
        .map(|t| lua_to_tree_node(&t?))
        .collect()
}

fn lua_to_tree_node(tbl: &LuaTable) -> LuaResult<TreeNode> {
    let children = if let Ok(children_tbl) = tbl.get::<LuaTable>("children") {
        lua_to_tree_nodes(&children_tbl)?
    } else {
        vec![]
    };

    let context_menu = if let Ok(menu_tbl) = tbl.get::<LuaTable>("context_menu") {
        Some(lua_to_context_menu_items(&menu_tbl)?)
    } else {
        None
    };

    Ok(TreeNode {
        id: tbl.get("id")?,
        label: tbl.get("label")?,
        icon: tbl.get("icon").ok(),
        icon_color: tbl.get("icon_color").ok(),
        bold: tbl.get("bold").ok(),
        badge: tbl.get("badge").ok(),
        expanded: tbl.get("expanded").ok(),
        children,
        context_menu,
    })
}

fn lua_to_context_menu_items(tbl: &LuaTable) -> LuaResult<Vec<ContextMenuItem>> {
    tbl.clone()
        .sequence_values::<LuaTable>()
        .map(|t| {
            let t = t?;
            Ok(ContextMenuItem {
                id: t.get("id")?,
                label: t.get("label")?,
                icon: t.get("icon").ok(),
                enabled: t.get("enabled").ok(),
                shortcut: t.get("shortcut").ok(),
            })
        })
        .collect()
}

fn lua_to_toolbar_items(tbl: &LuaTable) -> LuaResult<Vec<ToolbarItem>> {
    tbl.clone()
        .sequence_values::<LuaTable>()
        .map(|t| {
            let t = t?;
            let item_type: String = t.get("type").unwrap_or_else(|_| "button".into());
            match item_type.as_str() {
                "separator" => Ok(ToolbarItem::Separator),
                "spacer" => Ok(ToolbarItem::Spacer),
                "text_input" => Ok(ToolbarItem::TextInput {
                    id: t.get("id")?,
                    value: t.get("value").unwrap_or_default(),
                    hint: t.get("hint").ok(),
                }),
                _ => Ok(ToolbarItem::Button {
                    id: t.get("id")?,
                    icon: t.get("icon").ok(),
                    label: t.get("label").ok(),
                    tooltip: t.get("tooltip").ok(),
                    enabled: t.get("enabled").ok(),
                }),
            }
        })
        .collect()
}

fn lua_to_tab_panes(tbl: &LuaTable) -> LuaResult<Vec<TabPane>> {
    tbl.clone()
        .sequence_values::<LuaTable>()
        .map(|t| {
            let t = t?;
            Ok(TabPane {
                label: t.get("label")?,
                icon: t.get("icon").ok(),
                children: vec![], // Tab pane children set via layout container calls.
            })
        })
        .collect()
}

/// Convert a Lua value to serde_json::Value.
fn lua_value_to_json(value: LuaValue) -> LuaResult<serde_json::Value> {
    match value {
        LuaValue::Nil => Ok(serde_json::Value::Null),
        LuaValue::Boolean(b) => Ok(serde_json::Value::Bool(b)),
        LuaValue::Integer(i) => Ok(serde_json::json!(i)),
        LuaValue::Number(n) => Ok(serde_json::json!(n)),
        LuaValue::String(s) => Ok(serde_json::Value::String(s.to_str()?.to_string())),
        LuaValue::Table(t) => {
            // Check if this is an array (sequential integer keys from 1).
            let len = t.raw_len();
            if len > 0 {
                let mut arr = Vec::new();
                for v in t.clone().sequence_values::<LuaValue>() {
                    arr.push(lua_value_to_json(v?)?);
                }
                if arr.len() == len {
                    return Ok(serde_json::Value::Array(arr));
                }
            }
            // Otherwise, treat as object.
            let mut map = serde_json::Map::new();
            for pair in t.pairs::<String, LuaValue>() {
                let (k, v) = pair?;
                map.insert(k, lua_value_to_json(v)?);
            }
            Ok(serde_json::Value::Object(map))
        }
        _ => Ok(serde_json::Value::Null),
    }
}

// ---------------------------------------------------------------------------
// Dialog helpers — call through HostApi
// ---------------------------------------------------------------------------

fn build_form_json(title: &str, fields: &LuaTable) -> LuaResult<String> {
    let mut form_fields = Vec::new();
    for field in fields.clone().sequence_values::<LuaTable>() {
        let field = field?;
        let field_type: String = field.get("type").unwrap_or_else(|_| "text".into());
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), serde_json::json!(field_type));

        // Copy common fields.
        for key in &["id", "name", "label", "text", "value", "default"] {
            if let Ok(v) = field.get::<LuaValue>(key.to_string()) {
                if !matches!(v, LuaValue::Nil) {
                    obj.insert(key.to_string(), lua_value_to_json(v)?);
                }
            }
        }

        // Combo options.
        if let Ok(opts) = field.get::<LuaTable>("options") {
            let opts: Vec<String> = opts.sequence_values().collect::<LuaResult<_>>()?;
            obj.insert("options".into(), serde_json::json!(opts));
        }

        form_fields.push(serde_json::Value::Object(obj));
    }

    let form = serde_json::json!({
        "title": title,
        "fields": form_fields,
    });
    Ok(form.to_string())
}

fn call_show_form(lua: &Lua, json: &str) -> LuaResult<Option<LuaTable>> {
    let result_ptr = with_host_api(lua, |api| {
        let c_json = CString::new(json).unwrap_or_default();
        (api.show_form)(c_json.as_ptr(), json.len())
    });

    if result_ptr.is_null() {
        return Ok(None);
    }

    let result_str = unsafe { CStr::from_ptr(result_ptr) }
        .to_str()
        .unwrap_or("{}")
        .to_string();

    with_host_api(lua, |api| (api.free_string)(result_ptr));

    // Parse JSON result into a Lua table.
    let json_value: serde_json::Value =
        serde_json::from_str(&result_str).unwrap_or(serde_json::Value::Null);
    let tbl = json_to_lua_table(lua, &json_value)?;
    Ok(Some(tbl))
}

fn call_show_alert(lua: &Lua, title: &str, msg: &str) {
    with_host_api(lua, |api| {
        let c_title = CString::new(title).unwrap_or_default();
        let c_msg = CString::new(msg).unwrap_or_default();
        (api.show_alert)(c_title.as_ptr(), c_msg.as_ptr());
    });
}

fn call_show_error(lua: &Lua, title: &str, msg: &str) {
    with_host_api(lua, |api| {
        let c_title = CString::new(title).unwrap_or_default();
        let c_msg = CString::new(msg).unwrap_or_default();
        (api.show_error)(c_title.as_ptr(), c_msg.as_ptr());
    });
}

fn call_show_confirm(lua: &Lua, msg: &str) -> bool {
    with_host_api(lua, |api| {
        let c_msg = CString::new(msg).unwrap_or_default();
        (api.show_confirm)(c_msg.as_ptr())
    })
}

fn call_show_prompt(lua: &Lua, msg: &str, default: &str) -> Option<String> {
    with_host_api(lua, |api| {
        let c_msg = CString::new(msg).unwrap_or_default();
        let c_default = CString::new(default).unwrap_or_default();
        let ptr = (api.show_prompt)(c_msg.as_ptr(), c_default.as_ptr());
        if ptr.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .unwrap_or("")
            .to_string();
        (api.free_string)(ptr);
        Some(s)
    })
}

/// Convert a serde_json::Value to a Lua table.
fn json_to_lua_table(lua: &Lua, value: &serde_json::Value) -> LuaResult<LuaTable> {
    let tbl = lua.create_table()?;
    if let serde_json::Value::Object(map) = value {
        for (k, v) in map {
            tbl.set(k.clone(), json_to_lua_value(lua, v)?)?;
        }
    }
    Ok(tbl)
}

fn json_to_lua_value(lua: &Lua, value: &serde_json::Value) -> LuaResult<LuaValue> {
    match value {
        serde_json::Value::Null => Ok(LuaValue::Nil),
        serde_json::Value::Bool(b) => Ok(LuaValue::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i))
            } else {
                Ok(LuaValue::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(LuaValue::String(lua.create_string(s)?)),
        serde_json::Value::Array(arr) => {
            let tbl = lua.create_table()?;
            for (i, v) in arr.iter().enumerate() {
                tbl.set(i + 1, json_to_lua_value(lua, v)?)?;
            }
            Ok(LuaValue::Table(tbl))
        }
        serde_json::Value::Object(_) => {
            let tbl = json_to_lua_table(lua, value)?;
            Ok(LuaValue::Table(tbl))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_lua() -> Lua {
        let lua = Lua::new();
        lua.set_app_data(RefCell::new(WidgetAccumulator::new()));
        lua
    }

    #[test]
    fn widget_accumulator_basic() {
        let mut acc = WidgetAccumulator::new();
        acc.push_widget(Widget::Separator);
        acc.push_widget(Widget::Heading {
            text: "Hi".into(),
        });
        let widgets = acc.take_widgets();
        assert_eq!(widgets.len(), 2);
    }

    #[test]
    fn widget_accumulator_nested_scope() {
        let mut acc = WidgetAccumulator::new();
        acc.push_widget(Widget::Separator);
        acc.push_scope();
        acc.push_widget(Widget::Heading {
            text: "Child".into(),
        });
        let children = acc.pop_scope();
        assert_eq!(children.len(), 1);
        // Parent still has the separator.
        let parent = acc.take_widgets();
        assert_eq!(parent.len(), 1);
    }

    #[test]
    fn widget_accumulator_clear() {
        let mut acc = WidgetAccumulator::new();
        acc.push_widget(Widget::Separator);
        acc.push_widget(Widget::Separator);
        acc.clear();
        let widgets = acc.take_widgets();
        assert!(widgets.is_empty());
    }

    #[test]
    fn parse_text_styles() {
        assert!(matches!(parse_text_style("muted"), Some(TextStyle::Muted)));
        assert!(matches!(
            parse_text_style("accent"),
            Some(TextStyle::Accent)
        ));
        assert!(parse_text_style("invalid").is_none());
    }

    #[test]
    fn parse_badge_variants() {
        assert!(matches!(parse_badge_variant("success"), BadgeVariant::Success));
        assert!(matches!(parse_badge_variant("warn"), BadgeVariant::Warn));
        assert!(matches!(parse_badge_variant("warning"), BadgeVariant::Warn));
        assert!(matches!(parse_badge_variant("unknown"), BadgeVariant::Info));
    }

    #[test]
    fn lua_heading_pushes_widget() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(r#"ui.panel_heading("Hello")"#).exec().unwrap();
        let widgets = take_widgets(&lua);
        assert_eq!(widgets.len(), 1);
        assert!(matches!(&widgets[0], Widget::Heading { text } if text == "Hello"));
    }

    #[test]
    fn lua_multiple_widgets() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(
            r#"
            ui.panel_heading("Title")
            ui.panel_separator()
            ui.panel_label("Hello")
            ui.panel_kv("Key", "Value")
            ui.panel_button("btn", "Click")
        "#,
        )
        .exec()
        .unwrap();
        let widgets = take_widgets(&lua);
        assert_eq!(widgets.len(), 5);
    }

    #[test]
    fn lua_panel_clear() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(
            r#"
            ui.panel_heading("Old")
            ui.panel_clear()
            ui.panel_heading("New")
        "#,
        )
        .exec()
        .unwrap();
        let widgets = take_widgets(&lua);
        assert_eq!(widgets.len(), 1);
        assert!(matches!(&widgets[0], Widget::Heading { text } if text == "New"));
    }

    #[test]
    fn lua_horizontal_container() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(
            r#"
            ui.panel_horizontal(function()
                ui.panel_button("a", "A")
                ui.panel_button("b", "B")
            end)
        "#,
        )
        .exec()
        .unwrap();
        let widgets = take_widgets(&lua);
        assert_eq!(widgets.len(), 1);
        if let Widget::Horizontal { children, .. } = &widgets[0] {
            assert_eq!(children.len(), 2);
        } else {
            panic!("Expected Horizontal");
        }
    }

    #[test]
    fn lua_checkbox() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(r#"ui.panel_checkbox("cb", "Enable", true)"#)
            .exec()
            .unwrap();
        let widgets = take_widgets(&lua);
        assert_eq!(widgets.len(), 1);
        if let Widget::Checkbox {
            id, checked, label, ..
        } = &widgets[0]
        {
            assert_eq!(id, "cb");
            assert_eq!(label, "Enable");
            assert!(checked);
        } else {
            panic!("Expected Checkbox");
        }
    }

    #[test]
    fn lua_combobox_with_tables() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(
            r#"
            ui.panel_combobox("sel", "a", {
                { value = "a", label = "Alpha" },
                { value = "b", label = "Beta" },
            })
        "#,
        )
        .exec()
        .unwrap();
        let widgets = take_widgets(&lua);
        if let Widget::ComboBox { options, .. } = &widgets[0] {
            assert_eq!(options.len(), 2);
            assert_eq!(options[0].value, "a");
            assert_eq!(options[0].label, "Alpha");
        } else {
            panic!("Expected ComboBox");
        }
    }

    #[test]
    fn lua_simple_table() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(
            r#"
            ui.panel_table({"Name", "Value"}, {
                {"CPU", "45%"},
                {"RAM", "72%"},
            })
        "#,
        )
        .exec()
        .unwrap();
        let widgets = take_widgets(&lua);
        if let Widget::Table { columns, rows, .. } = &widgets[0] {
            assert_eq!(columns.len(), 2);
            assert_eq!(rows.len(), 2);
        } else {
            panic!("Expected Table");
        }
    }

    #[test]
    fn lua_badge() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(r#"ui.panel_badge("ok", "success")"#)
            .exec()
            .unwrap();
        let widgets = take_widgets(&lua);
        if let Widget::Badge { text, variant } = &widgets[0] {
            assert_eq!(text, "ok");
            assert!(matches!(variant, BadgeVariant::Success));
        } else {
            panic!("Expected Badge");
        }
    }

    #[test]
    fn lua_progress() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(r#"ui.panel_progress("p", 0.75, "75%")"#)
            .exec()
            .unwrap();
        let widgets = take_widgets(&lua);
        if let Widget::Progress {
            fraction, label, ..
        } = &widgets[0]
        {
            assert!((fraction - 0.75).abs() < 0.01);
            assert_eq!(label.as_deref(), Some("75%"));
        } else {
            panic!("Expected Progress");
        }
    }

    #[test]
    fn lua_text_input() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(r#"ui.panel_text_input("search", "", "Search...")"#)
            .exec()
            .unwrap();
        let widgets = take_widgets(&lua);
        if let Widget::TextInput { id, hint, .. } = &widgets[0] {
            assert_eq!(id, "search");
            assert_eq!(hint.as_deref(), Some("Search..."));
        } else {
            panic!("Expected TextInput");
        }
    }

    #[test]
    fn lua_path_bar() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(r#"ui.panel_path_bar("path", {"~", "projects", "conch"})"#)
            .exec()
            .unwrap();
        let widgets = take_widgets(&lua);
        if let Widget::PathBar { segments, .. } = &widgets[0] {
            assert_eq!(segments, &["~", "projects", "conch"]);
        } else {
            panic!("Expected PathBar");
        }
    }

    #[test]
    fn lua_value_to_json_primitives() {
        assert_eq!(
            lua_value_to_json(LuaValue::Nil).unwrap(),
            serde_json::Value::Null
        );
        assert_eq!(
            lua_value_to_json(LuaValue::Boolean(true)).unwrap(),
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            lua_value_to_json(LuaValue::Integer(42)).unwrap(),
            serde_json::json!(42)
        );
        assert_eq!(
            lua_value_to_json(LuaValue::Number(3.14)).unwrap(),
            serde_json::json!(3.14)
        );
    }

    #[test]
    fn json_to_lua_roundtrip() {
        let lua = Lua::new();
        let json = serde_json::json!({"name": "test", "count": 42, "active": true});
        let tbl = json_to_lua_table(&lua, &json).unwrap();
        assert_eq!(tbl.get::<String>("name").unwrap(), "test");
        assert_eq!(tbl.get::<i64>("count").unwrap(), 42);
        assert!(tbl.get::<bool>("active").unwrap());
    }

    #[test]
    fn lua_nested_vertical_in_horizontal() {
        let lua = make_test_lua();
        register_ui_table(&lua).unwrap();
        lua.load(
            r#"
            ui.panel_horizontal(function()
                ui.panel_vertical(function()
                    ui.panel_label("A")
                    ui.panel_label("B")
                end)
                ui.panel_button("btn", "Go")
            end)
        "#,
        )
        .exec()
        .unwrap();
        let widgets = take_widgets(&lua);
        assert_eq!(widgets.len(), 1);
        if let Widget::Horizontal { children, .. } = &widgets[0] {
            assert_eq!(children.len(), 2);
            if let Widget::Vertical {
                children: inner, ..
            } = &children[0]
            {
                assert_eq!(inner.len(), 2);
            } else {
                panic!("Expected Vertical inside Horizontal");
            }
        } else {
            panic!("Expected Horizontal");
        }
    }
}
