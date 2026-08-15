//! TermLab Plugin SDK — types for Lua and Java plugin authors.
//!
//! This crate defines the shared types between TermLab and its plugins:
//!
//! - **Plugin metadata** (`PluginInfo`, `PluginType`, `PanelLocation`)
//! - **Widget types** (`Widget`, `WidgetEvent`) — declarative UI via JSON/serde
//! - **Event types** (`PluginEvent`) — widget interactions, menu actions, bus events

pub mod icons;
pub mod plugin_info;
pub mod session;
pub mod widgets;

/// Host plugin API major version.
///
/// Compatibility rule: plugins targeting the same major version are considered
/// compatible in Phase 1.
pub const HOST_PLUGIN_API_MAJOR: u64 = 1;

/// Host plugin API minor version.
pub const HOST_PLUGIN_API_MINOR: u64 = 0;

// Re-export core types at crate root for convenience.
pub use plugin_info::*;
pub use session::*;
pub use widgets::{PluginEvent, Widget, WidgetEvent};
