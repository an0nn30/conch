//! Plugin integration for the Tauri UI.
//!
//! Discovers Lua plugins, spawns them with `TauriHostApi`, and exposes
//! Tauri commands for widget events and panel queries.

pub(crate) mod tauri_host_api;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use conch_plugin::bus::PluginBus;
use conch_plugin::jvm::runtime::JavaPluginManager;
use conch_plugin::lua::runner;
use parking_lot::Mutex;
use serde::Serialize;

use tauri_host_api::TauriHostApi;

/// Metadata for a registered plugin panel.
#[derive(Clone, Serialize)]
pub(crate) struct PanelInfo {
    pub plugin_name: String,
    pub panel_name: String,
    pub location: String,
    pub icon: Option<String>,
    pub widgets_json: String,
}

/// Shared plugin state accessible from Tauri commands.
pub(crate) struct PluginState {
    pub bus: Arc<PluginBus>,
    pub panels: Arc<Mutex<HashMap<u64, PanelInfo>>>,
    pub running_lua: Vec<runner::RunningLuaPlugin>,
    pub java_mgr: Option<JavaPluginManager>,
}

impl PluginState {
    pub fn new() -> Self {
        Self {
            bus: Arc::new(PluginBus::new()),
            panels: Arc::new(Mutex::new(HashMap::new())),
            running_lua: Vec::new(),
            java_mgr: None,
        }
    }

    /// Discover and start Lua plugins.
    pub fn start_lua_plugins(&mut self, app_handle: &tauri::AppHandle) {
        let search_paths = default_plugin_search_paths();

        for dir in &search_paths {
            if !dir.exists() {
                continue;
            }
            log::info!("Scanning for Lua plugins in {}", dir.display());

            let discovered = runner::discover(dir);
            for plugin in &discovered {
                log::info!(
                    "Found Lua plugin: {} ({})",
                    plugin.meta.name,
                    plugin.path.display()
                );

                let name = plugin.meta.name.clone();

                // Create a per-plugin HostApi instance.
                let host_api: Arc<dyn conch_plugin::HostApi> = Arc::new(TauriHostApi {
                    name: name.clone(),
                    app_handle: app_handle.clone(),
                    bus: Arc::clone(&self.bus),
                    panels: Arc::clone(&self.panels),
                });

                // Register on the bus and get the mailbox.
                let mailbox_rx = self.bus.register_plugin(&name);
                let mailbox_tx = match self.bus.sender_for(&name) {
                    Some(tx) => tx,
                    None => {
                        log::error!("Failed to get mailbox sender for plugin '{name}'");
                        continue;
                    }
                };

                match runner::spawn_lua_plugin(plugin, host_api, mailbox_tx, mailbox_rx) {
                    Ok(running) => {
                        log::info!("Lua plugin '{}' started", name);
                        self.running_lua.push(running);
                    }
                    Err(e) => {
                        log::error!("Failed to start Lua plugin '{}': {e}", name);
                    }
                }
            }
        }
    }

    /// Start the Java plugin manager and discover/load JAR plugins.
    pub fn start_java_plugins(&mut self, app_handle: &tauri::AppHandle) {
        let host_api: Arc<dyn conch_plugin::HostApi> = Arc::new(TauriHostApi {
            name: "java".to_string(),
            app_handle: app_handle.clone(),
            bus: Arc::clone(&self.bus),
            panels: Arc::clone(&self.panels),
        });

        let mut mgr = JavaPluginManager::new(Arc::clone(&self.bus), host_api);
        let search_paths = default_plugin_search_paths();

        for dir in &search_paths {
            if !dir.exists() {
                continue;
            }

            // Look for .jar files.
            let jar_files: Vec<_> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension().map_or(false, |ext| ext == "jar")
                })
                .map(|e| e.path())
                .collect();

            for jar_path in &jar_files {
                log::info!("Found JAR plugin: {}", jar_path.display());
                match mgr.load_plugin(jar_path) {
                    Ok(meta) => log::info!("Java plugin loaded: {} v{}", meta.name, meta.version),
                    Err(e) => log::error!("Failed to load JAR {}: {e}", jar_path.display()),
                }
            }
        }

        self.java_mgr = Some(mgr);
    }

    /// Shut down all running plugins.
    pub fn shutdown_all(&mut self) {
        for plugin in &self.running_lua {
            let _ = plugin.sender.blocking_send(conch_plugin::bus::PluginMail::Shutdown);
        }
    }
}

/// Default directories to search for plugins.
fn default_plugin_search_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();

    // User plugins dir.
    let config_dir = conch_core::config::config_dir();
    paths.push(config_dir.join("plugins"));

    // Development paths.
    paths.push(std::path::PathBuf::from("examples/plugins"));
    paths.push(std::path::PathBuf::from("target/debug"));
    paths.push(std::path::PathBuf::from("target/release"));

    paths
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Get all registered plugin panels.
#[tauri::command]
pub(crate) fn get_plugin_panels(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
) -> Vec<PanelInfo> {
    state.lock().panels.lock().values().cloned().collect()
}

/// Get the widget JSON for a specific panel.
#[tauri::command]
pub(crate) fn get_panel_widgets(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    handle: u64,
) -> Option<String> {
    state
        .lock()
        .panels
        .lock()
        .get(&handle)
        .map(|p| p.widgets_json.clone())
}

/// Send a widget event to a plugin.
#[tauri::command]
pub(crate) fn plugin_widget_event(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    plugin_name: String,
    event_json: String,
) {
    let bus = Arc::clone(&state.lock().bus);
    if let Some(sender) = bus.sender_for(&plugin_name) {
        let _ = sender.blocking_send(conch_plugin::bus::PluginMail::WidgetEvent {
            json: event_json,
        });
    }
}

/// Request a plugin to re-render its widgets.
#[tauri::command]
pub(crate) async fn request_plugin_render(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    plugin_name: String,
) -> Result<Option<String>, String> {
    let bus = {
        let s = state.lock();
        Arc::clone(&s.bus)
    };
    let sender = match bus.sender_for(&plugin_name) {
        Some(s) => s,
        None => return Ok(None),
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    sender
        .send(conch_plugin::bus::PluginMail::RenderRequest { reply: reply_tx })
        .await
        .map_err(|e| format!("send failed: {e}"))?;
    Ok(reply_rx.await.ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_state_new_is_empty() {
        let state = PluginState::new();
        assert!(state.panels.lock().is_empty());
        assert!(state.running_lua.is_empty());
    }

    #[test]
    fn default_search_paths_includes_user_dir() {
        let paths = default_plugin_search_paths();
        assert!(paths.iter().any(|p| p.to_string_lossy().contains("plugins")));
    }
}
