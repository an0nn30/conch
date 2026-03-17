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

/// Plugin discovery and loading configuration.
///
/// ```toml
/// [conch.plugins]
/// enabled = true              # Master switch — false disables all plugins
/// native = true               # Load native (.dylib/.so/.dll) plugins
/// lua = true                  # Load Lua (.lua) plugins
/// java = true                 # Load Java (.jar) plugins (starts a JVM)
/// search_paths = ["~/.config/conch/plugins"]
/// ```
///
/// Setting `enabled = false` skips the entire plugin engine: no bus, no
/// bridge, no discovery, no scanning.  The app runs as a lean terminal.
///
/// Setting individual type flags to `false` skips discovery and loading for
/// that type only.  For example, `java = false` means the JVM is never
/// started, but native and Lua plugins still work.
///
/// If `search_paths` is empty (the default), the app uses built-in defaults:
/// - `~/.config/conch/plugins/`
/// - `target/debug/` and `target/release/` (development)
/// - `examples/plugins/` (development)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    /// Master switch: `false` disables the entire plugin system.
    pub enabled: bool,
    /// Enable native (shared library) plugins.
    pub native: bool,
    /// Enable Lua plugins.
    pub lua: bool,
    /// Enable Java plugins (requires JVM).
    pub java: bool,
    /// Directories to scan for plugins.
    pub search_paths: Vec<String>,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            native: true,
            lua: true,
            java: true,
            search_paths: Vec::new(),
        }
    }
}

impl PluginsConfig {
    /// Whether any plugin type is enabled.
    pub fn any_enabled(&self) -> bool {
        self.enabled && (self.native || self.lua || self.java)
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

/// Font sizes for the UI chrome (panels, trees, tables, buttons).
///
/// ```toml
/// [conch.ui.font]
/// small = 12.0    # labels, tab titles, badges
/// list = 14.0     # tree nodes, table rows/headers
/// normal = 14.0   # body text, buttons, text inputs
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiFontConfig {
    pub small: f32,
    pub list: f32,
    pub normal: f32,
}

impl Default for UiFontConfig {
    fn default() -> Self {
        Self {
            small: 12.0,
            list: 14.0,
            normal: 14.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub font_family: String,
    pub font_size: f32,
    pub font: UiFontConfig,
    pub native_menu_bar: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_size: 13.0,
            font: UiFontConfig::default(),
            native_menu_bar: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_enabled_by_default() {
        let cfg = PluginsConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.native);
        assert!(cfg.lua);
        assert!(cfg.java);
        assert!(cfg.any_enabled());
    }

    #[test]
    fn plugins_disabled_master_switch() {
        let cfg = PluginsConfig { enabled: false, ..Default::default() };
        assert!(!cfg.any_enabled());
    }

    #[test]
    fn plugins_disabled_all_types() {
        let cfg = PluginsConfig {
            enabled: true,
            native: false,
            lua: false,
            java: false,
            ..Default::default()
        };
        assert!(!cfg.any_enabled());
    }

    #[test]
    fn plugins_one_type_enabled() {
        let cfg = PluginsConfig {
            enabled: true,
            native: false,
            lua: true,
            java: false,
            ..Default::default()
        };
        assert!(cfg.any_enabled());
    }

    #[test]
    fn plugins_config_from_toml() {
        let toml_str = r#"
            enabled = true
            native = true
            lua = false
            java = false
            search_paths = ["~/.config/conch/plugins"]
        "#;
        let cfg: PluginsConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.native);
        assert!(!cfg.lua);
        assert!(!cfg.java);
        assert_eq!(cfg.search_paths.len(), 1);
    }

    #[test]
    fn plugins_config_serde_default_fills_missing() {
        // Existing config.toml files won't have the new fields — serde(default) fills them.
        let toml_str = r#"search_paths = []"#;
        let cfg: PluginsConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.native);
        assert!(cfg.lua);
        assert!(cfg.java);
    }

    #[test]
    fn ui_font_config_defaults() {
        let f = UiFontConfig::default();
        assert_eq!(f.small, 12.0);
        assert_eq!(f.list, 14.0);
        assert_eq!(f.normal, 14.0);
    }

    #[test]
    fn ui_font_config_from_toml() {
        let toml_str = r#"
            small = 10.0
            list = 12.0
            normal = 13.0
        "#;
        let f: UiFontConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(f.small, 10.0);
        assert_eq!(f.list, 12.0);
        assert_eq!(f.normal, 13.0);
    }

    #[test]
    fn ui_font_config_serde_default_fills_missing() {
        let toml_str = r#"small = 10.0"#;
        let f: UiFontConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(f.small, 10.0);
        assert_eq!(f.list, 14.0);  // default
        assert_eq!(f.normal, 14.0); // default
    }

    #[test]
    fn ui_config_with_font_section() {
        let toml_str = r#"
            native_menu_bar = false
            [font]
            small = 11.0
            list = 13.0
            normal = 15.0
        "#;
        let cfg: UiConfig = toml::from_str(toml_str).unwrap();
        assert!(!cfg.native_menu_bar);
        assert_eq!(cfg.font.small, 11.0);
        assert_eq!(cfg.font.list, 13.0);
        assert_eq!(cfg.font.normal, 15.0);
    }

    #[test]
    fn ui_config_without_font_gets_defaults() {
        let toml_str = r#"native_menu_bar = true"#;
        let cfg: UiConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.font, UiFontConfig::default());
    }
}
