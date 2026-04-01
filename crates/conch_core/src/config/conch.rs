//! Conch-specific configuration: keyboard shortcuts and UI preferences.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConchConfig {
    pub keyboard: KeyboardConfig,
    pub ui: UiConfig,
    pub plugins: PluginsConfig,
    pub check_for_updates: bool,
}

impl Default for ConchConfig {
    fn default() -> Self {
        Self {
            keyboard: KeyboardConfig::default(),
            ui: UiConfig::default(),
            plugins: PluginsConfig::default(),
            check_for_updates: true,
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
    pub split_vertical: String,
    pub split_horizontal: String,
    pub close_pane: String,
    pub navigate_pane_up: String,
    pub navigate_pane_down: String,
    pub navigate_pane_left: String,
    pub navigate_pane_right: String,
    pub rename_tab: String,
    pub settings: String,
    pub plugin_shortcuts: HashMap<String, String>,
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
            split_vertical: "cmd+d".into(),
            split_horizontal: "cmd+shift+d".into(),
            close_pane: "cmd+shift+w".into(),
            navigate_pane_up: "cmd+alt+up".into(),
            navigate_pane_down: "cmd+alt+down".into(),
            navigate_pane_left: "cmd+alt+left".into(),
            navigate_pane_right: "cmd+alt+right".into(),
            rename_tab: "F2".into(),
            settings: "cmd+,".into(),
            plugin_shortcuts: HashMap::new(),
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
    /// Where toast notifications appear: "bottom" or "top".
    pub notification_position: String,
    /// Use native OS notifications when the app is not focused.
    pub native_notifications: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_size: 13.0,
            font: UiFontConfig::default(),
            native_menu_bar: true,
            notification_position: "bottom".into(),
            native_notifications: true,
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
        let cfg = PluginsConfig {
            enabled: false,
            ..Default::default()
        };
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
        assert_eq!(f.list, 14.0); // default
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

    #[test]
    fn notification_position_defaults_to_bottom() {
        let cfg = UiConfig::default();
        assert_eq!(cfg.notification_position, "bottom");
    }

    #[test]
    fn native_notifications_defaults_to_true() {
        let cfg = UiConfig::default();
        assert!(cfg.native_notifications);
    }

    #[test]
    fn ui_config_serde_default_fills_notification_fields() {
        let cfg: UiConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.notification_position, "bottom");
        assert!(cfg.native_notifications);
    }

    #[test]
    fn check_for_updates_defaults_to_true() {
        let config = ConchConfig::default();
        assert!(config.check_for_updates);
    }

    #[test]
    fn check_for_updates_survives_round_trip() {
        let mut config = ConchConfig::default();
        config.check_for_updates = false;
        let toml = toml::to_string(&config).unwrap();
        let parsed: ConchConfig = toml::from_str(&toml).unwrap();
        assert!(!parsed.check_for_updates);
    }

    #[test]
    fn check_for_updates_missing_defaults_true() {
        let parsed: ConchConfig = toml::from_str("").unwrap();
        assert!(parsed.check_for_updates);
    }

    #[test]
    fn keyboard_config_split_pane_defaults() {
        let cfg = KeyboardConfig::default();
        assert_eq!(cfg.split_vertical, "cmd+d");
        assert_eq!(cfg.split_horizontal, "cmd+shift+d");
        assert_eq!(cfg.close_pane, "cmd+shift+w");
        assert_eq!(cfg.navigate_pane_up, "cmd+alt+up");
        assert_eq!(cfg.navigate_pane_down, "cmd+alt+down");
        assert_eq!(cfg.navigate_pane_left, "cmd+alt+left");
        assert_eq!(cfg.navigate_pane_right, "cmd+alt+right");
    }

    #[test]
    fn keyboard_config_serde_default_fills_split_pane_fields() {
        let toml_str = r#"new_tab = "cmd+t""#;
        let cfg: KeyboardConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.split_vertical, "cmd+d");
        assert_eq!(cfg.close_pane, "cmd+shift+w");
    }
}
