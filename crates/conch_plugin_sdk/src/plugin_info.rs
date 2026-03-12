//! Plugin metadata types.

use std::ffi::c_char;

/// The kind of plugin.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PluginType {
    /// A run-once action (menu item, keybinding trigger).
    Action = 0,
    /// A persistent panel that renders widgets.
    Panel = 1,
}

/// Where a panel plugin wants to be placed by default.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PanelLocation {
    /// No panel (action plugins, or runtime-registered panels).
    None = 0,
    Left = 1,
    Right = 2,
    Bottom = 3,
}

/// Static metadata returned by `conch_plugin_info()`.
///
/// All string pointers must be valid for the lifetime of the loaded library
/// (typically `'static` string literals).
#[repr(C)]
pub struct PluginInfo {
    /// Human-readable plugin name (e.g., "SSH Manager").
    pub name: *const c_char,
    /// Short description.
    pub description: *const c_char,
    /// Semver version string (e.g., "1.0.0").
    pub version: *const c_char,
    /// Plugin kind.
    pub plugin_type: PluginType,
    /// Default panel location (ignored for Action plugins).
    pub panel_location: PanelLocation,
    /// Null-terminated array of dependency plugin names.
    /// E.g., an SFTP plugin depends on "ssh".
    /// Set to null if no dependencies.
    pub dependencies: *const *const c_char,
    /// Number of entries in `dependencies`.
    pub num_dependencies: usize,
}

// SAFETY: PluginInfo contains only raw pointers to static data and Copy types.
// It is constructed on the plugin side and read on the host side, never shared
// across threads simultaneously.
unsafe impl Send for PluginInfo {}
unsafe impl Sync for PluginInfo {}
