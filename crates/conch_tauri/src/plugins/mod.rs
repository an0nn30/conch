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
use tauri::Emitter;

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
/// Pending dialog responses from the frontend.
pub(crate) struct PendingDialogs {
    pub forms: HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>,
    pub prompts: HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>,
    pub confirms: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
}

impl PendingDialogs {
    fn new() -> Self {
        Self {
            forms: HashMap::new(),
            prompts: HashMap::new(),
            confirms: HashMap::new(),
        }
    }

    /// Remove all pending dialog channels whose prompt_id belongs to the
    /// given plugin.  Prompt IDs use the format `"{plugin_name}\0{uuid}"`.
    /// We use a null byte separator because it cannot appear in plugin
    /// names (derived from filenames and Lua comment headers), avoiding
    /// collisions between plugins whose names share a common prefix.
    /// Dropping the oneshot senders causes the blocked plugin thread to
    /// receive `None` / `false`, which is the expected cancellation value.
    fn drain_for_plugin(&mut self, plugin_name: &str) {
        let prefix = format!("{plugin_name}\0");
        self.forms.retain(|id, _| !id.starts_with(&prefix));
        self.prompts.retain(|id, _| !id.starts_with(&prefix));
        self.confirms.retain(|id, _| !id.starts_with(&prefix));
    }
}

/// A menu item registered by a plugin.
#[derive(Clone, Serialize)]
pub(crate) struct PluginMenuItem {
    pub plugin: String,
    pub menu: String,
    pub label: String,
    pub action: String,
    pub keybind: Option<String>,
}

/// Payload emitted when all panels for a plugin are removed.
#[derive(Clone, Serialize)]
struct PluginPanelsRemoved {
    plugin: String,
    handles: Vec<u64>,
}

pub(crate) struct PluginState {
    pub bus: Arc<PluginBus>,
    pub panels: Arc<Mutex<HashMap<u64, PanelInfo>>>,
    pub menu_items: Arc<Mutex<Vec<PluginMenuItem>>>,
    pub pending_dialogs: Arc<Mutex<PendingDialogs>>,
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
            pending_dialogs: Arc::new(Mutex::new(PendingDialogs::new())),
            running_lua: Vec::new(),
            java_mgr: None,
            plugins_config,
        }
    }

    fn search_paths(&self) -> Vec<std::path::PathBuf> {
        plugin_search_paths(&self.plugins_config.search_paths)
    }

    /// Create a TauriHostApi instance for a plugin.
    fn make_host_api(
        &self,
        name: &str,
        app_handle: &tauri::AppHandle,
    ) -> Arc<dyn conch_plugin::HostApi> {
        Arc::new(TauriHostApi {
            name: name.to_string(),
            app_handle: app_handle.clone(),
            bus: Arc::clone(&self.bus),
            panels: Arc::clone(&self.panels),
            menu_items: Arc::clone(&self.menu_items),
            pending_dialogs: Arc::clone(&self.pending_dialogs),
        })
    }

    /// Get names of all currently loaded plugins.
    fn loaded_plugin_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .running_lua
            .iter()
            .map(|p| p.meta.name.clone())
            .collect();
        if let Some(ref mgr) = self.java_mgr {
            for meta in mgr.loaded_plugins() {
                names.push(meta.name.clone());
            }
        }
        names
    }

    /// Save the list of currently enabled plugins to state.toml.
    fn persist_enabled_plugins(&self) {
        let names = self.loaded_plugin_names();
        let mut state = conch_core::config::load_persistent_state().unwrap_or_default();
        state.loaded_plugins = names;
        let _ = conch_core::config::save_persistent_state(&state);
    }

    /// Remove panels, menu items, and pending dialogs belonging to a plugin.
    /// Returns the list of removed panel handles so callers can notify the
    /// frontend.
    fn cleanup_plugin_resources(&self, plugin_name: &str) -> Vec<u64> {
        // Collect and remove panels owned by this plugin.
        let mut removed_handles = Vec::new();
        self.panels.lock().retain(|handle, info| {
            if info.plugin_name == plugin_name {
                removed_handles.push(*handle);
                false
            } else {
                true
            }
        });

        // Remove menu items registered by this plugin.
        self.menu_items
            .lock()
            .retain(|item| item.plugin != plugin_name);

        // Drop pending dialog channels owned by this plugin.
        self.pending_dialogs.lock().drain_for_plugin(plugin_name);

        removed_handles
    }

    /// Auto-enable plugins that were enabled in the previous session.
    pub fn restore_plugins(&mut self, app_handle: &tauri::AppHandle) {
        let state = conch_core::config::load_persistent_state().unwrap_or_default();
        if state.loaded_plugins.is_empty() {
            return;
        }

        log::info!(
            "Restoring {} plugins from previous session",
            state.loaded_plugins.len()
        );

        // Scan for all available plugins.
        let search_paths = self.search_paths();
        let mut lua_plugins = Vec::new();
        let mut jar_paths: Vec<(String, std::path::PathBuf)> = Vec::new();

        for dir in &search_paths {
            if !dir.exists() {
                continue;
            }
            for p in runner::discover(dir) {
                lua_plugins.push(p);
            }
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "jar") {
                        let name = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        jar_paths.push((name, path));
                    }
                }
            }
        }

        for saved_name in &state.loaded_plugins {
            // Try Lua first.
            if let Some(plugin) = lua_plugins.iter().find(|p| &p.meta.name == saved_name) {
                let name = plugin.meta.name.clone();
                let host_api = self.make_host_api(&name, app_handle);
                let mailbox_rx = self.bus.register_plugin(&name);
                let Some(mailbox_tx) = self.bus.sender_for(&name) else {
                    continue;
                };
                match runner::spawn_lua_plugin(plugin, host_api, mailbox_tx, mailbox_rx) {
                    Ok(running) => {
                        log::info!("Restored Lua plugin '{name}'");
                        self.running_lua.push(running);
                    }
                    Err(e) => log::error!("Failed to restore Lua plugin '{name}': {e}"),
                }
                continue;
            }

            // Try Java — probe each JAR to match by plugin name since
            // JAR filenames don't necessarily match plugin display names.
            if let Some(ref mut mgr) = self.java_mgr {
                let mut found = false;
                for (_, jar_path) in &jar_paths {
                    // Probe the JAR to get its plugin name.
                    match mgr.probe_jar_name(jar_path) {
                        Some(probe_name) if probe_name == *saved_name => {
                            match mgr.load_plugin(jar_path) {
                                Ok(meta) => {
                                    log::info!(
                                        "Restored Java plugin '{}' v{}",
                                        meta.name,
                                        meta.version
                                    );
                                    found = true;
                                }
                                Err(e) => {
                                    log::error!("Failed to restore Java plugin '{saved_name}': {e}")
                                }
                            }
                            break;
                        }
                        _ => continue,
                    }
                }
                if found {
                    continue;
                }
            }

            log::warn!("Previously enabled plugin '{saved_name}' not found in search paths");
        }
    }

    /// Initialize the Java plugin manager (JVM) without loading any plugins.
    /// Plugins are loaded on demand via the Plugin Manager UI.
    pub fn init_java_manager(&mut self, app_handle: &tauri::AppHandle) {
        let host_api = self.make_host_api("java", app_handle);
        self.java_mgr = Some(JavaPluginManager::new(Arc::clone(&self.bus), host_api));
        log::info!("Java plugin manager initialized (JVM ready, no plugins loaded)");
    }

    /// Shut down all running plugins.
    pub fn shutdown_all(&mut self) {
        for plugin in &self.running_lua {
            let _ = plugin
                .sender
                .blocking_send(conch_plugin::bus::PluginMail::Shutdown);
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
    pub source: String, // "lua" or "java"
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
    let mut ps = state.lock();
    let search_paths = ps.search_paths();
    let loaded_names: std::collections::HashSet<String> =
        ps.running_lua.iter().map(|p| p.meta.name.clone()).collect();

    let mut result = Vec::new();

    // Discover Lua plugins
    for dir in &search_paths {
        if !dir.exists() {
            continue;
        }
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

    // Discover Java plugins (JAR files) — probe each to get real metadata.
    if let Some(ref mut mgr) = ps.java_mgr {
        for dir in &search_paths {
            if !dir.exists() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "jar") {
                        // Probe to get the actual plugin name from metadata.
                        let probe = mgr.probe_jar_name(&path);
                        let name = probe.unwrap_or_else(|| {
                            path.file_stem()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default()
                        });
                        let loaded = mgr.is_loaded(&name);
                        result.push(DiscoveredPlugin {
                            name,
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
        let discovered = runner::discover(
            std::path::Path::new(&path)
                .parent()
                .unwrap_or(std::path::Path::new(".")),
        );
        let plugin = discovered
            .iter()
            .find(|p| p.meta.name == name)
            .ok_or_else(|| format!("Plugin '{name}' not found at {path}"))?;

        let host_api = ps.make_host_api(&name, &app);

        let mailbox_rx = ps.bus.register_plugin(&name);
        let mailbox_tx = ps
            .bus
            .sender_for(&name)
            .ok_or_else(|| "Failed to get mailbox sender".to_string())?;

        let running = runner::spawn_lua_plugin(plugin, host_api, mailbox_tx, mailbox_rx)
            .map_err(|e| format!("Failed to start: {e}"))?;
        ps.running_lua.push(running);
        ps.persist_enabled_plugins();
        Ok(())
    } else if source == "Java" {
        if let Some(ref mut mgr) = ps.java_mgr {
            // Check if already loaded (e.g., restored from previous session).
            if mgr.is_loaded(&name) {
                return Ok(());
            }
            mgr.load_plugin(std::path::Path::new(&path))
                .map(|_| ())
                .map_err(|e| format!("Failed to load JAR: {e}"))?;
            ps.persist_enabled_plugins();
            Ok(())
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
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    name: String,
    source: String,
) -> Result<(), String> {
    let mut ps = state.lock();

    if source == "Lua" {
        if let Some(idx) = ps.running_lua.iter().position(|p| p.meta.name == name) {
            let plugin = &ps.running_lua[idx];
            let _ = plugin
                .sender
                .blocking_send(conch_plugin::bus::PluginMail::Shutdown);
            ps.running_lua.remove(idx);
            ps.bus.unregister_plugin(&name);

            let removed_handles = ps.cleanup_plugin_resources(&name);
            if !removed_handles.is_empty() {
                let _ = app.emit(
                    "plugin-panels-removed",
                    PluginPanelsRemoved {
                        plugin: name.clone(),
                        handles: removed_handles,
                    },
                );
            }

            ps.persist_enabled_plugins();
            Ok(())
        } else {
            Err(format!("Plugin '{name}' is not running"))
        }
    } else if source == "Java" {
        if let Some(ref mut mgr) = ps.java_mgr {
            mgr.unload_plugin(&name)
                .map_err(|e| format!("Failed to unload: {e}"))?;

            let removed_handles = ps.cleanup_plugin_resources(&name);
            if !removed_handles.is_empty() {
                let _ = app.emit(
                    "plugin-panels-removed",
                    PluginPanelsRemoved {
                        plugin: name.clone(),
                        handles: removed_handles,
                    },
                );
            }
            ps.persist_enabled_plugins();
            Ok(())
        } else {
            Err("Java plugin manager not initialized".into())
        }
    } else {
        Err(format!("Unknown plugin source: {source}"))
    }
}

// ---------------------------------------------------------------------------
// Dialog response commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn dialog_respond_form(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    prompt_id: String,
    result: Option<String>,
) {
    if let Some(tx) = state.lock().pending_dialogs.lock().forms.remove(&prompt_id) {
        let _ = tx.send(result);
    }
}

#[tauri::command]
pub(crate) fn dialog_respond_prompt(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    prompt_id: String,
    value: Option<String>,
) {
    if let Some(tx) = state
        .lock()
        .pending_dialogs
        .lock()
        .prompts
        .remove(&prompt_id)
    {
        let _ = tx.send(value);
    }
}

#[tauri::command]
pub(crate) fn dialog_respond_confirm(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    prompt_id: String,
    accepted: bool,
) {
    if let Some(tx) = state
        .lock()
        .pending_dialogs
        .lock()
        .confirms
        .remove(&prompt_id)
    {
        let _ = tx.send(accepted);
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
        let _ =
            sender.blocking_send(conch_plugin::bus::PluginMail::WidgetEvent { json: event_json });
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
        assert!(
            paths
                .iter()
                .any(|p| p.to_string_lossy().contains("plugins"))
        );
    }

    #[test]
    fn search_paths_includes_custom_paths() {
        let paths = plugin_search_paths(&["/custom/plugins".to_string()]);
        assert!(
            paths
                .iter()
                .any(|p| p.to_string_lossy() == "/custom/plugins")
        );
    }

    #[test]
    fn search_paths_expands_tilde() {
        let paths = plugin_search_paths(&["~/my-plugins".to_string()]);
        assert!(paths.iter().any(|p| !p.to_string_lossy().starts_with('~')));
    }

    #[test]
    fn cleanup_removes_panels_for_plugin() {
        let state = PluginState::new(conch_core::config::PluginsConfig::default());

        // Insert panels for two different plugins.
        {
            let mut panels = state.panels.lock();
            panels.insert(
                1,
                PanelInfo {
                    plugin_name: "my-plugin".into(),
                    panel_name: "Panel A".into(),
                    location: "left".into(),
                    icon: None,
                    widgets_json: "[]".into(),
                },
            );
            panels.insert(
                2,
                PanelInfo {
                    plugin_name: "other-plugin".into(),
                    panel_name: "Panel B".into(),
                    location: "right".into(),
                    icon: None,
                    widgets_json: "[]".into(),
                },
            );
            panels.insert(
                3,
                PanelInfo {
                    plugin_name: "my-plugin".into(),
                    panel_name: "Panel C".into(),
                    location: "bottom".into(),
                    icon: None,
                    widgets_json: "[]".into(),
                },
            );
        }

        let removed = state.cleanup_plugin_resources("my-plugin");

        assert_eq!(
            removed.len(),
            2,
            "should remove exactly 2 panels for my-plugin"
        );
        assert!(removed.contains(&1));
        assert!(removed.contains(&3));

        let panels = state.panels.lock();
        assert_eq!(panels.len(), 1, "only other-plugin panel should remain");
        assert!(panels.contains_key(&2));
    }

    #[test]
    fn cleanup_removes_menu_items_for_plugin() {
        let state = PluginState::new(conch_core::config::PluginsConfig::default());

        {
            let mut items = state.menu_items.lock();
            items.push(PluginMenuItem {
                plugin: "my-plugin".into(),
                menu: "Tools".into(),
                label: "Do Thing".into(),
                action: "do_thing".into(),
                keybind: None,
            });
            items.push(PluginMenuItem {
                plugin: "other-plugin".into(),
                menu: "Tools".into(),
                label: "Other".into(),
                action: "other".into(),
                keybind: None,
            });
            items.push(PluginMenuItem {
                plugin: "my-plugin".into(),
                menu: "View".into(),
                label: "Show".into(),
                action: "show".into(),
                keybind: Some("cmd+k".into()),
            });
        }

        state.cleanup_plugin_resources("my-plugin");

        let items = state.menu_items.lock();
        assert_eq!(items.len(), 1, "only other-plugin menu item should remain");
        assert_eq!(items[0].plugin, "other-plugin");
    }

    #[test]
    fn cleanup_drains_pending_dialogs_for_plugin() {
        let state = PluginState::new(conch_core::config::PluginsConfig::default());

        {
            let mut dialogs = state.pending_dialogs.lock();
            let (tx1, _rx1) = tokio::sync::oneshot::channel();
            dialogs.forms.insert("my-plugin\0uuid-1".into(), tx1);

            let (tx2, _rx2) = tokio::sync::oneshot::channel();
            dialogs.prompts.insert("my-plugin\0uuid-2".into(), tx2);

            let (tx3, _rx3) = tokio::sync::oneshot::channel();
            dialogs.confirms.insert("other-plugin\0uuid-3".into(), tx3);

            let (tx4, _rx4) = tokio::sync::oneshot::channel();
            dialogs.forms.insert("other-plugin\0uuid-4".into(), tx4);
        }

        state.cleanup_plugin_resources("my-plugin");

        let dialogs = state.pending_dialogs.lock();
        assert!(
            dialogs.forms.get("my-plugin\0uuid-1").is_none(),
            "form dialog for my-plugin should be removed"
        );
        assert!(
            dialogs.prompts.get("my-plugin\0uuid-2").is_none(),
            "prompt dialog for my-plugin should be removed"
        );
        assert!(
            dialogs.confirms.contains_key("other-plugin\0uuid-3"),
            "confirm dialog for other-plugin should remain"
        );
        assert!(
            dialogs.forms.contains_key("other-plugin\0uuid-4"),
            "form dialog for other-plugin should remain"
        );
    }

    #[test]
    fn drain_for_plugin_is_noop_when_no_matching_dialogs() {
        let mut dialogs = PendingDialogs::new();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        dialogs.forms.insert("other\0uuid-1".into(), tx);

        dialogs.drain_for_plugin("nonexistent");

        assert_eq!(
            dialogs.forms.len(),
            1,
            "should not remove unrelated dialogs"
        );
    }

    #[test]
    fn drain_for_plugin_does_not_collide_with_prefix_names() {
        let mut dialogs = PendingDialogs::new();
        let (tx1, _rx1) = tokio::sync::oneshot::channel();
        let (tx2, _rx2) = tokio::sync::oneshot::channel();
        // Plugin "a" and plugin "a:b" — disabling "a" must not drain "a:b"'s dialogs.
        dialogs.forms.insert("a\0uuid-1".into(), tx1);
        dialogs.forms.insert("a:b\0uuid-2".into(), tx2);

        dialogs.drain_for_plugin("a");

        assert_eq!(dialogs.forms.len(), 1, "should only remove plugin 'a'");
        assert!(
            dialogs.forms.contains_key("a:b\0uuid-2"),
            "plugin 'a:b' dialog should remain"
        );
    }

    #[test]
    fn cleanup_with_no_resources_returns_empty() {
        let state = PluginState::new(conch_core::config::PluginsConfig::default());
        let removed = state.cleanup_plugin_resources("nonexistent");
        assert!(
            removed.is_empty(),
            "should return empty vec when plugin has no resources"
        );
    }
}
