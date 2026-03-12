//! Native plugin loader — discovers, loads, and manages shared-library plugins.
//!
//! # Architecture
//!
//! Each native plugin is a shared library (`.dylib`/`.so`/`.dll`) that exports
//! six well-known C symbols (see `conch_plugin_sdk::declare_plugin!`).
//!
//! On load, the manager:
//! 1. Opens the library and resolves all six symbols ([`library::PluginLibrary`]).
//! 2. Registers the plugin on the message bus.
//! 3. Spawns a dedicated OS thread that calls `setup`, then enters a command
//!    loop processing events, render requests, queries, and shutdown.
//!
//! Thread count is bounded by `max_plugins` on the manager.

pub mod library;
pub mod lifecycle;
pub mod manager;

pub use library::PluginLibrary;
pub use lifecycle::LoadedPlugin;
pub use manager::NativePluginManager;

use std::ffi::CStr;

use conch_plugin_sdk::{PanelLocation, PluginInfo, PluginType};

// ---------------------------------------------------------------------------
// Owned metadata (safe copy of the C ABI PluginInfo)
// ---------------------------------------------------------------------------

/// Owned version of [`conch_plugin_sdk::PluginInfo`].
///
/// All string data is copied into Rust `String`s so there is no dependency on
/// the loaded library's memory.
#[derive(Debug, Clone)]
pub struct PluginMeta {
    pub name: String,
    pub description: String,
    pub version: String,
    pub plugin_type: PluginType,
    pub panel_location: PanelLocation,
    pub dependencies: Vec<String>,
}

impl PluginMeta {
    /// Read string fields from a raw `PluginInfo` returned by `conch_plugin_info()`.
    ///
    /// # Safety
    ///
    /// All string pointers in `info` must be valid, null-terminated UTF-8.
    /// The `dependencies` array (if non-null) must contain `num_dependencies`
    /// valid pointers.
    pub unsafe fn from_raw(info: &PluginInfo) -> Self {
        let read_cstr = |p: *const std::ffi::c_char| -> String {
            if p.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(p) }
                    .to_string_lossy()
                    .into_owned()
            }
        };

        let mut deps = Vec::new();
        if !info.dependencies.is_null() {
            for i in 0..info.num_dependencies {
                let ptr = unsafe { *info.dependencies.add(i) };
                deps.push(read_cstr(ptr));
            }
        }

        Self {
            name: read_cstr(info.name),
            description: read_cstr(info.description),
            version: read_cstr(info.version),
            plugin_type: info.plugin_type,
            panel_location: info.panel_location,
            dependencies: deps,
        }
    }
}

/// A plugin discovered on disk but not yet loaded/activated.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub path: std::path::PathBuf,
    pub meta: PluginMeta,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum LoadError {
    /// `libloading` failed to open the library or resolve a symbol.
    Library(libloading::Error),
    /// A required symbol was not found.
    SymbolNotFound(&'static str),
    /// The maximum number of loaded plugins has been reached.
    MaxPluginsReached { limit: usize },
    /// A plugin with this name is already loaded.
    AlreadyLoaded(String),
    /// No loaded plugin with this name.
    NotLoaded(String),
    /// The plugin's mailbox channel is closed.
    ChannelClosed,
    /// I/O error during discovery.
    Io(std::io::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Library(e) => write!(f, "library error: {e}"),
            Self::SymbolNotFound(s) => write!(f, "symbol not found: {s}"),
            Self::MaxPluginsReached { limit } => {
                write!(f, "max plugins reached ({limit})")
            }
            Self::AlreadyLoaded(n) => write!(f, "plugin already loaded: {n}"),
            Self::NotLoaded(n) => write!(f, "plugin not loaded: {n}"),
            Self::ChannelClosed => write!(f, "plugin channel closed"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<libloading::Error> for LoadError {
    fn from(e: libloading::Error) -> Self {
        Self::Library(e)
    }
}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_error_display() {
        let err = LoadError::MaxPluginsReached { limit: 8 };
        assert_eq!(err.to_string(), "max plugins reached (8)");

        let err = LoadError::AlreadyLoaded("ssh".into());
        assert_eq!(err.to_string(), "plugin already loaded: ssh");

        let err = LoadError::NotLoaded("ssh".into());
        assert_eq!(err.to_string(), "plugin not loaded: ssh");

        let err = LoadError::SymbolNotFound("conch_plugin_info");
        assert_eq!(err.to_string(), "symbol not found: conch_plugin_info");
    }

    #[test]
    fn discovered_plugin_is_clone() {
        let dp = DiscoveredPlugin {
            path: "/tmp/test.dylib".into(),
            meta: PluginMeta {
                name: "test".into(),
                description: "A test".into(),
                version: "0.1.0".into(),
                plugin_type: PluginType::Panel,
                panel_location: PanelLocation::Left,
                dependencies: vec![],
            },
        };
        let dp2 = dp.clone();
        assert_eq!(dp2.meta.name, "test");
    }
}
