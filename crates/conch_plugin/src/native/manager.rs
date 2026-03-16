//! Native plugin manager — discovery, load/unload, bounded thread pool.
//!
//! [`NativePluginManager`] ties together the library loader, the message bus,
//! and the plugin lifecycle. It enforces a bounded maximum on concurrent plugins
//! and provides the high-level API that `conch_app` uses to manage plugins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::oneshot;

use crate::bus::{PluginBus, PluginMail};

use super::library::{discover_library_paths, PluginLibrary};
use super::lifecycle::{self, LoadedPlugin};
use super::{DiscoveredPlugin, LoadError, PluginMeta};

/// Default maximum number of concurrently loaded native plugins.
const DEFAULT_MAX_PLUGINS: usize = 16;


/// Manages native plugin discovery, loading, and lifecycle.
pub struct NativePluginManager {
    /// Directories to scan for plugin shared libraries.
    search_paths: Vec<PathBuf>,
    /// Currently loaded and running plugins, keyed by plugin name.
    plugins: HashMap<String, LoadedPlugin>,
    /// Shared message bus.
    bus: Arc<PluginBus>,
    /// Pointer to the host API vtable (stable address, outlives all plugins).
    host_api: *const conch_plugin_sdk::HostApi,
    /// Boxed HostApi to ensure the pointer remains valid.
    _host_api_box: Box<conch_plugin_sdk::HostApi>,
    /// Maximum number of plugins that can be loaded simultaneously.
    max_plugins: usize,
}

// SAFETY: The host_api raw pointer is derived from _host_api_box which we own.
// All plugin interaction goes through channels (Send) and the bus (Sync).
unsafe impl Send for NativePluginManager {}

impl NativePluginManager {
    /// Create a new manager.
    ///
    /// `host_api` is the vtable of host function implementations. It is boxed
    /// internally to ensure a stable pointer for the lifetime of the manager.
    pub fn new(bus: Arc<PluginBus>, host_api: conch_plugin_sdk::HostApi) -> Self {
        let boxed = Box::new(host_api);
        let ptr: *const conch_plugin_sdk::HostApi = &*boxed;
        Self {
            search_paths: Vec::new(),
            plugins: HashMap::new(),
            bus,
            host_api: ptr,
            _host_api_box: boxed,
            max_plugins: DEFAULT_MAX_PLUGINS,
        }
    }

    /// Set the maximum number of concurrently loaded plugins.
    pub fn set_max_plugins(&mut self, max: usize) {
        self.max_plugins = max;
    }

    /// Add a directory to scan for plugin libraries.
    pub fn add_search_path(&mut self, path: PathBuf) {
        if !self.search_paths.contains(&path) {
            self.search_paths.push(path);
        }
    }

    /// Scan all search paths and return metadata for each discovered plugin.
    ///
    /// This temporarily loads each library to read its `conch_plugin_info()`,
    /// then unloads it. Plugins are not activated.
    pub fn discover(&self) -> Vec<DiscoveredPlugin> {
        let mut discovered = Vec::new();
        for dir in &self.search_paths {
            let paths = match discover_library_paths(dir) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("failed to scan {}: {e}", dir.display());
                    continue;
                }
            };

            for path in paths {
                match unsafe { PluginLibrary::load(&path) } {
                    Ok(lib) => {
                        let meta = unsafe { lib.read_info() };
                        discovered.push(DiscoveredPlugin {
                            path,
                            meta,
                        });
                        // Library is dropped here, unloading the .dylib/.so.
                    }
                    Err(e) => {
                        log::warn!("failed to probe {}: {e}", path.display());
                    }
                }
            }
        }
        discovered
    }

    /// Load and activate a plugin from the given shared library path.
    ///
    /// The plugin is:
    /// 1. Opened and symbols resolved.
    /// 2. Registered on the message bus (mailbox created).
    /// 3. Started on a new OS thread (setup + event loop).
    pub fn load_plugin(&mut self, path: &Path) -> Result<PluginMeta, LoadError> {
        if self.plugins.len() >= self.max_plugins {
            return Err(LoadError::MaxPluginsReached {
                limit: self.max_plugins,
            });
        }

        // Load library and read metadata.
        let library = unsafe { PluginLibrary::load(path)? };
        let meta = unsafe { library.read_info() };
        let name = meta.name.clone();

        if self.plugins.contains_key(&name) {
            return Err(LoadError::AlreadyLoaded(name));
        }

        // Register on the bus and get the mailbox receiver.
        let mailbox_rx = self.bus.register_plugin(&name);
        let sender = self.bus.sender_for(&name).unwrap();

        // Spawn the plugin thread.
        // Cast to usize so it's Send; the pointer is valid for the lifetime of
        // NativePluginManager (enforced by shutdown_all in Drop).
        let host_api_addr = self.host_api as usize;
        let thread_name = name.clone();
        let thread_plugin_name = name.clone();
        let handle = std::thread::Builder::new()
            .name(format!("plugin:{thread_name}"))
            .spawn(move || {
                let api = host_api_addr as *const conch_plugin_sdk::HostApi;
                // SAFETY: host_api is valid for the lifetime of NativePluginManager,
                // and the manager ensures shutdown before drop.
                unsafe {
                    lifecycle::plugin_thread(
                        library,
                        api,
                        mailbox_rx,
                        thread_plugin_name,
                    );
                }
            })
            .map_err(|e| LoadError::Io(e))?;

        self.plugins.insert(
            name,
            LoadedPlugin {
                meta: meta.clone(),
                sender,
                thread_handle: Some(handle),
            },
        );

        log::info!("loaded plugin: {} v{}", meta.name, meta.version);
        Ok(meta)
    }

    /// Unload a plugin: send shutdown, join thread, remove from bus.
    pub fn unload_plugin(&mut self, name: &str) -> Result<(), LoadError> {
        let mut plugin = self
            .plugins
            .remove(name)
            .ok_or_else(|| LoadError::NotLoaded(name.to_string()))?;

        // Send shutdown signal.
        if plugin.sender.try_send(PluginMail::Shutdown).is_err() {
            log::warn!("plugin [{name}]: failed to send shutdown (channel closed)");
        }

        // Wait for the thread to exit.
        plugin.join();

        // Remove from bus.
        self.bus.unregister_plugin(name);

        log::info!("unloaded plugin: {name}");
        Ok(())
    }

    /// Request a render from a loaded panel plugin.
    ///
    /// Returns the JSON widget tree string, or an error if the plugin is not
    /// loaded or the channel is closed.
    pub fn request_render(&self, name: &str) -> Result<String, LoadError> {
        let plugin = self
            .plugins
            .get(name)
            .ok_or_else(|| LoadError::NotLoaded(name.to_string()))?;

        let (tx, rx) = oneshot::channel();
        plugin
            .sender
            .try_send(PluginMail::RenderRequest { reply: tx })
            .map_err(|_| LoadError::ChannelClosed)?;

        rx.blocking_recv().map_err(|_| LoadError::ChannelClosed)
    }

    /// Send an event to a specific loaded plugin.
    pub fn send_event(&self, name: &str, event_json: &str) -> Result<(), LoadError> {
        let plugin = self
            .plugins
            .get(name)
            .ok_or_else(|| LoadError::NotLoaded(name.to_string()))?;

        let data: serde_json::Value =
            serde_json::from_str(event_json).unwrap_or(serde_json::Value::Null);

        let msg = crate::bus::BusMessage {
            source: "host".to_string(),
            event_type: "host.direct".to_string(),
            data,
        };

        plugin
            .sender
            .try_send(PluginMail::BusEvent(msg))
            .map_err(|_| LoadError::ChannelClosed)
    }

    /// List all currently loaded plugins.
    pub fn loaded_plugins(&self) -> Vec<&PluginMeta> {
        self.plugins.values().map(|p| &p.meta).collect()
    }

    /// Get the host API pointer (for sharing with Lua plugin runner).
    pub fn host_api_ptr(&self) -> *const conch_plugin_sdk::HostApi {
        self.host_api
    }

    /// Check if a plugin is loaded.
    pub fn is_loaded(&self, name: &str) -> bool {
        self.plugins.contains_key(name)
    }

    /// Number of currently loaded plugins.
    pub fn loaded_count(&self) -> usize {
        self.plugins.len()
    }

    /// Gracefully shut down all loaded plugins.
    pub fn shutdown_all(&mut self) {
        let names: Vec<String> = self.plugins.keys().cloned().collect();
        for name in names {
            if let Err(e) = self.unload_plugin(&name) {
                log::error!("failed to unload plugin {name}: {e}");
            }
        }
    }
}

impl Drop for NativePluginManager {
    fn drop(&mut self) {
        if !self.plugins.is_empty() {
            log::warn!(
                "NativePluginManager dropped with {} plugins still loaded, shutting down",
                self.plugins.len()
            );
            self.shutdown_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::PluginBus;
    use std::ffi::c_char;

    /// Create a dummy HostApi with stub function pointers for testing.
    ///
    /// None of these functions will actually be called in unit tests — they
    /// exist only to construct a valid HostApi struct.
    fn dummy_host_api() -> conch_plugin_sdk::HostApi {
        use conch_plugin_sdk::*;
        use conch_plugin_sdk::sftp::{SftpHandle, SftpVtable};
        use std::ffi::c_void;

        extern "C" fn stub_register_panel(
            _: PanelLocation,
            _: *const c_char,
            _: *const c_char,
        ) -> PanelHandle {
            PanelHandle(0)
        }
        extern "C" fn stub_set_widgets(_: PanelHandle, _: *const c_char, _: usize) {}
        extern "C" fn stub_open_session(
            _: *const SessionMeta,
            _: *const SessionBackendVtable,
            _: *mut c_void,
        ) -> OpenSessionResult {
            OpenSessionResult {
                handle: SessionHandle(0),
                output_cb: stub_output_cb,
                output_ctx: std::ptr::null_mut(),
            }
        }
        extern "C" fn stub_output_cb(_: *mut c_void, _: *const u8, _: usize) {}
        extern "C" fn stub_close_session(_: SessionHandle) {}
        extern "C" fn stub_set_session_status(
            _: SessionHandle,
            _: conch_plugin_sdk::SessionStatus,
            _: *const c_char,
        ) {
        }
        extern "C" fn stub_show_form(_: *const c_char, _: usize) -> *mut c_char {
            std::ptr::null_mut()
        }
        extern "C" fn stub_show_confirm(_: *const c_char) -> bool {
            false
        }
        extern "C" fn stub_show_prompt(
            _: *const c_char,
            _: *const c_char,
        ) -> *mut c_char {
            std::ptr::null_mut()
        }
        extern "C" fn stub_show_alert(_: *const c_char, _: *const c_char) {}
        extern "C" fn stub_show_error(_: *const c_char, _: *const c_char) {}
        extern "C" fn stub_notify(_: *const c_char, _: usize) {}
        extern "C" fn stub_log(_: u8, _: *const c_char) {}
        extern "C" fn stub_publish_event(
            _: *const c_char,
            _: *const c_char,
            _: usize,
        ) {
        }
        extern "C" fn stub_subscribe(_: *const c_char) {}
        extern "C" fn stub_query_plugin(
            _: *const c_char,
            _: *const c_char,
            _: *const c_char,
            _: usize,
        ) -> *mut c_char {
            std::ptr::null_mut()
        }
        extern "C" fn stub_register_service(_: *const c_char) {}
        extern "C" fn stub_get_config(_: *const c_char) -> *mut c_char {
            std::ptr::null_mut()
        }
        extern "C" fn stub_set_config(_: *const c_char, _: *const c_char) {}
        extern "C" fn stub_register_menu_item(
            _: *const c_char,
            _: *const c_char,
            _: *const c_char,
            _: *const c_char,
        ) {
        }
        extern "C" fn stub_clipboard_set(_: *const c_char) {}
        extern "C" fn stub_clipboard_get() -> *mut c_char {
            std::ptr::null_mut()
        }
        extern "C" fn stub_get_theme() -> *mut c_char {
            std::ptr::null_mut()
        }
        extern "C" fn stub_show_context_menu(
            _: *const c_char,
            _: usize,
        ) -> *mut c_char {
            std::ptr::null_mut()
        }
        extern "C" fn stub_session_prompt(
            _: SessionHandle,
            _: u8,
            _: *const c_char,
            _: *const c_char,
        ) -> *mut c_char {
            std::ptr::null_mut()
        }
        extern "C" fn stub_free_string(_: *mut c_char) {}
        extern "C" fn stub_set_status(_: *const c_char, _: u8, _: f32) {}
        extern "C" fn stub_register_sftp(_: u64, _: *const SftpVtable, _: *mut c_void) {}
        extern "C" fn stub_acquire_sftp(_: u64) -> SftpHandle {
            SftpHandle { vtable: std::ptr::null(), ctx: std::ptr::null_mut() }
        }
        extern "C" fn stub_write_to_pty(_: *const u8, _: usize) {}
        extern "C" fn stub_new_tab(_: *const c_char, _: bool) {}

        HostApi {
            register_panel: stub_register_panel,
            set_widgets: stub_set_widgets,
            open_session: stub_open_session,
            close_session: stub_close_session,
            set_session_status: stub_set_session_status,
            show_form: stub_show_form,
            show_confirm: stub_show_confirm,
            show_prompt: stub_show_prompt,
            show_alert: stub_show_alert,
            show_error: stub_show_error,
            notify: stub_notify,
            log: stub_log,
            publish_event: stub_publish_event,
            subscribe: stub_subscribe,
            query_plugin: stub_query_plugin,
            register_service: stub_register_service,
            get_config: stub_get_config,
            set_config: stub_set_config,
            register_menu_item: stub_register_menu_item,
            clipboard_set: stub_clipboard_set,
            clipboard_get: stub_clipboard_get,
            get_theme: stub_get_theme,
            show_context_menu: stub_show_context_menu,
            session_prompt: stub_session_prompt,
            free_string: stub_free_string,
            set_status: stub_set_status,
            register_sftp: stub_register_sftp,
            acquire_sftp: stub_acquire_sftp,
            write_to_pty: stub_write_to_pty,
            new_tab: stub_new_tab,
        }
    }

    #[test]
    fn new_manager_is_empty() {
        let bus = Arc::new(PluginBus::new());
        let mgr = NativePluginManager::new(bus, dummy_host_api());
        assert_eq!(mgr.loaded_count(), 0);
        assert!(mgr.loaded_plugins().is_empty());
    }

    #[test]
    fn add_search_path_deduplicates() {
        let bus = Arc::new(PluginBus::new());
        let mut mgr = NativePluginManager::new(bus, dummy_host_api());
        mgr.add_search_path("/tmp/plugins".into());
        mgr.add_search_path("/tmp/plugins".into());
        assert_eq!(mgr.search_paths.len(), 1);
    }

    #[test]
    fn set_max_plugins() {
        let bus = Arc::new(PluginBus::new());
        let mut mgr = NativePluginManager::new(bus, dummy_host_api());
        assert_eq!(mgr.max_plugins, DEFAULT_MAX_PLUGINS);
        mgr.set_max_plugins(4);
        assert_eq!(mgr.max_plugins, 4);
    }

    #[test]
    fn load_nonexistent_returns_error() {
        let bus = Arc::new(PluginBus::new());
        let mut mgr = NativePluginManager::new(bus, dummy_host_api());
        let result = mgr.load_plugin(Path::new("/nonexistent/plugin.dylib"));
        assert!(result.is_err());
    }

    #[test]
    fn unload_missing_returns_error() {
        let bus = Arc::new(PluginBus::new());
        let mut mgr = NativePluginManager::new(bus, dummy_host_api());
        let result = mgr.unload_plugin("nonexistent");
        assert!(matches!(result, Err(LoadError::NotLoaded(_))));
    }

    #[test]
    fn is_loaded_false_initially() {
        let bus = Arc::new(PluginBus::new());
        let mgr = NativePluginManager::new(bus, dummy_host_api());
        assert!(!mgr.is_loaded("ssh"));
    }

    #[test]
    fn discover_empty_search_paths() {
        let bus = Arc::new(PluginBus::new());
        let mgr = NativePluginManager::new(bus, dummy_host_api());
        let discovered = mgr.discover();
        assert!(discovered.is_empty());
    }

    #[test]
    fn render_missing_plugin_returns_error() {
        let bus = Arc::new(PluginBus::new());
        let mgr = NativePluginManager::new(bus, dummy_host_api());
        let result = mgr.request_render("nonexistent");
        assert!(matches!(result, Err(LoadError::NotLoaded(_))));
    }

    #[test]
    fn send_event_missing_plugin_returns_error() {
        let bus = Arc::new(PluginBus::new());
        let mgr = NativePluginManager::new(bus, dummy_host_api());
        let result = mgr.send_event("nonexistent", "{}");
        assert!(matches!(result, Err(LoadError::NotLoaded(_))));
    }
}
