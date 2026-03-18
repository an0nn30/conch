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
/// A menu item registered by a plugin.
#[derive(Clone, Serialize)]
pub(crate) struct PluginMenuItem {
    pub plugin: String,
    pub menu: String,
    pub label: String,
    pub action: String,
    pub keybind: Option<String>,
}

pub(crate) struct PluginState {
    pub bus: Arc<PluginBus>,
    pub panels: Arc<Mutex<HashMap<u64, PanelInfo>>>,
    pub menu_items: Arc<Mutex<Vec<PluginMenuItem>>>,
    pub running_lua: Vec<runner::RunningLuaPlugin>,
    pub java_mgr: Option<JavaPluginManager>,
    pub plugins_config: conch_core::config::PluginsConfig,
}

impl PluginState {
    pub fn new(plugins_config: conch_core::config::PluginsConfig) -> Self {
        Self {
            bus: Arc::new(PluginBus::new()),
            panels: Arc::new(Mutex::new(HashMap::new())),
            menu_items: Arc::new(Mutex::new(Vec::new())),
            running_lua: Vec::new(),
            java_mgr: None,
            plugins_config,
        }
    }

    fn search_paths(&self) -> Vec<std::path::PathBuf> {
        plugin_search_paths(&self.plugins_config.search_paths)
    }

    /// Initialize the Java plugin manager (JVM) without loading any plugins.
    /// Plugins are loaded on demand via the Plugin Manager UI.
    pub fn init_java_manager(&mut self, app_handle: &tauri::AppHandle) {
        let host_api: Arc<dyn conch_plugin::HostApi> = Arc::new(TauriHostApi {
            name: "java".to_string(),
            app_handle: app_handle.clone(),
            bus: Arc::clone(&self.bus),
            panels: Arc::clone(&self.panels),
            menu_items: Arc::clone(&self.menu_items),
        });

        self.java_mgr = Some(JavaPluginManager::new(Arc::clone(&self.bus), host_api));
        log::info!("Java plugin manager initialized (JVM ready, no plugins loaded)");
    }

    /// Shut down all running plugins.
    pub fn shutdown_all(&mut self) {
        for plugin in &self.running_lua {
            let _ = plugin.sender.blocking_send(conch_plugin::bus::PluginMail::Shutdown);
        }
    }
}

/// Build the plugin search paths from config + defaults.
fn plugin_search_paths(extra: &[String]) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();

    // Development paths.
    paths.push(std::path::PathBuf::from("target/debug"));
    paths.push(std::path::PathBuf::from("target/release"));

    // Exe directory and sibling paths (installed builds).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            paths.push(exe_dir.to_path_buf());
            paths.push(exe_dir.join("plugins"));
            // Linux: /opt/conch/lib/ or ../lib/ relative to bin.
            if let Some(parent) = exe_dir.parent() {
                paths.push(parent.join("lib"));
            }
        }
    }

    // User plugins dir (~/.config/conch/plugins/).
    let config_dir = conch_core::config::config_dir();
    paths.push(config_dir.join("plugins"));

    // User-configured extra search paths from [conch.plugins] search_paths.
    for p in extra {
        let expanded = if p.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(&p[2..])
            } else {
                std::path::PathBuf::from(p)
            }
        } else {
            std::path::PathBuf::from(p)
        };
        paths.push(expanded);
    }

    paths
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Plugin manager types
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
pub(crate) struct DiscoveredPlugin {
    pub name: String,
    pub description: String,
    pub version: String,
    pub plugin_type: String,
    pub source: String,  // "lua" or "java"
    pub path: String,
    pub loaded: bool,
}

// ---------------------------------------------------------------------------
// Plugin manager commands
// ---------------------------------------------------------------------------

/// Scan all search paths and return discovered plugins with their status.
#[tauri::command]
pub(crate) fn scan_plugins(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
) -> Vec<DiscoveredPlugin> {
    let ps = state.lock();
    let search_paths = ps.search_paths();
    let loaded_names: std::collections::HashSet<String> = ps
        .running_lua
        .iter()
        .map(|p| p.meta.name.clone())
        .collect();

    let mut result = Vec::new();

    // Discover Lua plugins
    for dir in &search_paths {
        if !dir.exists() { continue; }
        let discovered = runner::discover(dir);
        for plugin in &discovered {
            result.push(DiscoveredPlugin {
                name: plugin.meta.name.clone(),
                description: plugin.meta.description.clone(),
                version: plugin.meta.version.clone(),
                plugin_type: format!("{:?}", plugin.meta.plugin_type),
                source: "Lua".into(),
                path: plugin.path.to_string_lossy().to_string(),
                loaded: loaded_names.contains(&plugin.meta.name),
            });
        }
    }

    // Discover Java plugins (JAR files)
    for dir in &search_paths {
        if !dir.exists() { continue; }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "jar") {
                    let name = path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let loaded = ps.java_mgr.as_ref().map_or(false, |mgr| mgr.is_loaded(&name));
                    result.push(DiscoveredPlugin {
                        name: name.clone(),
                        description: String::new(),
                        version: String::new(),
                        plugin_type: "Unknown".into(),
                        source: "Java".into(),
                        path: path.to_string_lossy().to_string(),
                        loaded,
                    });
                }
            }
        }
    }

    result
}

/// Enable (load) a plugin by name and path.
#[tauri::command]
pub(crate) fn enable_plugin(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    name: String,
    source: String,
    path: String,
) -> Result<(), String> {
    let mut ps = state.lock();

    if source == "Lua" {
        // Find the discovered plugin by path.
        let discovered = runner::discover(std::path::Path::new(&path).parent().unwrap_or(std::path::Path::new(".")));
        let plugin = discovered.iter().find(|p| p.meta.name == name)
            .ok_or_else(|| format!("Plugin '{name}' not found at {path}"))?;

        let host_api: Arc<dyn conch_plugin::HostApi> = Arc::new(TauriHostApi {
            name: name.clone(),
            app_handle: app.clone(),
            bus: Arc::clone(&ps.bus),
            panels: Arc::clone(&ps.panels),
            menu_items: Arc::clone(&ps.menu_items),
        });

        let mailbox_rx = ps.bus.register_plugin(&name);
        let mailbox_tx = ps.bus.sender_for(&name)
            .ok_or_else(|| "Failed to get mailbox sender".to_string())?;

        let running = runner::spawn_lua_plugin(plugin, host_api, mailbox_tx, mailbox_rx)
            .map_err(|e| format!("Failed to start: {e}"))?;
        ps.running_lua.push(running);
        Ok(())
    } else if source == "Java" {
        if let Some(ref mut mgr) = ps.java_mgr {
            mgr.load_plugin(std::path::Path::new(&path))
                .map(|_| ())
                .map_err(|e| format!("Failed to load JAR: {e}"))
        } else {
            Err("Java plugin manager not initialized".into())
        }
    } else {
        Err(format!("Unknown plugin source: {source}"))
    }
}

/// Disable (unload) a plugin by name.
#[tauri::command]
pub(crate) fn disable_plugin(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    name: String,
    source: String,
) -> Result<(), String> {
    let mut ps = state.lock();

    if source == "Lua" {
        if let Some(idx) = ps.running_lua.iter().position(|p| p.meta.name == name) {
            let plugin = &ps.running_lua[idx];
            let _ = plugin.sender.blocking_send(conch_plugin::bus::PluginMail::Shutdown);
            ps.running_lua.remove(idx);
            ps.bus.unregister_plugin(&name);
            Ok(())
        } else {
            Err(format!("Plugin '{name}' is not running"))
        }
    } else if source == "Java" {
        if let Some(ref mut mgr) = ps.java_mgr {
            mgr.unload_plugin(&name).map_err(|e| format!("Failed to unload: {e}"))
        } else {
            Err("Java plugin manager not initialized".into())
        }
    } else {
        Err(format!("Unknown plugin source: {source}"))
    }
}

/// Get all menu items registered by plugins.
#[tauri::command]
pub(crate) fn get_plugin_menu_items(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
) -> Vec<PluginMenuItem> {
    state.lock().menu_items.lock().clone()
}

/// Trigger a plugin menu action (sends menu_action event to the plugin).
#[tauri::command]
pub(crate) fn trigger_plugin_menu_action(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    plugin_name: String,
    action: String,
) {
    let bus = Arc::clone(&state.lock().bus);
    if let Some(sender) = bus.sender_for(&plugin_name) {
        let event = conch_plugin_sdk::PluginEvent::MenuAction { action };
        let json = serde_json::to_string(&event).unwrap_or_default();
        let _ = sender.blocking_send(conch_plugin::bus::PluginMail::WidgetEvent { json });
    }
}

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
        let state = PluginState::new(conch_core::config::PluginsConfig::default());
        assert!(state.panels.lock().is_empty());
        assert!(state.running_lua.is_empty());
    }

    #[test]
    fn search_paths_includes_user_dir() {
        let paths = plugin_search_paths(&[]);
        assert!(paths.iter().any(|p| p.to_string_lossy().contains("plugins")));
    }

    #[test]
    fn search_paths_includes_custom_paths() {
        let paths = plugin_search_paths(&["/custom/plugins".to_string()]);
        assert!(paths.iter().any(|p| p.to_string_lossy() == "/custom/plugins"));
    }

    #[test]
    fn search_paths_expands_tilde() {
        let paths = plugin_search_paths(&["~/my-plugins".to_string()]);
        assert!(paths.iter().any(|p| !p.to_string_lossy().starts_with('~')));
    }
}
