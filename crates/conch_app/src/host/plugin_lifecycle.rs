//! Plugin discovery, auto-loading, persistence, and render polling.

use std::collections::HashSet;
use std::path::PathBuf;

use conch_core::config;
use conch_plugin::bus::PluginMail;
use conch_plugin::lua::runner::{DiscoveredLuaPlugin, RunningLuaPlugin};
use conch_plugin_sdk::PanelLocation;
use tokio::sync::oneshot;

use crate::app::ConchApp;
use crate::host::bridge;
use crate::host::plugin_manager_ui::{PluginEntry, PluginSource};

impl ConchApp {
    /// Scan search paths for native and Lua plugins, updating the plugin manager.
    pub(crate) fn discover_plugins(&mut self) {
        let mut entries = Vec::new();
        let configured = self.shared.config.lock().user_config.conch.plugins.search_paths.clone();
        let configured = &configured;

        // Build search directories. Default platform paths are always included;
        // user-configured paths are appended so they can override or supplement.
        let mut dirs = Vec::new();

        // Development paths (only useful when running from the repo).
        dirs.push(PathBuf::from("target/debug"));
        dirs.push(PathBuf::from("target/release"));
        dirs.push(PathBuf::from("examples/plugins"));

        // Exe directory and sibling paths (handles installed builds).
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                // Same directory as the binary (Windows, portable builds).
                dirs.push(exe_dir.to_path_buf());
                // plugins/ subdirectory next to the binary.
                dirs.push(exe_dir.join("plugins"));
                // macOS app bundle: Conch.app/Contents/Plugins/
                if let Some(contents_dir) = exe_dir.parent() {
                    dirs.push(contents_dir.join("Plugins"));
                }
                // Linux: /opt/conch/lib/ when binary is in /opt/conch/bin/
                if let Some(install_root) = exe_dir.parent() {
                    dirs.push(install_root.join("lib"));
                }
            }
        }

        // Standard Linux install path.
        #[cfg(target_os = "linux")]
        {
            dirs.push(PathBuf::from("/opt/conch/lib"));
            dirs.push(PathBuf::from("/usr/lib/conch/plugins"));
        }

        // User plugin directory (~/.config/conch/plugins/ or platform equivalent).
        dirs.push(conch_core::config::config_dir().join("plugins"));

        // Append user-configured search paths (these supplement the defaults).
        for p in configured {
            let expanded = if p.starts_with("~/") {
                dirs::home_dir()
                    .map(|home| home.join(&p[2..]))
                    .unwrap_or_else(|| PathBuf::from(p))
            } else {
                PathBuf::from(p)
            };
            dirs.push(expanded);
        }

        let search_dirs = dirs;

        for dir in &search_dirs {
            if !dir.is_dir() {
                continue;
            }

            let plugins_cfg = self.shared.config.lock().user_config.conch.plugins.clone();

            // Discover native plugins (.dylib/.so/.dll).
            if plugins_cfg.native {
                if let Ok(paths) = conch_plugin::native::library::discover_library_paths(dir) {
                    for path in paths {
                        match unsafe { conch_plugin::native::PluginLibrary::load(&path) } {
                            Ok(lib) => {
                                let meta = unsafe { lib.read_info() };
                                entries.push(PluginEntry {
                                    name: meta.name,
                                    description: meta.description,
                                    version: meta.version,
                                    plugin_type: meta.plugin_type,
                                    panel_location: meta.panel_location,
                                    source: PluginSource::Native,
                                    path,
                                });
                            }
                            Err(_) => {}
                        }
                    }
                }
            }

            // Discover Java plugins (.jar).
            if plugins_cfg.java {
                if let Some(mgr) = &mut self.java_plugin_mgr {
                    for (path, meta) in mgr.discover(dir) {
                        entries.push(PluginEntry {
                            name: meta.name,
                            description: meta.description,
                            version: meta.version,
                            plugin_type: meta.plugin_type,
                            panel_location: meta.panel_location,
                            source: PluginSource::Java,
                            path,
                        });
                    }
                }
            }

            // Discover Lua plugins (.lua).
            if plugins_cfg.lua {
                for plugin in conch_plugin::lua::runner::discover(dir) {
                    entries.push(PluginEntry {
                        name: plugin.meta.name,
                        description: plugin.meta.description,
                        version: plugin.meta.version,
                        plugin_type: plugin.meta.plugin_type,
                        panel_location: plugin.meta.panel_location,
                        source: PluginSource::Lua,
                        path: plugin.path,
                    });
                }
            }
        }

        // Deduplicate by name (keep first occurrence).
        let mut seen = HashSet::new();
        entries.retain(|e| seen.insert(e.name.clone()));

        log::info!("Discovered {} plugins", entries.len());
        for e in &entries {
            log::info!("  - {} v{} ({}) [{}]", e.name, e.version, e.source, e.path.display());
        }

        self.shared.plugin_manager.lock().set_plugins(entries);
    }

    /// Load plugins that were enabled in the previous session.
    pub(crate) fn auto_load_plugins(&mut self) {
        let to_load: Vec<String> = self.shared.config.lock().persistent.loaded_plugins.clone();
        for name in &to_load {
            let entry_info = self.shared.plugin_manager.lock().find_plugin(name)
                .map(|e| (e.source, e.path.clone()));
            if let Some((source, path)) = entry_info {
                match source {
                    PluginSource::Native => {
                        if let Some(mgr) = &mut self.native_plugin_mgr {
                            match mgr.load_plugin(&path) {
                                Ok(meta) => {
                                    log::info!("Auto-loaded native plugin '{}' v{}", meta.name, meta.version);
                                    self.shared.plugin_manager.lock().set_loaded(name, true);
                                }
                                Err(e) => log::warn!("Failed to auto-load plugin '{name}': {e}"),
                            }
                        }
                    }
                    PluginSource::Java => {
                        if let Some(mgr) = &mut self.java_plugin_mgr {
                            match mgr.load_plugin(&path) {
                                Ok(meta) => {
                                    log::info!("Auto-loaded Java plugin '{}' v{}", meta.name, meta.version);
                                    self.shared.plugin_manager.lock().set_loaded(name, true);
                                }
                                Err(e) => log::warn!("Failed to auto-load Java plugin '{name}': {e}"),
                            }
                        }
                    }
                    PluginSource::Lua => {
                        match self.load_lua_plugin(name, &path) {
                            Ok(()) => {
                                log::info!("Auto-loaded Lua plugin '{name}'");
                                self.shared.plugin_manager.lock().set_loaded(name, true);
                            }
                            Err(e) => log::warn!("Failed to auto-load Lua plugin '{name}': {e}"),
                        }
                    }
                }
            } else {
                log::warn!("Previously loaded plugin '{name}' not found during discovery");
            }
        }
    }

    /// Persist the current set of loaded plugin names to state.toml.
    pub(crate) fn save_loaded_plugins(&mut self) {
        let mut loaded: Vec<String> = self.native_plugin_mgr.as_ref()
            .map(|m| m.loaded_plugins().iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        if let Some(mgr) = &self.java_plugin_mgr {
            loaded.extend(mgr.loaded_plugins().iter().map(|p| p.name.clone()));
        }
        // Include Lua plugins.
        for name in self.lua_plugins.keys() {
            loaded.push(name.clone());
        }
        let mut cfg = self.shared.config.lock();
        cfg.persistent.loaded_plugins = loaded;
        let _ = config::save_persistent_state(&cfg.persistent);
    }

    /// Handle a single plugin manager action (load/unload/refresh).
    pub(crate) fn handle_plugin_manager_action(
        &mut self,
        action: crate::host::plugin_manager_ui::PluginManagerAction,
    ) {
        use crate::host::plugin_manager_ui::PluginManagerAction;
        match action {
            PluginManagerAction::Refresh => {
                self.discover_plugins();
            }
            PluginManagerAction::Load(name) => {
                let entry_info = self.shared.plugin_manager.lock().find_plugin(&name)
                    .map(|e| (e.source, e.path.clone()));
                if let Some((source, path)) = entry_info {
                    match source {
                        PluginSource::Native => {
                            if let Some(mgr) = &mut self.native_plugin_mgr {
                                match mgr.load_plugin(&path) {
                                    Ok(meta) => {
                                        log::info!("Loaded plugin '{}' v{}", meta.name, meta.version);
                                        self.shared.plugin_manager.lock().set_loaded(&name, true);
                                        self.save_loaded_plugins();
                                    }
                                    Err(e) => log::error!("Failed to load plugin '{name}': {e}"),
                                }
                            } else {
                                log::warn!("Cannot load native plugin '{name}': native plugins disabled");
                            }
                        }
                        PluginSource::Java => {
                            if let Some(mgr) = &mut self.java_plugin_mgr {
                                match mgr.load_plugin(&path) {
                                    Ok(meta) => {
                                        log::info!("Loaded Java plugin '{}' v{}", meta.name, meta.version);
                                        self.shared.plugin_manager.lock().set_loaded(&name, true);
                                        self.save_loaded_plugins();
                                    }
                                    Err(e) => log::error!("Failed to load Java plugin '{name}': {e}"),
                                }
                            } else {
                                log::warn!("Cannot load Java plugin '{name}': Java plugins disabled");
                            }
                        }
                        PluginSource::Lua => {
                            match self.load_lua_plugin(&name, &path) {
                                Ok(()) => {
                                    log::info!("Loaded Lua plugin '{name}'");
                                    self.shared.plugin_manager.lock().set_loaded(&name, true);
                                    self.save_loaded_plugins();
                                }
                                Err(e) => {
                                    log::error!("Failed to load Lua plugin '{name}': {e}");
                                }
                            }
                        }
                    }
                }
            }
            PluginManagerAction::Unload(name) => {
                // Try Lua first, then Java, then native.
                if self.lua_plugins.contains_key(&name) {
                    self.unload_lua_plugin(&name);
                    self.shared.plugin_manager.lock().set_loaded(&name, false);
                    self.save_loaded_plugins();
                } else {
                    let result: Result<(), String> =
                        if self.java_plugin_mgr.as_ref().map_or(false, |m| m.is_loaded(&name)) {
                            self.java_plugin_mgr.as_mut().unwrap().unload_plugin(&name).map_err(|e| e.to_string())
                        } else if let Some(mgr) = &mut self.native_plugin_mgr {
                            mgr.unload_plugin(&name).map_err(|e| e.to_string())
                        } else {
                            Err(format!("No plugin manager available for '{name}'"))
                        };
                    match result {
                        Ok(()) => {
                            log::info!("Unloaded plugin '{name}'");
                            self.shared.panel_registry.lock().remove_by_plugin(&name);
                            self.render_pending.remove(&name);
                            self.shared.render_cache.lock().remove(&name);
                            self.shared.plugin_manager.lock().set_loaded(&name, false);
                            self.save_loaded_plugins();
                        }
                        Err(e) => log::error!("Failed to unload plugin '{name}': {e}"),
                    }
                }
            }
        }
    }

    /// Load a Lua plugin by reading its source and spawning a runner thread.
    fn load_lua_plugin(&mut self, name: &str, path: &PathBuf) -> Result<(), String> {
        if self.lua_plugins.contains_key(name) {
            return Err(format!("Lua plugin '{name}' is already loaded"));
        }

        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let meta = conch_plugin::lua::metadata::parse_lua_metadata(&source);
        let discovered = DiscoveredLuaPlugin {
            path: path.clone(),
            source,
            meta,
        };

        // Register on the bus and get the mailbox.
        let mailbox_rx = self.shared.plugin_bus.register_plugin(name);
        let mailbox_tx = self.shared.plugin_bus.sender_for(name).unwrap();
        let host_api_ptr: *const conch_plugin_sdk::HostApi = self.lua_host_api.as_ref()
            .map(|b| &**b as *const _)
            .or_else(|| self.native_plugin_mgr.as_ref().map(|m| m.host_api_ptr()))
            .ok_or_else(|| "No HostApi available for Lua plugins".to_string())?;

        // Wrap the C vtable in the safe trait adapter for the Lua runner.
        let host_api: std::sync::Arc<dyn conch_plugin::HostApi> = std::sync::Arc::new(
            unsafe { conch_plugin::CHostApiAdapter::new(name.to_string(), &*host_api_ptr) }
        );

        let running = conch_plugin::lua::runner::spawn_lua_plugin(
            &discovered,
            host_api,
            mailbox_tx,
            mailbox_rx,
        )?;

        self.lua_plugins.insert(name.to_string(), running);
        Ok(())
    }

    /// Unload a Lua plugin: send shutdown, join thread, clean up.
    fn unload_lua_plugin(&mut self, name: &str) {
        if let Some(mut running) = self.lua_plugins.remove(name) {
            // Send shutdown signal.
            if running.sender.try_send(PluginMail::Shutdown).is_err() {
                log::warn!("Lua plugin [{name}]: failed to send shutdown");
            }
            // Wait for thread to exit.
            if let Some(handle) = running.thread.take() {
                let _ = handle.join();
            }
            // Clean up bus, panels, caches.
            self.shared.plugin_bus.unregister_plugin(name);
            self.shared.panel_registry.lock().remove_by_plugin(name);
            self.render_pending.remove(name);
            self.shared.render_cache.lock().remove(name);
            log::info!("Unloaded Lua plugin '{name}'");
        }
    }

    /// Minimum interval between render requests to the same plugin.
    /// Caps plugin render polling to ~4fps when idle, which is plenty for
    /// panel UIs. User interactions (widget events) trigger immediate
    /// re-renders via the event path.
    const RENDER_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

    /// Poll pending render requests and fire new ones for panel plugins.
    pub(crate) fn poll_plugin_renders(&mut self) {
        // Check pending render responses.
        let pending_names: Vec<String> = self.render_pending.keys().cloned().collect();
        for name in pending_names {
            let ready = {
                let rx = self.render_pending.get_mut(&name).unwrap();
                match rx.try_recv() {
                    Ok(json) => Some(json),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(oneshot::error::TryRecvError::Closed) => {
                        // Channel closed — remove the pending entry.
                        Some("[]".to_string())
                    }
                }
            };
            if let Some(json) = ready {
                self.shared.render_cache.lock().insert(name.clone(), json);
                self.render_pending.remove(&name);
            }
        }

        // Fire new render requests for loaded panel plugins, throttled to
        // avoid driving the app at 60fps when plugins have nothing new.
        let now = std::time::Instant::now();
        let panels: Vec<(String, String)> = {
            let reg = self.shared.panel_registry.lock();
            reg.panels()
                .map(|(_, info)| (info.plugin_name.clone(), info.name.clone()))
                .collect()
        };
        // Drain force-render requests (from plugin keybinding dispatch).
        let force_set: std::collections::HashSet<String> = {
            let mut f = self.shared.force_render.lock();
            f.drain(..).collect()
        };
        for (plugin_name, _panel_name) in panels {
            if self.render_pending.contains_key(&plugin_name) {
                continue;
            }
            // Skip throttle for plugins with pending force-render.
            if !force_set.contains(&plugin_name) {
                if let Some(last) = self.render_last_request.get(&plugin_name) {
                    if now.duration_since(*last) < Self::RENDER_POLL_INTERVAL {
                        continue;
                    }
                }
            }
            if let Some(sender) = self.shared.plugin_bus.sender_for(&plugin_name) {
                let (tx, rx) = oneshot::channel();
                if sender.try_send(PluginMail::RenderRequest { reply: tx }).is_ok() {
                    self.render_pending.insert(plugin_name.clone(), rx);
                    self.render_last_request.insert(plugin_name, now);
                }
            }
        }
    }
}
