//! Host-side infrastructure for plugin integration.
//!
//! - [`bridge`] — HostApi vtable implementation for native plugins.
//! - [`panel_renderer`] — renders plugin widget trees to egui.
//! - [`session_bridge`] — bridges plugin byte-stream sessions to the terminal
//!   emulator via the VTE parser.
//! - [`dialogs`] — modal dialog rendering (form, prompt, confirm, alert, error).
//! - [`plugin_manager_ui`] — plugin discovery and load/unload UI.

pub mod bridge;
pub mod dialogs;
pub mod panel_renderer;
pub mod plugin_lifecycle;
pub mod plugin_manager_ui;
pub mod plugin_panels;
pub mod session_bridge;
