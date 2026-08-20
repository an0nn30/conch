//! Persistent UI state: window layout and zoom (machine-local, not user-edited).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistentState {
    pub layout: LayoutConfig,
    /// Names of plugins that were loaded when the app last exited.
    pub loaded_plugins: Vec<String>,
    /// Measured window geometry from the last run, so the next launch can
    /// open at the exact size for the configured columns x lines instead of
    /// opening at a guess and visibly correcting itself.
    pub window_metrics: WindowMetrics,
    /// The file chooser window's size from its last close (logical px).
    /// `None` means "never closed one yet" — the chooser opens at its floor
    /// size instead. Written on every chooser close, read at open, and
    /// clamped to the floor and the parent's monitor work area there (see
    /// `termlab_tauri::chooser_window`).
    pub chooser_window: Option<ChooserWindowSize>,
}

impl Default for PersistentState {
    fn default() -> Self {
        Self {
            layout: LayoutConfig::default(),
            loaded_plugins: Vec::new(),
            window_metrics: WindowMetrics::default(),
            chooser_window: None,
        }
    }
}

/// The file chooser window's persisted size, in logical pixels. No position
/// is persisted — the chooser is always re-centered on its parent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChooserWindowSize {
    pub width: f64,
    pub height: f64,
}

/// A popped-out tool window's last known position and size, in logical
/// pixels. Unlike the chooser (which is always re-centered), a panel host
/// keeps whatever bounds the user left it at, so both position AND size are
/// persisted here — see `termlab_tauri::panel_host`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowBoundsRecord {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The two numbers that turn "columns x lines" into window pixels: how big a
/// terminal cell really is (depends on the font, which only the webview can
/// measure), and how much of the window is not terminal (titlebar, tab bar,
/// status bar, panels at their startup state). Written by the frontend the
/// first time its size correction converges; zeros mean "never measured".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WindowMetrics {
    /// Logical pixels per terminal column.
    pub cell_width: f32,
    /// Logical pixels per terminal row.
    pub cell_height: f32,
    /// Window logical width minus (columns x cell_width).
    pub chrome_width: f32,
    /// Window logical height minus (lines x cell_height).
    pub chrome_height: f32,
    /// The font family these cells were measured under. A measurement from a
    /// different font is worse than none: it converts columns to pixels with
    /// the wrong cell and opens every window visibly wrong.
    pub font_family: String,
    /// The font size the measurement was taken at.
    pub font_size: f32,
    /// The UI zoom factor at measurement time (zoom rescales CSS pixels, so
    /// it changes what a "cell" is in window-logical terms).
    pub zoom: f32,
}

impl WindowMetrics {
    /// Usable means every component was actually measured. Chrome may
    /// legitimately be small but never negative-large garbage; cells must be
    /// positive or the multiplication is meaningless.
    pub fn is_usable(&self) -> bool {
        self.cell_width > 0.0
            && self.cell_height > 0.0
            && self.chrome_width >= 0.0
            && self.chrome_height >= 0.0
    }

    /// A measurement only applies to the configuration it was taken under.
    /// Files written before fingerprinting existed have an empty family and
    /// therefore never match — they get remeasured, not trusted.
    pub fn matches(&self, family: &str, size: f32, zoom: f32) -> bool {
        self.font_family == family
            && (self.font_size - size).abs() < f32::EPSILON
            && (self.zoom - zoom).abs() < 0.001
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
    /// Whether zen mode is active.
    pub zen_mode: bool,
    /// Tool window zone assignments: window-id → zone-name.
    pub tool_window_zones: HashMap<String, String>,
    /// Active tool window by zone-name.
    pub active_tool_windows: HashMap<String, String>,
    /// Left sidebar top/bottom split ratio (0.0–1.0, top portion).
    pub left_split_ratio: f32,
    /// Right sidebar top/bottom split ratio (0.0–1.0, top portion).
    pub right_split_ratio: f32,
    /// Popped-out tool windows' last known bounds, keyed by tool-window id
    /// ALONE — deliberately not `(parent_label, tool_window_id)`. Parent
    /// labels (`window-2`, `window-3`, ...) are assigned by launch order and
    /// are not stable across restarts, so a composite key would silently
    /// orphan its record every time a window closed and a later session
    /// opened windows in a different order — an ever-growing pile of dead
    /// entries the id-only key structurally cannot accumulate. The traded-off
    /// behavior: if the SAME tool window is popped out from two different
    /// main windows at once, they remain fully independent live windows, but
    /// they share one REMEMBERED bounds record — last save wins, and closing
    /// either one overwrites what the other will reopen at. Absent keys
    /// (including the whole map, for state files written before this field
    /// existed) mean "never popped out" — the panel host opens at its
    /// default size, centered on the parent (see
    /// `termlab_tauri::panel_host::persist_tool_window_bounds`, the save
    /// site this same trade is documented at).
    pub tool_window_bounds: HashMap<String, WindowBoundsRecord>,
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
            zen_mode: false,
            tool_window_zones: HashMap::new(),
            active_tool_windows: HashMap::new(),
            left_split_ratio: 0.5,
            right_split_ratio: 0.5,
            tool_window_bounds: HashMap::new(),
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
        let mut active = HashMap::new();
        active.insert("right-top".into(), "ssh-sessions".into());
        active.insert("left-top".into(), "file-explorer".into());

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
                zen_mode: true,
                tool_window_zones: zones,
                active_tool_windows: active,
                left_split_ratio: 0.6,
                right_split_ratio: 0.4,
                tool_window_bounds: HashMap::new(),
            },
            loaded_plugins: vec!["ssh-manager".into(), "git-status".into()],
            window_metrics: WindowMetrics {
                cell_width: 8.4,
                cell_height: 20.0,
                chrome_width: 4.0,
                chrome_height: 44.0,
                font_family: "JetBrains Mono".into(),
                font_size: 14.0,
                zoom: 1.0,
            },
            chooser_window: Some(ChooserWindowSize {
                width: 900.0,
                height: 600.0,
            }),
        };
        let toml_str = toml::to_string(&original).expect("serialize");
        let restored: PersistentState = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(restored.window_metrics, original.window_metrics);
        assert!(restored.window_metrics.matches("JetBrains Mono", 14.0, 1.0));
        assert_eq!(restored.chooser_window, original.chooser_window);
        assert!(!restored.window_metrics.matches("JetBrains Mono", 16.0, 1.0));

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
        assert_eq!(
            restored.layout.active_tool_windows.get("right-top"),
            Some(&"ssh-sessions".to_string())
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
        assert!(ps.layout.active_tool_windows.is_empty());
        assert_eq!(ps.layout.left_split_ratio, 0.5);
        assert_eq!(ps.layout.right_split_ratio, 0.5);
    }

    #[test]
    fn chooser_window_size_round_trips_when_present() {
        let original = PersistentState {
            chooser_window: Some(ChooserWindowSize {
                width: 800.0,
                height: 500.0,
            }),
            ..PersistentState::default()
        };
        let toml_str = toml::to_string(&original).expect("serialize");
        let restored: PersistentState = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(
            restored.chooser_window,
            Some(ChooserWindowSize {
                width: 800.0,
                height: 500.0,
            })
        );
    }

    #[test]
    fn chooser_window_size_defaults_to_none_when_key_absent() {
        // Backward compat: every state.toml written before this field existed
        // has no [chooser_window] table at all.
        let toml_str = r#"
loaded_plugins = ["my-plugin"]
"#;
        let ps: PersistentState = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(ps.chooser_window, None);
    }

    #[test]
    fn tool_window_bounds_round_trips_toml() {
        let mut bounds = HashMap::new();
        bounds.insert(
            "ssh-sessions".to_string(),
            WindowBoundsRecord {
                x: 120.0,
                y: 80.0,
                width: 640.0,
                height: 480.0,
            },
        );
        let original = PersistentState {
            layout: LayoutConfig {
                tool_window_bounds: bounds,
                ..LayoutConfig::default()
            },
            ..PersistentState::default()
        };
        let toml_str = toml::to_string(&original).expect("serialize");
        let restored: PersistentState = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(
            restored.layout.tool_window_bounds.get("ssh-sessions"),
            Some(&WindowBoundsRecord {
                x: 120.0,
                y: 80.0,
                width: 640.0,
                height: 480.0,
            })
        );
    }

    #[test]
    fn tool_window_bounds_is_keyed_by_tool_window_id_alone_so_a_second_save_overwrites() {
        // Pins the deliberate trade documented on the field above: two main
        // windows popping the SAME tool-window id share one persisted
        // record. There is no parent-scoped key to preserve here — the
        // second save for an id is simply the new remembered bounds for
        // that id, full stop.
        let mut lc = LayoutConfig::default();
        lc.tool_window_bounds.insert(
            "ssh-sessions".to_string(),
            WindowBoundsRecord {
                x: 10.0,
                y: 20.0,
                width: 500.0,
                height: 350.0,
            },
        );
        // A later save for the same id, from any parent window at all —
        // the key carries no parent information to disambiguate.
        lc.tool_window_bounds.insert(
            "ssh-sessions".to_string(),
            WindowBoundsRecord {
                x: 99.0,
                y: 88.0,
                width: 700.0,
                height: 500.0,
            },
        );
        assert_eq!(
            lc.tool_window_bounds.get("ssh-sessions"),
            Some(&WindowBoundsRecord {
                x: 99.0,
                y: 88.0,
                width: 700.0,
                height: 500.0,
            }),
            "the second save wins outright — there is only ever one record per id"
        );
        assert_eq!(lc.tool_window_bounds.len(), 1);
    }

    #[test]
    fn tool_window_bounds_defaults_to_empty_map_when_key_absent() {
        // Backward compat: every state.toml written before this field existed
        // has no [layout.tool_window_bounds] table at all.
        let toml_str = r#"
loaded_plugins = ["my-plugin"]

[layout]
zoom_factor = 1.5
"#;
        let ps: PersistentState = toml::from_str(toml_str).expect("deserialize");
        assert!(ps.layout.tool_window_bounds.is_empty());
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
        assert!(!ps.layout.zen_mode, "default zen_mode");
    }
}
