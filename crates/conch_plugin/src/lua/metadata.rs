//! Lua plugin metadata parser — extracts `-- plugin-*` headers from `.lua` files.
//!
//! Lua plugins declare metadata in comment headers at the top of the file:
//!
//! ```lua
//! -- plugin-name: System Info
//! -- plugin-description: Live system information panel
//! -- plugin-type: tool_window
//! -- plugin-version: 1.0.0
//! -- plugin-location: left
//! -- plugin-icon: info.png
//! -- plugin-keybind: open_panel = cmd+shift+i | Toggle panel
//! ```

use conch_plugin_sdk::{PanelLocation, PluginType};

/// Metadata extracted from a Lua plugin's comment headers.
#[derive(Debug, Clone)]
pub struct LuaPluginMeta {
    pub name: String,
    pub description: String,
    pub version: String,
    pub api_required: Option<String>,
    pub permissions: Vec<String>,
    pub plugin_type: PluginType,
    pub panel_location: PanelLocation,
    pub icon: Option<String>,
    pub keybinds: Vec<LuaKeybind>,
}

/// A keybinding declared in the plugin header.
#[derive(Debug, Clone)]
pub struct LuaKeybind {
    pub action: String,
    pub binding: String,
    pub description: Option<String>,
}

impl Default for LuaPluginMeta {
    fn default() -> Self {
        Self {
            name: "Unknown".into(),
            description: String::new(),
            version: "0.0.0".into(),
            api_required: None,
            permissions: Vec::new(),
            plugin_type: PluginType::Action,
            panel_location: PanelLocation::None,
            icon: None,
            keybinds: Vec::new(),
        }
    }
}

/// Parse metadata from the comment headers of a Lua plugin source file.
///
/// Scans lines from the top of the file. Stops at the first non-comment,
/// non-empty line (i.e., the start of actual code).
pub fn parse_lua_metadata(source: &str) -> LuaPluginMeta {
    let mut meta = LuaPluginMeta::default();

    for line in source.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("--") {
            if !trimmed.is_empty() {
                break;
            }
            continue;
        }

        let comment = trimmed.trim_start_matches('-').trim();

        if let Some(value) = comment.strip_prefix("plugin-name:") {
            meta.name = value.trim().to_string();
        } else if let Some(value) = comment.strip_prefix("plugin-description:") {
            meta.description = value.trim().to_string();
        } else if let Some(value) = comment.strip_prefix("plugin-version:") {
            meta.version = value.trim().to_string();
        } else if let Some(value) = comment.strip_prefix("plugin-api:") {
            let req = value.trim();
            if !req.is_empty() {
                meta.api_required = Some(req.to_string());
            }
        } else if let Some(value) = comment.strip_prefix("plugin-permissions:") {
            meta.permissions = value
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(ToString::to_string)
                .collect();
        } else if let Some(value) = comment.strip_prefix("plugin-type:") {
            meta.plugin_type = match value.trim() {
                "tool_window" | "panel" => PluginType::ToolWindow,
                _ => PluginType::Action,
            };
            // Default zone for tool-window plugins.
            if matches!(meta.plugin_type, PluginType::ToolWindow)
                && matches!(meta.panel_location, PanelLocation::None)
            {
                meta.panel_location = PanelLocation::Left;
            }
        } else if let Some(value) = comment.strip_prefix("plugin-location:") {
            meta.panel_location = match value.trim() {
                "left" => PanelLocation::Left,
                "right" => PanelLocation::Right,
                "bottom" => PanelLocation::Bottom,
                _ => PanelLocation::None,
            };
        } else if let Some(value) = comment.strip_prefix("plugin-icon:") {
            meta.icon = Some(value.trim().to_string());
        } else if let Some(value) = comment.strip_prefix("plugin-keybind:") {
            if let Some(keybind) = parse_keybind(value) {
                meta.keybinds.push(keybind);
            }
        }
    }

    meta
}

/// Parse a keybind declaration: `action = binding | description`
fn parse_keybind(value: &str) -> Option<LuaKeybind> {
    let (action_part, rest) = value.split_once('=')?;
    let action = action_part.trim().to_string();
    let (binding, description) = if let Some((b, d)) = rest.split_once('|') {
        (b.trim().to_string(), Some(d.trim().to_string()))
    } else {
        (rest.trim().to_string(), None)
    };
    Some(LuaKeybind {
        action,
        binding,
        description,
    })
}

/// Discover `.lua` plugin files in a directory.
///
/// Returns `(path, source)` pairs for each `.lua` file found.
pub fn discover_lua_plugins(dir: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    let mut plugins = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return plugins,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "lua") {
            if let Ok(source) = std::fs::read_to_string(&path) {
                plugins.push((path, source));
            }
        }
    }
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_header() {
        let source = r#"-- plugin-name: System Info
-- plugin-description: Live system information panel
-- plugin-type: tool_window
-- plugin-version: 1.3.0
-- plugin-api: ^1.0
-- plugin-permissions: ui.panel, ui.menu
-- plugin-location: right
-- plugin-icon: system-info.png
-- plugin-keybind: open_panel = cmd+shift+i | Toggle System Info panel

function render()
    ui.panel_heading("System")
end
"#;
        let meta = parse_lua_metadata(source);
        assert_eq!(meta.name, "System Info");
        assert_eq!(meta.description, "Live system information panel");
        assert_eq!(meta.version, "1.3.0");
        assert_eq!(meta.api_required.as_deref(), Some("^1.0"));
        assert_eq!(meta.permissions, vec!["ui.panel", "ui.menu"]);
        assert!(matches!(meta.plugin_type, PluginType::ToolWindow));
        assert!(matches!(meta.panel_location, PanelLocation::Right));
        assert_eq!(meta.icon.as_deref(), Some("system-info.png"));
        assert_eq!(meta.keybinds.len(), 1);
        assert_eq!(meta.keybinds[0].action, "open_panel");
        assert_eq!(meta.keybinds[0].binding, "cmd+shift+i");
        assert_eq!(
            meta.keybinds[0].description.as_deref(),
            Some("Toggle System Info panel")
        );
    }

    #[test]
    fn parse_minimal_header() {
        let source = "-- plugin-name: Hello\nprint('hi')\n";
        let meta = parse_lua_metadata(source);
        assert_eq!(meta.name, "Hello");
        assert!(matches!(meta.plugin_type, PluginType::Action));
        assert!(matches!(meta.panel_location, PanelLocation::None));
    }

    #[test]
    fn parse_no_header() {
        let source = "print('no header')\n";
        let meta = parse_lua_metadata(source);
        assert_eq!(meta.name, "Unknown");
    }

    #[test]
    fn parse_panel_default_location() {
        let source = "-- plugin-type: tool_window\nfunction render() end\n";
        let meta = parse_lua_metadata(source);
        assert!(matches!(meta.plugin_type, PluginType::ToolWindow));
        // Tool-window plugins default to Left if no location specified.
        assert!(matches!(meta.panel_location, PanelLocation::Left));
    }

    #[test]
    fn parse_panel_explicit_bottom() {
        let source =
            "-- plugin-type: tool_window\n-- plugin-location: bottom\nfunction render() end\n";
        let meta = parse_lua_metadata(source);
        assert!(matches!(meta.panel_location, PanelLocation::Bottom));
    }

    #[test]
    fn parse_legacy_panel_type() {
        let source = "-- plugin-type: panel\nfunction render() end\n";
        let meta = parse_lua_metadata(source);
        assert!(
            matches!(meta.plugin_type, PluginType::ToolWindow),
            "legacy 'panel' header should map to ToolWindow"
        );
    }

    #[test]
    fn parse_multiple_keybinds() {
        let source = r#"-- plugin-name: Multi
-- plugin-keybind: run = cmd+r | Run plugin
-- plugin-keybind: toggle = cmd+t
function setup() end
"#;
        let meta = parse_lua_metadata(source);
        assert_eq!(meta.keybinds.len(), 2);
        assert_eq!(meta.keybinds[0].action, "run");
        assert!(meta.keybinds[0].description.is_some());
        assert_eq!(meta.keybinds[1].action, "toggle");
        assert!(meta.keybinds[1].description.is_none());
    }

    #[test]
    fn parse_stops_at_code() {
        let source = r#"-- plugin-name: Early
-- plugin-version: 1.0.0
local x = 1
-- plugin-description: This should be ignored
"#;
        let meta = parse_lua_metadata(source);
        assert_eq!(meta.name, "Early");
        assert_eq!(meta.version, "1.0.0");
        assert!(meta.description.is_empty());
    }

    #[test]
    fn parse_blank_lines_in_header() {
        let source = "-- plugin-name: Spaced\n\n-- plugin-version: 2.0.0\n\nfunction setup() end\n";
        let meta = parse_lua_metadata(source);
        assert_eq!(meta.name, "Spaced");
        assert_eq!(meta.version, "2.0.0");
    }

    #[test]
    fn parse_keybind_no_description() {
        let kb = parse_keybind(" open_panel = cmd+shift+p ").unwrap();
        assert_eq!(kb.action, "open_panel");
        assert_eq!(kb.binding, "cmd+shift+p");
        assert!(kb.description.is_none());
    }

    #[test]
    fn parse_keybind_with_description() {
        let kb = parse_keybind(" run = cmd+r | Run the plugin ").unwrap();
        assert_eq!(kb.action, "run");
        assert_eq!(kb.binding, "cmd+r");
        assert_eq!(kb.description.as_deref(), Some("Run the plugin"));
    }

    #[test]
    fn parse_keybind_invalid() {
        assert!(parse_keybind("no equals sign").is_none());
    }

    #[test]
    fn discover_in_nonexistent_dir() {
        let result = discover_lua_plugins(std::path::Path::new("/nonexistent/path"));
        assert!(result.is_empty());
    }
}
