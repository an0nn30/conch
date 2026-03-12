//! Conch-specific configuration: keyboard shortcuts and UI preferences.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConchConfig {
    pub keyboard: KeyboardConfig,
    pub ui: UiConfig,
    pub plugins: PluginsConfig,
}

impl Default for ConchConfig {
    fn default() -> Self {
        Self {
            keyboard: KeyboardConfig::default(),
            ui: UiConfig::default(),
            plugins: PluginsConfig::default(),
        }
    }
}

/// Plugin discovery configuration.
///
/// ```toml
/// [conch.plugins]
/// search_paths = ["~/.config/conch/plugins", "/usr/local/lib/conch/plugins"]
/// ```
///
/// If `search_paths` is empty (the default), the app uses built-in defaults:
/// - `~/.config/conch/plugins/`
/// - `target/debug/` and `target/release/` (development)
/// - `examples/plugins/` (development)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    /// Directories to scan for native (`.dylib`/`.so`/`.dll`) and Lua (`.lua`) plugins.
    pub search_paths: Vec<String>,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            search_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardConfig {
    pub new_tab: String,
    pub close_tab: String,
    pub quit: String,
    pub new_window: String,
    pub zen_mode: String,
    pub toggle_left_panel: String,
    pub toggle_right_panel: String,
    pub toggle_bottom_panel: String,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            new_tab: "cmd+t".into(),
            close_tab: "cmd+w".into(),
            quit: "cmd+q".into(),
            new_window: "cmd+shift+n".into(),
            zen_mode: "cmd+shift+z".into(),
            toggle_left_panel: "cmd+shift+e".into(),
            toggle_right_panel: "cmd+shift+r".into(),
            toggle_bottom_panel: "cmd+shift+j".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub font_family: String,
    pub font_size: f32,
    pub native_menu_bar: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_size: 13.0,
            native_menu_bar: true,
        }
    }
}
