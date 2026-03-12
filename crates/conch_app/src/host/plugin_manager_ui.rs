//! Plugin manager UI — discovery, load/unload controls.
//!
//! Renders a table of all discovered plugins (native + Lua) with their status
//! and provides load/unload controls. Uses the centralized UiTheme for all
//! colors and metrics.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use conch_plugin_sdk::{PanelLocation, PluginType};

use crate::ui_theme::UiTheme;

/// Information about a discovered plugin (from any source).
#[derive(Debug, Clone)]
pub struct PluginEntry {
    pub name: String,
    pub description: String,
    pub version: String,
    pub plugin_type: PluginType,
    pub panel_location: PanelLocation,
    /// "native" or "lua".
    pub source: PluginSource,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginSource {
    Native,
    Lua,
}

impl std::fmt::Display for PluginSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginSource::Native => write!(f, "Native"),
            PluginSource::Lua => write!(f, "Lua"),
        }
    }
}

/// State for the plugin manager UI.
pub struct PluginManagerState {
    /// All discovered plugins.
    plugins: Vec<PluginEntry>,
    /// Names of currently loaded plugins.
    loaded: HashSet<String>,
    /// Pending toggle changes (name → desired loaded state).
    pending: HashMap<String, bool>,
}

/// Actions emitted by the plugin manager UI.
#[derive(Debug, Clone)]
pub enum PluginManagerAction {
    /// Load a plugin by name.
    Load(String),
    /// Unload a plugin by name.
    Unload(String),
    /// Rescan for plugins.
    Refresh,
}

impl Default for PluginManagerState {
    fn default() -> Self {
        Self {
            plugins: Vec::new(),
            loaded: HashSet::new(),
            pending: HashMap::new(),
        }
    }
}

impl PluginManagerState {
    /// Look up a plugin entry by name.
    pub fn find_plugin(&self, name: &str) -> Option<&PluginEntry> {
        self.plugins.iter().find(|p| p.name == name)
    }

    /// Replace the list of discovered plugins.
    pub fn set_plugins(&mut self, plugins: Vec<PluginEntry>) {
        self.plugins = plugins;
        self.pending.clear();
    }

    /// Mark a plugin as loaded.
    pub fn set_loaded(&mut self, name: &str, loaded: bool) {
        if loaded {
            self.loaded.insert(name.to_string());
        } else {
            self.loaded.remove(name);
        }
        self.pending.remove(name);
    }

    /// Whether any changes are pending.
    pub fn has_pending_changes(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Get pending actions to apply.
    pub fn take_pending_actions(&mut self) -> Vec<PluginManagerAction> {
        self.pending
            .drain()
            .map(|(name, load)| {
                if load {
                    PluginManagerAction::Load(name)
                } else {
                    PluginManagerAction::Unload(name)
                }
            })
            .collect()
    }

    /// Render the plugin manager as a table.
    pub fn show(&mut self, ui: &mut egui::Ui, theme: &UiTheme) -> Vec<PluginManagerAction> {
        let mut actions = Vec::new();

        // Header row: title + scan button.
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Plugin Manager")
                    .size(theme.font_normal + 2.0)
                    .color(theme.text)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let scan_btn = egui::Button::new(
                    egui::RichText::new("Scan").size(theme.font_normal),
                )
                .corner_radius(egui::CornerRadius::same(theme.rounding))
                .fill(theme.surface_raised);
                if ui.add(scan_btn).clicked() {
                    actions.push(PluginManagerAction::Refresh);
                }
            });
        });

        ui.add_space(6.0);

        if self.plugins.is_empty() {
            ui.label(
                egui::RichText::new("No plugins discovered. Click Scan to search.")
                    .size(theme.font_normal)
                    .color(theme.text_muted),
            );
            return actions;
        }

        // Table header.
        let row_height = theme.font_normal + 6.0;

        egui::Grid::new("plugin_table_header")
            .num_columns(5)
            .spacing([8.0, 0.0])
            .min_col_width(0.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("").size(theme.font_small)); // checkbox col
                ui.label(egui::RichText::new("Name").size(theme.font_small).color(theme.text_secondary).strong());
                ui.label(egui::RichText::new("Description").size(theme.font_small).color(theme.text_secondary).strong());
                ui.label(egui::RichText::new("Version").size(theme.font_small).color(theme.text_secondary).strong());
                ui.label(egui::RichText::new("Type").size(theme.font_small).color(theme.text_secondary).strong());
                ui.end_row();
            });

        ui.separator();

        // Scrollable plugin rows — constrain height so it doesn't blow up.
        let available = ui.available_height() - if self.has_pending_changes() { 40.0 } else { 0.0 };
        let max_h = available.max(row_height * 3.0); // at least 3 rows visible

        egui::ScrollArea::vertical()
            .max_height(max_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("plugin_table_body")
                    .num_columns(5)
                    .spacing([8.0, 4.0])
                    .min_col_width(0.0)
                    .striped(true)
                    .show(ui, |ui| {
                        for plugin in &self.plugins {
                            let is_loaded = self.loaded.contains(&plugin.name);
                            let pending_state = self.pending.get(&plugin.name).copied();
                            let effective_loaded = pending_state.unwrap_or(is_loaded);

                            // Enabled checkbox.
                            let mut checked = effective_loaded;
                            if ui.checkbox(&mut checked, "").changed() {
                                if checked != is_loaded {
                                    self.pending.insert(plugin.name.clone(), checked);
                                } else {
                                    self.pending.remove(&plugin.name);
                                }
                            }

                            // Name — colored by status.
                            let name_color = if pending_state.is_some() {
                                theme.warn
                            } else if is_loaded {
                                theme.accent
                            } else {
                                theme.text
                            };
                            ui.label(
                                egui::RichText::new(&plugin.name)
                                    .size(theme.font_normal)
                                    .color(name_color),
                            );

                            // Description.
                            ui.label(
                                egui::RichText::new(&plugin.description)
                                    .size(theme.font_small)
                                    .color(theme.text_secondary),
                            );

                            // Version.
                            ui.label(
                                egui::RichText::new(&plugin.version)
                                    .size(theme.font_small)
                                    .color(theme.text_secondary),
                            );

                            // Type / Source.
                            let type_label = match plugin.plugin_type {
                                PluginType::Action => "Action",
                                PluginType::Panel => "Panel",
                            };
                            ui.label(
                                egui::RichText::new(format!("{} / {}", type_label, plugin.source))
                                    .size(theme.font_small)
                                    .color(theme.text_muted),
                            );

                            ui.end_row();
                        }
                    });
            });

        // Apply button.
        if self.has_pending_changes() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let apply_btn = egui::Button::new(
                        egui::RichText::new("Apply Changes")
                            .size(theme.font_normal)
                            .color(theme.text),
                    )
                    .corner_radius(egui::CornerRadius::same(theme.rounding))
                    .fill(theme.accent);
                    if ui.add(apply_btn).clicked() {
                        actions.extend(self.take_pending_actions());
                    }
                });
            });
        }

        actions
    }
}

/// Show the plugin manager as a floating egui::Window.
pub fn show_plugin_manager_window(
    ctx: &egui::Context,
    open: &mut bool,
    state: &mut PluginManagerState,
    theme: &UiTheme,
) -> Vec<PluginManagerAction> {
    let mut actions = Vec::new();

    egui::Window::new("Plugin Manager")
        .open(open)
        .default_width(640.0)
        .default_height(340.0)
        .min_width(480.0)
        .min_height(200.0)
        .resizable(true)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(theme.surface)
                .stroke(egui::Stroke::new(1.0, theme.border))
                .inner_margin(12.0),
        )
        .show(ctx, |ui| {
            actions = state.show(ui, theme);
        });

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(name: &str, source: PluginSource) -> PluginEntry {
        PluginEntry {
            name: name.into(),
            description: format!("{name} plugin"),
            version: "1.0.0".into(),
            plugin_type: PluginType::Panel,
            panel_location: PanelLocation::Left,
            source,
            path: PathBuf::from(format!("/plugins/{name}")),
        }
    }

    #[test]
    fn default_state() {
        let state = PluginManagerState::default();
        assert!(state.plugins.is_empty());
        assert!(state.loaded.is_empty());
        assert!(!state.has_pending_changes());
    }

    #[test]
    fn set_plugins() {
        let mut state = PluginManagerState::default();
        state.set_plugins(vec![
            make_entry("ssh", PluginSource::Native),
            make_entry("system-info", PluginSource::Lua),
        ]);
        assert_eq!(state.plugins.len(), 2);
    }

    #[test]
    fn set_loaded() {
        let mut state = PluginManagerState::default();
        state.set_loaded("ssh", true);
        assert!(state.loaded.contains("ssh"));
        state.set_loaded("ssh", false);
        assert!(!state.loaded.contains("ssh"));
    }

    #[test]
    fn pending_changes() {
        let mut state = PluginManagerState::default();
        state.set_plugins(vec![make_entry("ssh", PluginSource::Native)]);
        assert!(!state.has_pending_changes());

        state.pending.insert("ssh".into(), true);
        assert!(state.has_pending_changes());

        let actions = state.take_pending_actions();
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], PluginManagerAction::Load(n) if n == "ssh"));
        assert!(!state.has_pending_changes());
    }

    #[test]
    fn pending_unload() {
        let mut state = PluginManagerState::default();
        state.set_loaded("ssh", true);
        state.pending.insert("ssh".into(), false);

        let actions = state.take_pending_actions();
        assert!(matches!(&actions[0], PluginManagerAction::Unload(n) if n == "ssh"));
    }

    #[test]
    fn plugin_source_display() {
        assert_eq!(format!("{}", PluginSource::Native), "Native");
        assert_eq!(format!("{}", PluginSource::Lua), "Lua");
    }
}
