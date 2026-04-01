//! Plugin integration for the Tauri UI.
//!
//! Discovers Lua plugins, spawns them with `TauriHostApi`, and exposes
//! Tauri commands for widget events and panel queries.

mod permission_host_api;
pub(crate) mod tauri_host_api;

use std::collections::HashMap;
use std::sync::Arc;

use conch_plugin::bus::PluginBus;
use conch_plugin::jvm::runtime::JavaPluginManager;
use conch_plugin::lua::runner;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tauri::Emitter;
use ts_rs::TS;

use permission_host_api::{PermissionCheckedHostApi, PermissionProfile};
use tauri_host_api::TauriHostApi;

const HOST_PLUGIN_API_MAJOR: u64 = conch_plugin_sdk::HOST_PLUGIN_API_MAJOR;
const HOST_PLUGIN_API_MINOR: u64 = conch_plugin_sdk::HOST_PLUGIN_API_MINOR;

fn supported_capability_set() -> &'static [&'static str] {
    &[
        "ui.menu",
        "ui.panel",
        "ui.dock",
        "ui.notify",
        "ui.dialog",
        "clipboard.read",
        "clipboard.write",
        "config.read",
        "config.write",
        "bus.publish",
        "bus.subscribe",
        "bus.query",
        "session.write",
        "session.new_tab",
        "session.exec",
        "session.open",
        "session.close",
        "session.status",
        "net.resolve",
        "net.scan",
    ]
}

fn normalize_and_validate_permissions(
    plugin_name: &str,
    declared: &[String],
) -> Result<Vec<String>, String> {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    let supported = supported_capability_set();
    let mut out = Vec::new();
    for raw in declared {
        let cap = raw.trim().to_ascii_lowercase();
        if cap.is_empty() {
            continue;
        }
        if !supported.contains(&cap.as_str()) {
            return Err(format!(
                "Plugin '{}' declares unknown capability '{}'",
                plugin_name, cap
            ));
        }
        if seen.insert(cap.clone()) {
            out.push(cap);
        }
    }
    Ok(out)
}

fn ensure_plugin_api_compatible(plugin_name: &str, required: Option<&str>) -> Result<(), String> {
    let Some(req) = required.map(str::trim).filter(|r| !r.is_empty()) else {
        // Legacy plugin with no declared API requirement: allow in Phase 1.
        return Ok(());
    };
    if api_requirement_matches(req) {
        return Ok(());
    }
    Err(format!(
        "Plugin '{}' requires plugin API '{}' but host is {}.{}",
        plugin_name, req, HOST_PLUGIN_API_MAJOR, HOST_PLUGIN_API_MINOR
    ))
}

fn api_requirement_matches(req: &str) -> bool {
    // Phase 1 supports:
    // - caret major range: ^1 or ^1.0
    // - exact major/minor: 1 / 1.0 / 1.0.0
    if let Some(rest) = req.strip_prefix('^') {
        if let Some((major, _minor)) = parse_major_minor(rest) {
            return major == HOST_PLUGIN_API_MAJOR;
        }
        return false;
    }
    if let Some((major, minor)) = parse_major_minor(req) {
        return major == HOST_PLUGIN_API_MAJOR && minor == HOST_PLUGIN_API_MINOR;
    }
    false
}

fn parse_major_minor(s: &str) -> Option<(u64, u64)> {
    let mut parts = s.trim().split('.');
    let major = parts.next()?.trim().parse::<u64>().ok()?;
    let minor = parts
        .next()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    Some((major, minor))
}

/// Metadata for a registered plugin panel.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub(crate) struct PanelInfo {
    pub plugin_name: String,
    pub panel_name: String,
    pub location: String,
    pub icon: Option<String>,
    pub widgets_json: String,
}

/// Metadata for a plugin-owned docked split view.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub(crate) struct PluginViewInfo {
    pub view_id: String,
    pub plugin_name: String,
    pub pane_id: u32,
    pub tab_id: u32,
    pub title: String,
    pub icon: Option<String>,
    pub last_widgets_json: String,
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
#[derive(Clone, Serialize, TS)]
#[ts(export)]
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

#[derive(Clone, Serialize)]
struct PluginViewsRemoved {
    plugin: String,
    view_ids: Vec<String>,
}

pub(crate) struct PluginState {
    pub bus: Arc<PluginBus>,
    pub panels: Arc<RwLock<HashMap<u64, PanelInfo>>>,
    pub views_by_id: Arc<RwLock<HashMap<String, PluginViewInfo>>>,
    pub pane_to_view: Arc<RwLock<HashMap<u32, String>>>,
    pub menu_items: Arc<RwLock<Vec<PluginMenuItem>>>,
    pub pending_dialogs: Arc<Mutex<PendingDialogs>>,
    pub permission_profiles: Arc<RwLock<HashMap<String, PermissionProfile>>>,
    pub running_lua: Vec<runner::RunningLuaPlugin>,
    pub java_mgr: Option<JavaPluginManager>,
    pub plugins_config: conch_core::config::PluginsConfig,
}

impl PluginState {
    pub fn new(plugins_config: conch_core::config::PluginsConfig) -> Self {
        Self {
            bus: Arc::new(PluginBus::new()),
            panels: Arc::new(RwLock::new(HashMap::new())),
            views_by_id: Arc::new(RwLock::new(HashMap::new())),
            pane_to_view: Arc::new(RwLock::new(HashMap::new())),
            menu_items: Arc::new(RwLock::new(Vec::new())),
            pending_dialogs: Arc::new(Mutex::new(PendingDialogs::new())),
            permission_profiles: Arc::new(RwLock::new(HashMap::new())),
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
        declared_permissions: Option<&[String]>,
    ) -> Arc<dyn conch_plugin::HostApi> {
        // Store/refresh permission profile for this plugin.
        let profile = match declared_permissions {
            Some(perms) => PermissionProfile::from_declared(perms),
            None => PermissionProfile::deny_all(),
        };
        self.permission_profiles
            .write()
            .insert(name.to_string(), profile);

        let base: Arc<dyn conch_plugin::HostApi> = Arc::new(TauriHostApi {
            name: name.to_string(),
            app_handle: app_handle.clone(),
            bus: Arc::clone(&self.bus),
            panels: Arc::clone(&self.panels),
            views_by_id: Arc::clone(&self.views_by_id),
            pane_to_view: Arc::clone(&self.pane_to_view),
            menu_items: Arc::clone(&self.menu_items),
            pending_dialogs: Arc::clone(&self.pending_dialogs),
        });

        Arc::new(PermissionCheckedHostApi::new(
            base,
            name.to_string(),
            Arc::clone(&self.permission_profiles),
        ))
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

    /// Remove plugin-owned UI/runtime resources.
    /// Returns removed panel handles and removed view ids.
    fn cleanup_plugin_resources(&self, plugin_name: &str) -> (Vec<u64>, Vec<String>) {
        // Collect and remove panels owned by this plugin.
        let mut removed_handles = Vec::new();
        self.panels.write().retain(|handle, info| {
            if info.plugin_name == plugin_name {
                removed_handles.push(*handle);
                false
            } else {
                true
            }
        });

        // Collect and remove docked views owned by this plugin.
        let mut removed_view_ids = Vec::new();
        self.views_by_id.write().retain(|view_id, info| {
            if info.plugin_name == plugin_name {
                removed_view_ids.push(view_id.clone());
                false
            } else {
                true
            }
        });
        if !removed_view_ids.is_empty() {
            let mut pane_to_view = self.pane_to_view.write();
            pane_to_view.retain(|_, vid| !removed_view_ids.contains(vid));
        }

        // Remove menu items registered by this plugin.
        self.menu_items
            .write()
            .retain(|item| item.plugin != plugin_name);

        // Drop pending dialog channels owned by this plugin.
        self.pending_dialogs.lock().drain_for_plugin(plugin_name);

        // Remove runtime permission profile for this plugin.
        self.permission_profiles.write().remove(plugin_name);

        (removed_handles, removed_view_ids)
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
                if let Err(err) = ensure_plugin_api_compatible(
                    &plugin.meta.name,
                    plugin.meta.api_required.as_deref(),
                ) {
                    log::warn!("{err}");
                    continue;
                }
                let normalized_permissions = match normalize_and_validate_permissions(
                    &plugin.meta.name,
                    &plugin.meta.permissions,
                ) {
                    Ok(p) => p,
                    Err(err) => {
                        log::warn!("{err}");
                        continue;
                    }
                };
                let name = plugin.meta.name.clone();
                let host_api = self.make_host_api(&name, app_handle, Some(&normalized_permissions));
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
                let profiles = Arc::clone(&self.permission_profiles);
                let mut found = false;
                for (_, jar_path) in &jar_paths {
                    // Probe the JAR to get its plugin name.
                    match mgr.probe_jar_name(jar_path) {
                        Some(probe_name) if probe_name == *saved_name => {
                            let declared = mgr.probe_jar_permissions(jar_path);
                            let normalized =
                                match normalize_and_validate_permissions(&probe_name, &declared) {
                                    Ok(p) => p,
                                    Err(err) => {
                                        log::warn!("{err}");
                                        found = true;
                                        break;
                                    }
                                };
                            profiles.write().insert(
                                probe_name.clone(),
                                PermissionProfile::from_declared(&normalized),
                            );
                            if let Err(err) = ensure_plugin_api_compatible(
                                saved_name,
                                mgr.probe_jar_api_requirement(jar_path).as_deref(),
                            ) {
                                log::warn!("{err}");
                                found = true;
                                break;
                            }
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
        let host_api = self.make_host_api("java", app_handle, None);
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

    // Deduplicate paths so the same directory isn't scanned twice.
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| {
        let key = p.canonicalize().unwrap_or_else(|_| p.clone());
        seen.insert(key)
    });

    paths
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Plugin manager types
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub(crate) struct DiscoveredPlugin {
    pub name: String,
    pub description: String,
    pub version: String,
    pub plugin_type: String,
    pub source: String, // "lua" or "java"
    pub path: String,
    pub loaded: bool,
    pub permissions: Vec<String>,
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
                permissions: plugin.meta.permissions.clone(),
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
                            permissions: mgr.probe_jar_permissions(&path),
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
        ensure_plugin_api_compatible(&plugin.meta.name, plugin.meta.api_required.as_deref())?;
        let normalized_permissions =
            normalize_and_validate_permissions(&plugin.meta.name, &plugin.meta.permissions)?;

        let host_api = ps.make_host_api(&name, &app, Some(&normalized_permissions));

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
        let profiles = Arc::clone(&ps.permission_profiles);
        if let Some(ref mut mgr) = ps.java_mgr {
            // Check if already loaded (e.g., restored from previous session).
            if mgr.is_loaded(&name) {
                return Ok(());
            }
            let declared = mgr.probe_jar_permissions(std::path::Path::new(&path));
            let normalized = normalize_and_validate_permissions(&name, &declared)?;
            profiles
                .write()
                .insert(name.clone(), PermissionProfile::from_declared(&normalized));
            ensure_plugin_api_compatible(
                &name,
                mgr.probe_jar_api_requirement(std::path::Path::new(&path))
                    .as_deref(),
            )?;
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

            let (removed_handles, removed_view_ids) = ps.cleanup_plugin_resources(&name);
            if !removed_handles.is_empty() {
                let _ = app.emit(
                    "plugin-panels-removed",
                    PluginPanelsRemoved {
                        plugin: name.clone(),
                        handles: removed_handles,
                    },
                );
            }
            if !removed_view_ids.is_empty() {
                let _ = app.emit(
                    "plugin-views-removed",
                    PluginViewsRemoved {
                        plugin: name.clone(),
                        view_ids: removed_view_ids,
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

            let (removed_handles, removed_view_ids) = ps.cleanup_plugin_resources(&name);
            if !removed_handles.is_empty() {
                let _ = app.emit(
                    "plugin-panels-removed",
                    PluginPanelsRemoved {
                        plugin: name.clone(),
                        handles: removed_handles,
                    },
                );
            }
            if !removed_view_ids.is_empty() {
                let _ = app.emit(
                    "plugin-views-removed",
                    PluginViewsRemoved {
                        plugin: name.clone(),
                        view_ids: removed_view_ids,
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
    app_state: tauri::State<'_, crate::TauriState>,
) -> Vec<PluginMenuItem> {
    let mut items = state.lock().menu_items.read().clone();
    let kb = app_state.config.read().conch.keyboard.clone();
    for item in &mut items {
        let key = format!("{}:{}", item.plugin, item.action);
        if let Some(override_keybind) = kb.plugin_shortcuts.get(&key) {
            let trimmed = override_keybind.trim();
            item.keybind = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }
    }
    items
}

/// Trigger a plugin menu action (sends menu_action event to the plugin).
#[tauri::command]
pub(crate) fn trigger_plugin_menu_action(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    plugin_name: String,
    action: String,
) {
    let ps = state.lock();
    let bus = Arc::clone(&ps.bus);
    let event = conch_plugin_sdk::PluginEvent::MenuAction {
        action: action.clone(),
    };
    let json = serde_json::to_string(&event).unwrap_or_default();

    let sent = if let Some(sender) = bus.sender_for(&plugin_name) {
        sender
            .blocking_send(conch_plugin::bus::PluginMail::WidgetEvent { json: json.clone() })
            .is_ok()
    } else {
        false
    };

    // For Java plugins: the shared TauriHostApi registers menu items under
    // a generic name ("java") while each plugin registers on the bus with
    // its real name. Broadcast to all loaded Java plugins so the right one
    // picks it up via its action filter.
    if !sent {
        if let Some(ref mgr) = ps.java_mgr {
            for meta in mgr.loaded_plugins() {
                if let Some(sender) = bus.sender_for(&meta.name) {
                    let _ = sender.blocking_send(conch_plugin::bus::PluginMail::WidgetEvent {
                        json: json.clone(),
                    });
                }
            }
        }
    }
}

/// Get all registered plugin panels.
#[tauri::command]
pub(crate) fn get_plugin_panels(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
) -> Vec<PanelInfo> {
    state.lock().panels.read().values().cloned().collect()
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
        .read()
        .get(&handle)
        .map(|p| p.widgets_json.clone())
}

/// Record the frontend pane/tab binding for a plugin docked view.
#[tauri::command]
pub(crate) fn register_plugin_view_binding(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    view_id: String,
    pane_id: u32,
    tab_id: u32,
) -> bool {
    let state = state.lock();
    let mut views = state.views_by_id.write();
    let Some(info) = views.get_mut(&view_id) else {
        return false;
    };
    info.pane_id = pane_id;
    info.tab_id = tab_id;
    let payload = serde_json::json!({
        "view_id": info.view_id,
        "plugin": info.plugin_name,
        "pane_id": info.pane_id,
        "tab_id": info.tab_id,
        "title": info.title,
        "icon": info.icon,
    });
    let _ = app.emit("plugin-view-opened", payload);
    state.pane_to_view.write().insert(pane_id, view_id);
    true
}

/// Notify backend that a docked view was closed in the frontend.
#[tauri::command]
pub(crate) fn plugin_view_closed(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    view_id: String,
) -> bool {
    let state = state.lock();
    let mut views = state.views_by_id.write();
    let Some(info) = views.remove(&view_id) else {
        return false;
    };
    if info.pane_id != 0 {
        state.pane_to_view.write().remove(&info.pane_id);
    }
    let payload = serde_json::json!({
        "view_id": info.view_id,
        "plugin": info.plugin_name,
        "pane_id": info.pane_id,
        "tab_id": info.tab_id,
    });
    let _ = app.emit("plugin-view-closed", payload);
    true
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
        .send(conch_plugin::bus::PluginMail::RenderRequest {
            view_id: None,
            reply: reply_tx,
        })
        .await
        .map_err(|e| format!("send failed: {e}"))?;
    Ok(reply_rx.await.ok())
}

/// Request a plugin to re-render widgets for a specific view.
#[tauri::command]
pub(crate) async fn request_plugin_view_render(
    state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    plugin_name: String,
    view_id: String,
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
        .send(conch_plugin::bus::PluginMail::RenderRequest {
            view_id: Some(view_id),
            reply: reply_tx,
        })
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
        assert!(state.panels.read().is_empty());
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
    fn search_paths_deduplicates() {
        // The default plugins dir is always included; passing it again as an
        // extra path should not produce duplicates.
        let config_dir = conch_core::config::config_dir();
        let plugins_dir = config_dir.join("plugins");
        let extra = vec![plugins_dir.to_string_lossy().to_string()];
        let paths = plugin_search_paths(&extra);
        let count = paths
            .iter()
            .filter(|p| {
                p.canonicalize().unwrap_or_else(|_| (*p).clone())
                    == plugins_dir
                        .canonicalize()
                        .unwrap_or_else(|_| plugins_dir.clone())
            })
            .count();
        assert_eq!(count, 1, "plugins dir should appear exactly once");
    }

    #[test]
    fn cleanup_removes_panels_for_plugin() {
        let state = PluginState::new(conch_core::config::PluginsConfig::default());

        // Insert panels for two different plugins.
        {
            let mut panels = state.panels.write();
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

        let (removed, removed_views) = state.cleanup_plugin_resources("my-plugin");

        assert_eq!(
            removed.len(),
            2,
            "should remove exactly 2 panels for my-plugin"
        );
        assert!(
            removed_views.is_empty(),
            "no docked views removed in this test"
        );
        assert!(removed.contains(&1));
        assert!(removed.contains(&3));

        let panels = state.panels.read();
        assert_eq!(panels.len(), 1, "only other-plugin panel should remain");
        assert!(panels.contains_key(&2));
    }

    #[test]
    fn cleanup_removes_menu_items_for_plugin() {
        let state = PluginState::new(conch_core::config::PluginsConfig::default());

        {
            let mut items = state.menu_items.write();
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

        let items = state.menu_items.read();
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
        let (removed, removed_views) = state.cleanup_plugin_resources("nonexistent");
        assert!(
            removed.is_empty(),
            "should return empty vec when plugin has no resources"
        );
        assert!(
            removed_views.is_empty(),
            "no docked views should be removed"
        );
    }

    #[test]
    fn api_requirement_accepts_legacy_none() {
        assert!(ensure_plugin_api_compatible("legacy", None).is_ok());
    }

    #[test]
    fn api_requirement_accepts_caret_major_match() {
        assert!(api_requirement_matches("^1"));
        assert!(api_requirement_matches("^1.0"));
    }

    #[test]
    fn api_requirement_rejects_major_mismatch() {
        assert!(!api_requirement_matches("^2.0"));
        assert!(!api_requirement_matches("2.0.0"));
    }

    #[test]
    fn api_requirement_accepts_exact_minor_lte_host() {
        assert!(api_requirement_matches("1"));
        assert!(api_requirement_matches("1.0"));
        assert!(api_requirement_matches("1.0.0"));
    }

    #[test]
    fn api_requirement_rejects_invalid_syntax() {
        assert!(!api_requirement_matches(">=1 <2"));
        assert!(!api_requirement_matches("latest"));
    }

    #[test]
    fn normalize_permissions_rejects_unknown_capability() {
        let out = normalize_and_validate_permissions(
            "x",
            &["ui.menu".into(), "definitely.not.valid".into()],
        );
        assert!(out.is_err());
    }

    #[test]
    fn normalize_permissions_dedupes_and_normalizes_case() {
        let out = normalize_and_validate_permissions(
            "x",
            &[
                "UI.MENU".into(),
                " ui.menu ".into(),
                "clipboard.read".into(),
            ],
        )
        .unwrap();
        assert_eq!(
            out,
            vec!["ui.menu".to_string(), "clipboard.read".to_string()]
        );
    }
}
