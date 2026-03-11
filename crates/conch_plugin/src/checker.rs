//! Plugin syntax and API checker.
//!
//! Validates Lua plugin files by:
//! 1. Checking plugin header metadata for well-formed comments.
//! 2. Parsing the Lua source for syntax errors.
//! 3. Loading the script into a sandboxed Lua environment with stub API tables
//!    that validate function names and argument counts.

use std::path::Path;

use mlua::prelude::*;

use crate::manager::{PluginMeta, PluginType};

/// Severity level for a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A single diagnostic message.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// 1-based line number (0 = unknown).
    pub line: usize,
    /// 1-based column number (0 = unknown).
    pub col: usize,
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        if self.line > 0 {
            write!(f, "{}:{}: {sev}: {}", self.line, self.col.max(1), self.message)
        } else {
            write!(f, "{sev}: {}", self.message)
        }
    }
}

/// Result of checking a plugin file.
pub struct CheckResult {
    pub path: std::path::PathBuf,
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckResult {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn has_warnings(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Warning)
    }
}

/// Check a plugin file for syntax errors, header issues, and API misuse.
pub fn check_plugin(path: &Path) -> CheckResult {
    let mut diagnostics = Vec::new();
    let path = path.to_path_buf();

    // Read the file.
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                line: 0,
                col: 0,
                message: format!("cannot read file: {e}"),
            });
            return CheckResult { path, diagnostics };
        }
    };

    // 1. Validate header metadata.
    check_header(&source, &path, &mut diagnostics);

    // 2. Parse the plugin type from header (needed for lifecycle checks).
    let meta = crate::manager::parse_plugin_header_from_source(&source, &path);

    // 3. Load into Lua with stub APIs and check for errors.
    check_lua(&source, &path, &meta, &mut diagnostics);

    CheckResult { path, diagnostics }
}

// ---------------------------------------------------------------------------
// Header validation
// ---------------------------------------------------------------------------

fn check_header(source: &str, path: &Path, diags: &mut Vec<Diagnostic>) {
    let mut has_name = false;
    let mut has_description = false;
    let mut line_num = 0;

    for line in source.lines() {
        line_num += 1;
        let trimmed = line.trim();
        if !trimmed.starts_with("--") {
            break;
        }
        let comment = trimmed.trim_start_matches('-').trim();

        if let Some(val) = comment.strip_prefix("plugin-name:") {
            has_name = true;
            if val.trim().is_empty() {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    line: line_num,
                    col: 0,
                    message: "plugin-name is empty".into(),
                });
            }
        } else if let Some(val) = comment.strip_prefix("plugin-description:") {
            has_description = true;
            if val.trim().is_empty() {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    line: line_num,
                    col: 0,
                    message: "plugin-description is empty".into(),
                });
            }
        } else if let Some(val) = comment.strip_prefix("plugin-type:") {
            let t = val.trim();
            if t != "panel" && t != "action" {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    line: line_num,
                    col: 0,
                    message: format!("unknown plugin-type '{t}' (expected 'panel' or 'action')"),
                });
            }
        } else if let Some(val) = comment.strip_prefix("plugin-keybind:") {
            if !val.contains('=') {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    line: line_num,
                    col: 0,
                    message: "plugin-keybind must use format: action = binding [| description]".into(),
                });
            }
        } else if let Some(val) = comment.strip_prefix("plugin-icon:") {
            let icon_str = val.trim();
            if !icon_str.is_empty() {
                let icon_path = if std::path::Path::new(icon_str).is_absolute() {
                    std::path::PathBuf::from(icon_str)
                } else {
                    path.parent().unwrap_or(Path::new(".")).join(icon_str)
                };
                if !icon_path.exists() {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        line: line_num,
                        col: 0,
                        message: format!("plugin-icon file not found: {}", icon_path.display()),
                    });
                }
            }
        } else if let Some(val) = comment.strip_prefix("plugin-version:") {
            let v = val.trim();
            // Simple semver check.
            let parts: Vec<&str> = v.split('.').collect();
            if parts.len() < 2 || parts.iter().any(|p| p.parse::<u32>().is_err()) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    line: line_num,
                    col: 0,
                    message: format!("plugin-version '{v}' is not a valid version number"),
                });
            }
        }
    }

    if !has_name {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            line: 0,
            col: 0,
            message: "missing plugin-name header comment".into(),
        });
    }
    if !has_description {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            line: 0,
            col: 0,
            message: "missing plugin-description header comment".into(),
        });
    }
}

// ---------------------------------------------------------------------------
// Lua validation
// ---------------------------------------------------------------------------

/// Known API functions: (table, function, min_args, max_args).
const API_FUNCTIONS: &[(&str, &str, u8, u8)] = &[
    // session
    ("session", "exec",      1, 1),
    ("session", "send",      1, 1),
    ("session", "run",       1, 1),
    ("session", "platform",  0, 0),
    ("session", "current",   0, 0),
    ("session", "all",       0, 0),
    ("session", "named",     1, 1),
    // app
    ("app", "open_session",    1, 1),
    ("app", "clipboard",       1, 1),
    ("app", "notify",          1, 1),
    ("app", "log",             1, 1),
    ("app", "servers",         0, 0),
    ("app", "server_details",  0, 0),
    ("app", "set_icon",        1, 1),
    ("app", "register_keybind", 2, 3),
    // ui - output
    ("ui", "append",          1, 1),
    ("ui", "clear",           0, 0),
    // ui - dialogs
    ("ui", "form",            2, 2),
    ("ui", "prompt",          1, 1),
    ("ui", "confirm",         1, 1),
    ("ui", "alert",           2, 2),
    ("ui", "error",           2, 2),
    ("ui", "show",            2, 2),
    ("ui", "table",           3, 3),
    ("ui", "progress",        1, 1),
    ("ui", "hide_progress",   0, 0),
    // ui - panel widgets
    ("ui", "panel_clear",     0, 0),
    ("ui", "panel_heading",   1, 1),
    ("ui", "panel_text",      1, 1),
    ("ui", "panel_label",     1, 1),
    ("ui", "panel_separator", 0, 0),
    ("ui", "panel_table",     2, 2),
    ("ui", "panel_progress",  3, 3),
    ("ui", "panel_button",    2, 2),
    ("ui", "panel_kv",        2, 2),
    ("ui", "set_refresh",     1, 1),
    // crypto
    ("crypto", "encrypt",     3, 3),
    ("crypto", "decrypt",     3, 3),
    ("crypto", "algorithms",  0, 0),
    // net
    ("net", "check_port",  2, 3),
    ("net", "scan",        2, 4),
    ("net", "scan_range",  3, 5),
    ("net", "resolve",     1, 1),
    ("net", "time",        0, 0),
];

fn check_lua(source: &str, path: &Path, meta: &Option<PluginMeta>, diags: &mut Vec<Diagnostic>) {
    let lua = Lua::new();

    // Sandbox: remove dangerous modules.
    let _ = lua.globals().set("os", LuaValue::Nil);
    let _ = lua.globals().set("io", LuaValue::Nil);
    let _ = lua.globals().set("loadfile", LuaValue::Nil);
    let _ = lua.globals().set("dofile", LuaValue::Nil);

    // Register stub API tables.
    if let Err(e) = register_stub_apis(&lua) {
        diags.push(Diagnostic {
            severity: Severity::Error,
            line: 0,
            col: 0,
            message: format!("internal error setting up checker: {e}"),
        });
        return;
    }

    // Load and execute the script.
    let chunk_name = path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    match lua.load(source).set_name(&chunk_name).exec() {
        Ok(()) => {}
        Err(e) => {
            let (line, msg) = parse_lua_error(&e);
            diags.push(Diagnostic {
                severity: Severity::Error,
                line,
                col: 0,
                message: msg,
            });
            // If load failed, we can't check lifecycle functions.
            return;
        }
    }

    // Check lifecycle functions for panel plugins.
    if let Some(meta) = meta {
        if matches!(meta.plugin_type, PluginType::Panel | PluginType::BottomPanel | PluginType::SessionPanel) {
            check_panel_lifecycle(&lua, diags);
        }
    }

    // Check for common mistakes: defining main() instead of setup/render.
    if lua.globals().get::<LuaFunction>("main").is_ok() {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            line: 0,
            col: 0,
            message: "function 'main()' defined but never called; did you mean 'setup()' or 'render()'?".into(),
        });
    }

    // Invoke lifecycle functions to validate API calls inside them.
    for func_name in &["setup", "render", "on_click", "on_keybind"] {
        if let Ok(func) = lua.globals().get::<LuaFunction>(*func_name) {
            if let Err(e) = func.call::<()>(()) {
                let (line, msg) = parse_lua_error(&e);
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    line,
                    col: 0,
                    message: format!("in {func_name}(): {msg}"),
                });
            }
        }
    }
}

fn register_stub_apis(lua: &Lua) -> LuaResult<()> {
    // Build a lookup set for fast checking.
    let mut tables: std::collections::HashMap<&str, Vec<(&str, u8, u8)>> =
        std::collections::HashMap::new();
    for &(table, func, min, max) in API_FUNCTIONS {
        tables.entry(table).or_default().push((func, min, max));
    }

    for (table_name, funcs) in &tables {
        let tbl = lua.create_table()?;
        for &(func_name, min_args, max_args) in funcs {
            let tname = table_name.to_string();
            let fname = func_name.to_string();
            let f = lua.create_function(
                move |_lua, args: LuaMultiValue| {
                    let count = args.len() as u8;
                    if count < min_args || count > max_args {
                        let expected = if min_args == max_args {
                            format!("{min_args}")
                        } else {
                            format!("{min_args}-{max_args}")
                        };
                        return Err(LuaError::RuntimeError(format!(
                            "{tname}.{fname}() expects {expected} argument(s), got {count}"
                        )));
                    }
                    // Return nil for all stub functions (safe default).
                    Ok(LuaValue::Nil)
                },
            )?;
            tbl.set(func_name, f)?;
        }

        // Add a metatable that catches calls to unknown functions.
        let meta_tbl = lua.create_table()?;
        let valid_names: Vec<String> = funcs.iter().map(|(n, _, _)| n.to_string()).collect();
        let tname = table_name.to_string();
        meta_tbl.set(
            "__index",
            lua.create_function(move |_lua, (_tbl, key): (LuaValue, String)| {
                Err::<LuaValue, _>(LuaError::RuntimeError(format!(
                    "'{tname}.{key}()' is not a valid API function; available: {}",
                    valid_names.join(", ")
                )))
            })?,
        )?;
        tbl.set_metatable(Some(meta_tbl));

        lua.globals().set(*table_name, tbl)?;
    }

    Ok(())
}

fn check_panel_lifecycle(lua: &Lua, diags: &mut Vec<Diagnostic>) {
    let has_setup = lua.globals().get::<LuaFunction>("setup").is_ok();
    let has_render = lua.globals().get::<LuaFunction>("render").is_ok();
    let has_on_click = lua.globals().get::<LuaFunction>("on_click").is_ok();
    let has_on_keybind = lua.globals().get::<LuaFunction>("on_keybind").is_ok();

    if !has_setup && !has_render {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            line: 0,
            col: 0,
            message: "panel plugin defines neither setup() nor render(); it won't display anything".into(),
        });
    }

    // These are fine to be missing, but note them at info level? Skip for now.
    let _ = (has_on_click, has_on_keybind);
}

/// Parse a Lua error into (line_number, message).
fn parse_lua_error(err: &LuaError) -> (usize, String) {
    let full = err.to_string();
    // Strip stack traceback if present, but try to extract line from it first.
    let (msg, traceback_line) = if let Some(tb_pos) = full.find("\nstack traceback:") {
        let tb = &full[tb_pos..];
        // Look for [string "..."]:LINE in the traceback.
        let line = extract_line_from_bracket_pattern(tb);
        (full[..tb_pos].trim(), line)
    } else {
        (full.trim(), 0)
    };
    // Lua errors often look like: "[string \"name\"]:42: message"
    // or "name:42: message"
    if let Some(colon_pos) = msg.find("]:") {
        let after = &msg[colon_pos + 2..];
        if let Some(next_colon) = after.find(':') {
            if let Ok(line) = after[..next_colon].trim().parse::<usize>() {
                let detail = after[next_colon + 1..].trim().to_string();
                return (line, strip_runtime_prefix(&detail));
            }
        }
    }
    // Fallback: try "filename:line: msg" pattern.
    let parts: Vec<&str> = msg.splitn(3, ':').collect();
    if parts.len() >= 3 {
        if let Ok(line) = parts[1].trim().parse::<usize>() {
            let detail = parts[2..].join(":").trim().to_string();
            return (line, strip_runtime_prefix(&detail));
        }
    }
    (traceback_line, strip_runtime_prefix(msg))
}

/// Extract a line number from a `[string "..."]:LINE` pattern in text.
fn extract_line_from_bracket_pattern(text: &str) -> usize {
    // Look for ]:NUMBER:
    for part in text.split("]:") {
        let num_str = part.split(':').next().unwrap_or("").trim();
        if let Ok(line) = num_str.parse::<usize>() {
            return line;
        }
    }
    0
}

/// Strip the "runtime error: " prefix that mlua adds.
fn strip_runtime_prefix(msg: &str) -> String {
    msg.strip_prefix("runtime error: ")
        .unwrap_or(msg)
        .to_string()
}
