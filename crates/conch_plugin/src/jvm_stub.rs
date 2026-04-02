//! Stub JVM plugin module — used when the Java SDK JAR was not found at build time.

pub mod runtime {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use crate::HostApi;
    use crate::bus::PluginBus;

    /// Metadata about a discovered/loaded plugin.
    #[derive(Debug, Clone)]
    pub struct PluginMeta {
        pub name: String,
        pub description: String,
        pub version: String,
        pub plugin_type: conch_plugin_sdk::PluginType,
        pub panel_location: conch_plugin_sdk::PanelLocation,
    }

    /// Error type for plugin loading operations.
    #[derive(Debug)]
    pub enum LoadError {
        Io(std::io::Error),
        AlreadyLoaded(String),
        NotLoaded(String),
    }

    impl std::fmt::Display for LoadError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Io(e) => write!(f, "{e}"),
                Self::AlreadyLoaded(n) => write!(f, "plugin '{n}' already loaded"),
                Self::NotLoaded(n) => write!(f, "plugin '{n}' not loaded"),
            }
        }
    }

    pub struct JavaPluginManager {
        _private: (),
    }

    unsafe impl Send for JavaPluginManager {}

    impl JavaPluginManager {
        pub fn new(_bus: Arc<PluginBus>, _host_api: Arc<dyn HostApi>) -> Self {
            log::warn!(
                "JVM plugin support unavailable — binary was built without the Java SDK JAR. \
                 Build it with: make -C java-sdk build"
            );
            Self { _private: () }
        }

        pub fn probe_jar_name(&mut self, _jar_path: &Path) -> Option<String> {
            None
        }

        pub fn probe_jar_api_requirement(&self, _jar_path: &Path) -> Option<String> {
            None
        }

        pub fn probe_jar_permissions(&self, _jar_path: &Path) -> Vec<String> {
            Vec::new()
        }

        pub fn discover(&mut self, _dir: &Path) -> Vec<(PathBuf, PluginMeta)> {
            Vec::new()
        }

        pub fn load_plugin(&mut self, _jar_path: &Path) -> Result<PluginMeta, LoadError> {
            Err(LoadError::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "JVM plugin support was not compiled in (Java SDK JAR missing at build time)",
            )))
        }

        pub fn unload_plugin(&mut self, name: &str) -> Result<(), LoadError> {
            Err(LoadError::NotLoaded(name.to_string()))
        }

        pub fn loaded_plugins(&self) -> Vec<&PluginMeta> {
            Vec::new()
        }

        pub fn is_loaded(&self, _name: &str) -> bool {
            false
        }

        pub fn loaded_count(&self) -> usize {
            0
        }

        pub fn shutdown_all(&mut self) {}
    }
}
