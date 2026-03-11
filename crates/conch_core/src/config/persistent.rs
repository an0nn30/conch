//! Persistent UI state: window layout and zoom (machine-local, not user-edited).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistentState {
    pub layout: LayoutConfig,
}

impl Default for PersistentState {
    fn default() -> Self {
        Self { layout: LayoutConfig::default() }
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
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            window_width: 0.0,
            window_height: 0.0,
            zoom_factor: 1.0,
        }
    }
}
