//! Conch Plugin SDK — types for Lua and Java plugin authors.
//!
//! This crate defines the shared types between Conch and its plugins:
//!
//! - **Plugin metadata** (`PluginInfo`, `PluginType`, `PanelLocation`)
//! - **Widget types** (`Widget`, `WidgetEvent`) — declarative UI via JSON/serde
//! - **Event types** (`PluginEvent`) — widget interactions, menu actions, bus events

pub mod icons;
pub mod plugin_info;
pub mod session;
pub mod widgets;

// Re-export core types at crate root for convenience.
pub use plugin_info::*;
pub use session::*;
pub use widgets::{PluginEvent, Widget, WidgetEvent};
