//! Persistent UI state: window layout and zoom (machine-local, not user-edited).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistentState {
    pub layout: LayoutConfig,
    /// Names of plugins that were loaded when the app last exited.
    pub loaded_plugins: Vec<String>,
}

impl Default for PersistentState {
    fn default() -> Self {
        Self {
            layout: LayoutConfig::default(),
            loaded_plugins: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    /// Persisted window width in logical points (0 = use config default).
    pub window_width: f32,
    /// Persisted window height in logical points (0 = use config default).
    pub window_height: f32,
    /// Persisted UI zoom factor (1.0 = default).
    pub zoom_factor: f32,
    /// Persisted left plugin panel width (0 = use default).
    pub left_panel_width: f32,
    /// Persisted right plugin panel width (0 = use default).
    pub right_panel_width: f32,
    /// Persisted bottom plugin panel height (0 = use default).
    pub bottom_panel_height: f32,
    /// Whether the left panel is visible.
    pub left_panel_visible: bool,
    /// Whether the right panel is visible.
    pub right_panel_visible: bool,
    /// Whether the bottom panel is visible.
    pub bottom_panel_visible: bool,
    /// Whether the status bar is visible.
    pub status_bar_visible: bool,
    /// Tool window zone assignments: window-id → zone-name.
    pub tool_window_zones: HashMap<String, String>,
    /// Left sidebar top/bottom split ratio (0.0–1.0, top portion).
    pub left_split_ratio: f32,
    /// Right sidebar top/bottom split ratio (0.0–1.0, top portion).
    pub right_split_ratio: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            window_width: 0.0,
            window_height: 0.0,
            zoom_factor: 1.0,
            left_panel_width: 0.0,
            right_panel_width: 0.0,
            bottom_panel_height: 0.0,
            left_panel_visible: true,
            right_panel_visible: true,
            bottom_panel_visible: true,
            status_bar_visible: true,
            tool_window_zones: HashMap::new(),
            left_split_ratio: 0.5,
            right_split_ratio: 0.5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_state_default() {
        let ps = PersistentState::default();
        assert!(
            ps.loaded_plugins.is_empty(),
            "loaded_plugins should be empty by default"
        );
        assert_eq!(ps.layout.zoom_factor, 1.0);
    }

    #[test]
    fn layout_config_default_panels_visible() {
        let lc = LayoutConfig::default();
        assert!(lc.left_panel_visible);
        assert!(lc.right_panel_visible);
        assert!(lc.bottom_panel_visible);
        assert!(lc.status_bar_visible);
    }

    #[test]
    fn layout_config_default_dimensions() {
        let lc = LayoutConfig::default();
        assert_eq!(lc.window_width, 0.0);
        assert_eq!(lc.window_height, 0.0);
        assert_eq!(lc.left_panel_width, 0.0);
        assert_eq!(lc.right_panel_width, 0.0);
        assert_eq!(lc.bottom_panel_height, 0.0);
    }

    #[test]
    fn persistent_state_serde_round_trip() {
        let mut zones = HashMap::new();
        zones.insert("ssh-sessions".into(), "right-top".into());
        zones.insert("file-explorer".into(), "left-top".into());

        let original = PersistentState {
            layout: LayoutConfig {
                window_width: 1280.0,
                window_height: 720.0,
                zoom_factor: 1.25,
                left_panel_width: 250.0,
                right_panel_width: 300.0,
                bottom_panel_height: 200.0,
                left_panel_visible: true,
                right_panel_visible: false,
                bottom_panel_visible: true,
                status_bar_visible: false,
                tool_window_zones: zones,
                left_split_ratio: 0.6,
                right_split_ratio: 0.4,
            },
            loaded_plugins: vec!["ssh-manager".into(), "git-status".into()],
        };
        let toml_str = toml::to_string(&original).expect("serialize");
        let restored: PersistentState = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(restored.layout.window_width, 1280.0);
        assert_eq!(restored.layout.window_height, 720.0);
        assert_eq!(restored.layout.zoom_factor, 1.25);
        assert_eq!(restored.layout.left_panel_width, 250.0);
        assert!(!restored.layout.right_panel_visible);
        assert!(!restored.layout.status_bar_visible);
        assert_eq!(restored.loaded_plugins.len(), 2);
        assert_eq!(restored.loaded_plugins[0], "ssh-manager");
        assert_eq!(restored.loaded_plugins[1], "git-status");
        assert_eq!(
            restored.layout.tool_window_zones.get("ssh-sessions"),
            Some(&"right-top".to_string())
        );
        assert_eq!(restored.layout.left_split_ratio, 0.6);
        assert_eq!(restored.layout.right_split_ratio, 0.4);
    }

    #[test]
    fn persistent_state_deserialize_empty_toml() {
        let ps: PersistentState = toml::from_str("").expect("deserialize empty");
        assert!(ps.loaded_plugins.is_empty());
        assert_eq!(ps.layout.zoom_factor, 1.0);
        assert!(ps.layout.left_panel_visible);
        assert!(ps.layout.status_bar_visible);
        assert!(ps.layout.tool_window_zones.is_empty());
        assert_eq!(ps.layout.left_split_ratio, 0.5);
        assert_eq!(ps.layout.right_split_ratio, 0.5);
    }

    #[test]
    fn persistent_state_deserialize_partial_toml() {
        let toml_str = r#"
loaded_plugins = ["my-plugin"]

[layout]
zoom_factor = 1.5
left_panel_visible = false
"#;
        let ps: PersistentState = toml::from_str(toml_str).expect("deserialize partial");
        assert_eq!(ps.loaded_plugins.len(), 1);
        assert_eq!(ps.loaded_plugins[0], "my-plugin");
        assert_eq!(ps.layout.zoom_factor, 1.5);
        assert!(!ps.layout.left_panel_visible);
        // Unset fields should get defaults
        assert_eq!(ps.layout.window_width, 0.0, "default window_width");
        assert_eq!(ps.layout.window_height, 0.0, "default window_height");
        assert!(ps.layout.right_panel_visible, "default right_panel_visible");
        assert!(
            ps.layout.bottom_panel_visible,
            "default bottom_panel_visible"
        );
        assert!(ps.layout.status_bar_visible, "default status_bar_visible");
    }
}
