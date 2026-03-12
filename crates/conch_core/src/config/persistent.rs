//! Persistent UI state: window layout and zoom (machine-local, not user-edited).

use serde::{Deserialize, Serialize};

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
        }
    }
}
