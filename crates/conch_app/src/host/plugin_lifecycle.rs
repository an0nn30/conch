//! Plugin discovery, auto-loading, persistence, and render polling.

use std::collections::HashSet;
use std::path::PathBuf;

use conch_core::config;
use conch_plugin::bus::PluginMail;
use conch_plugin_sdk::PanelLocation;
use tokio::sync::oneshot;

use crate::app::ConchApp;
use crate::host::plugin_manager_ui::{PluginEntry, PluginSource};

impl ConchApp {
    /// Scan search paths for native and Lua plugins, updating the plugin manager.
    pub(crate) fn discover_plugins(&mut self) {
        let mut entries = Vec::new();
        let configured = &self.state.user_config.conch.plugins.search_paths;

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
        if let Some(config_dir) = dirs::config_dir() {
            dirs.push(config_dir.join("conch").join("plugins"));
        }

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

            // Discover native plugins (.dylib/.so/.dll).
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
                        Err(_) => {} // Not a valid Conch plugin.
                    }
                }
            }

            // Discover Lua plugins (.lua).
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

        // Deduplicate by name (keep first occurrence).
        let mut seen = HashSet::new();
        entries.retain(|e| seen.insert(e.name.clone()));

        log::info!("Discovered {} plugins", entries.len());
        for e in &entries {
            log::info!("  - {} v{} ({}) [{}]", e.name, e.version, e.source, e.path.display());
        }

        self.plugin_manager.set_plugins(entries);
    }

    /// Load plugins that were enabled in the previous session.
    pub(crate) fn auto_load_plugins(&mut self) {
        let to_load: Vec<String> = self.state.persistent.loaded_plugins.clone();
        for name in &to_load {
            if let Some(entry) = self.plugin_manager.find_plugin(name) {
                let path = entry.path.clone();
                match self.native_plugin_mgr.load_plugin(&path) {
                    Ok(meta) => {
                        log::info!("Auto-loaded plugin '{}' v{}", meta.name, meta.version);
                        self.plugin_manager.set_loaded(name, true);
                    }
                    Err(e) => {
                        log::warn!("Failed to auto-load plugin '{name}': {e}");
                    }
                }
            } else {
                log::warn!("Previously loaded plugin '{name}' not found during discovery");
            }
        }
    }

    /// Persist the current set of loaded plugin names to state.toml.
    pub(crate) fn save_loaded_plugins(&mut self) {
        let loaded: Vec<String> = self
            .native_plugin_mgr
            .loaded_plugins()
            .iter()
            .map(|m| m.name.clone())
            .collect();
        self.state.persistent.loaded_plugins = loaded;
        let _ = config::save_persistent_state(&self.state.persistent);
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
                if let Some(entry) = self.plugin_manager.find_plugin(&name) {
                    let path = entry.path.clone();
                    match self.native_plugin_mgr.load_plugin(&path) {
                        Ok(meta) => {
                            log::info!("Loaded plugin '{}' v{}", meta.name, meta.version);
                            self.plugin_manager.set_loaded(&name, true);
                            self.save_loaded_plugins();
                        }
                        Err(e) => {
                            log::error!("Failed to load plugin '{name}': {e}");
                        }
                    }
                }
            }
            PluginManagerAction::Unload(name) => {
                match self.native_plugin_mgr.unload_plugin(&name) {
                    Ok(()) => {
                        log::info!("Unloaded plugin '{name}'");
                        self.panel_registry.lock().remove_by_plugin(&name);
                        self.render_pending.remove(&name);
                        self.render_cache.remove(&name);
                        self.plugin_manager.set_loaded(&name, false);
                        self.save_loaded_plugins();
                    }
                    Err(e) => {
                        log::error!("Failed to unload plugin '{name}': {e}");
                    }
                }
            }
        }
    }

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
                self.render_cache.insert(name.clone(), json);
                self.render_pending.remove(&name);
            }
        }

        // Fire new render requests for loaded panel plugins that don't have a pending request.
        let panels: Vec<(String, String)> = {
            let reg = self.panel_registry.lock();
            reg.panels()
                .map(|(_, info)| (info.plugin_name.clone(), info.name.clone()))
                .collect()
        };
        for (plugin_name, _panel_name) in panels {
            if self.render_pending.contains_key(&plugin_name) {
                continue; // Already waiting.
            }
            if let Some(sender) = self.plugin_bus.sender_for(&plugin_name) {
                let (tx, rx) = oneshot::channel();
                if sender.try_send(PluginMail::RenderRequest { reply: tx }).is_ok() {
                    self.render_pending.insert(plugin_name, rx);
                }
            }
        }
    }
}
