//! Panel containers — left, right, and bottom panels for plugin-registered tabs.
//!
//! Panels auto-hide when no tabs are registered and support resizable borders.
//! Multiple plugins can register at the same location, producing a tab strip.

use std::collections::HashMap;

use conch_plugin_sdk::widgets::{Widget, WidgetEvent};
use conch_plugin_sdk::PanelLocation;

use super::widget_renderer::{self, RendererState};

/// Default panel width (left/right) in points.
const DEFAULT_PANEL_WIDTH: f32 = 260.0;
/// Minimum panel width in points.
const MIN_PANEL_WIDTH: f32 = 120.0;
/// Maximum panel width in points.
const MAX_PANEL_WIDTH: f32 = 600.0;
/// Default bottom panel height in points.
const DEFAULT_BOTTOM_HEIGHT: f32 = 200.0;
/// Minimum bottom panel height.
const MIN_BOTTOM_HEIGHT: f32 = 80.0;
/// Maximum bottom panel height.
const MAX_BOTTOM_HEIGHT: f32 = 500.0;

/// A single tab registered by a plugin in a panel location.
#[derive(Debug)]
pub struct PanelTab {
    /// Display name shown in the tab strip.
    pub name: String,
    /// Icon name (for future icon rendering).
    pub icon: Option<String>,
    /// Plugin name that owns this tab.
    pub plugin_name: String,
    /// Cached widget tree from the plugin's last render.
    pub widgets: Vec<Widget>,
    /// Per-tab renderer state (text input values, etc.).
    pub renderer_state: RendererState,
}

/// State for all three panel locations.
pub struct PanelState {
    /// All registered tabs, keyed by a unique tab ID.
    tabs: HashMap<String, PanelTab>,
    /// Tab IDs for each location, in registration order.
    left_tabs: Vec<String>,
    right_tabs: Vec<String>,
    bottom_tabs: Vec<String>,
    /// Active tab ID per location.
    active_left: Option<String>,
    active_right: Option<String>,
    active_bottom: Option<String>,
    /// Resizable widths/heights.
    left_width: f32,
    right_width: f32,
    bottom_height: f32,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            tabs: HashMap::new(),
            left_tabs: Vec::new(),
            right_tabs: Vec::new(),
            bottom_tabs: Vec::new(),
            active_left: None,
            active_right: None,
            active_bottom: None,
            left_width: DEFAULT_PANEL_WIDTH,
            right_width: DEFAULT_PANEL_WIDTH,
            bottom_height: DEFAULT_BOTTOM_HEIGHT,
        }
    }
}

/// Events collected from all panels during a frame.
pub struct PanelOutput {
    /// (plugin_name, events) for each plugin that had widget interactions.
    pub plugin_events: Vec<(String, Vec<WidgetEvent>)>,
}

impl PanelState {
    /// Register a new tab in the given panel location.
    ///
    /// `tab_id` should be unique (e.g., `"{plugin_name}.{panel_handle}"`).
    pub fn register_tab(
        &mut self,
        tab_id: String,
        location: PanelLocation,
        name: String,
        icon: Option<String>,
        plugin_name: String,
    ) {
        let tab = PanelTab {
            name,
            icon,
            plugin_name,
            widgets: Vec::new(),
            renderer_state: RendererState::default(),
        };
        self.tabs.insert(tab_id.clone(), tab);

        let (tab_list, active) = match location {
            PanelLocation::Left => (&mut self.left_tabs, &mut self.active_left),
            PanelLocation::Right => (&mut self.right_tabs, &mut self.active_right),
            PanelLocation::Bottom => (&mut self.bottom_tabs, &mut self.active_bottom),
            PanelLocation::None => return,
        };
        tab_list.push(tab_id.clone());
        if active.is_none() {
            *active = Some(tab_id);
        }
    }

    /// Remove a tab (e.g., when a plugin is unloaded).
    pub fn remove_tab(&mut self, tab_id: &str) {
        self.tabs.remove(tab_id);

        for (tab_list, active) in [
            (&mut self.left_tabs, &mut self.active_left),
            (&mut self.right_tabs, &mut self.active_right),
            (&mut self.bottom_tabs, &mut self.active_bottom),
        ] {
            tab_list.retain(|id| id != tab_id);
            if active.as_deref() == Some(tab_id) {
                *active = tab_list.first().cloned();
            }
        }
    }

    /// Remove all tabs belonging to a plugin.
    pub fn remove_plugin_tabs(&mut self, plugin_name: &str) {
        let ids: Vec<String> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.plugin_name == plugin_name)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.remove_tab(&id);
        }
    }

    /// Update the cached widget tree for a tab.
    pub fn set_widgets(&mut self, tab_id: &str, widgets: Vec<Widget>) {
        if let Some(tab) = self.tabs.get_mut(tab_id) {
            tab.widgets = widgets;
        }
    }

    /// Whether any tabs are registered at a location.
    pub fn has_tabs(&self, location: PanelLocation) -> bool {
        match location {
            PanelLocation::Left => !self.left_tabs.is_empty(),
            PanelLocation::Right => !self.right_tabs.is_empty(),
            PanelLocation::Bottom => !self.bottom_tabs.is_empty(),
            PanelLocation::None => false,
        }
    }

    /// Show all panels and return collected events.
    ///
    /// Call this from the main `update()` loop, **before** `CentralPanel`.
    pub fn show(&mut self, ctx: &egui::Context) -> PanelOutput {
        let mut output = PanelOutput {
            plugin_events: Vec::new(),
        };

        // Left panel.
        if !self.left_tabs.is_empty() {
            let tab_ids = self.left_tabs.clone();
            egui::SidePanel::left("plugin_panel_left")
                .default_width(self.left_width)
                .width_range(MIN_PANEL_WIDTH..=MAX_PANEL_WIDTH)
                .resizable(true)
                .show(ctx, |ui| {
                    self.left_width = ui.available_width();
                    show_panel_content(
                        ui,
                        &mut self.tabs,
                        &tab_ids,
                        &mut self.active_left,
                        &mut output,
                    );
                });
        }

        // Right panel.
        if !self.right_tabs.is_empty() {
            let tab_ids = self.right_tabs.clone();
            egui::SidePanel::right("plugin_panel_right")
                .default_width(self.right_width)
                .width_range(MIN_PANEL_WIDTH..=MAX_PANEL_WIDTH)
                .resizable(true)
                .show(ctx, |ui| {
                    self.right_width = ui.available_width();
                    show_panel_content(
                        ui,
                        &mut self.tabs,
                        &tab_ids,
                        &mut self.active_right,
                        &mut output,
                    );
                });
        }

        // Bottom panel.
        if !self.bottom_tabs.is_empty() {
            let tab_ids = self.bottom_tabs.clone();
            egui::TopBottomPanel::bottom("plugin_panel_bottom")
                .default_height(self.bottom_height)
                .height_range(MIN_BOTTOM_HEIGHT..=MAX_BOTTOM_HEIGHT)
                .resizable(true)
                .show(ctx, |ui| {
                    self.bottom_height = ui.available_height();
                    show_panel_content(
                        ui,
                        &mut self.tabs,
                        &tab_ids,
                        &mut self.active_bottom,
                        &mut output,
                    );
                });
        }

        output
    }
}

/// Render content for a single panel location (free function to avoid borrow conflicts).
fn show_panel_content(
    ui: &mut egui::Ui,
    tabs: &mut HashMap<String, PanelTab>,
    tab_ids: &[String],
    active: &mut Option<String>,
    output: &mut PanelOutput,
) {
    // Tab strip (only if multiple tabs).
    if tab_ids.len() > 1 {
        ui.horizontal(|ui| {
            for tab_id in tab_ids {
                if let Some(tab) = tabs.get(tab_id) {
                    let is_active = active.as_deref() == Some(tab_id.as_str());
                    if ui.selectable_label(is_active, &tab.name).clicked() {
                        *active = Some(tab_id.clone());
                    }
                }
            }
        });
        ui.separator();
    }

    // Render the active tab's widget tree.
    let active_id = active.clone();
    if let Some(tab_id) = &active_id {
        if let Some(tab) = tabs.get_mut(tab_id) {
            let widgets = tab.widgets.clone();
            let plugin_name = tab.plugin_name.clone();
            let events = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    widget_renderer::render_widgets(ui, &widgets, &mut tab.renderer_state)
                })
                .inner;

            if !events.is_empty() {
                output.plugin_events.push((plugin_name, events));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_has_no_tabs() {
        let state = PanelState::default();
        assert!(!state.has_tabs(PanelLocation::Left));
        assert!(!state.has_tabs(PanelLocation::Right));
        assert!(!state.has_tabs(PanelLocation::Bottom));
        assert!(!state.has_tabs(PanelLocation::None));
    }

    #[test]
    fn register_tab_left() {
        let mut state = PanelState::default();
        state.register_tab(
            "ssh.panel".into(),
            PanelLocation::Left,
            "SSH".into(),
            None,
            "ssh".into(),
        );
        assert!(state.has_tabs(PanelLocation::Left));
        assert_eq!(state.active_left.as_deref(), Some("ssh.panel"));
    }

    #[test]
    fn register_multiple_tabs_first_is_active() {
        let mut state = PanelState::default();
        state.register_tab("a".into(), PanelLocation::Right, "A".into(), None, "a".into());
        state.register_tab("b".into(), PanelLocation::Right, "B".into(), None, "b".into());
        assert_eq!(state.right_tabs.len(), 2);
        assert_eq!(state.active_right.as_deref(), Some("a"));
    }

    #[test]
    fn remove_tab() {
        let mut state = PanelState::default();
        state.register_tab("x".into(), PanelLocation::Left, "X".into(), None, "x".into());
        state.remove_tab("x");
        assert!(!state.has_tabs(PanelLocation::Left));
        assert!(state.active_left.is_none());
    }

    #[test]
    fn remove_active_tab_selects_next() {
        let mut state = PanelState::default();
        state.register_tab("a".into(), PanelLocation::Left, "A".into(), None, "pa".into());
        state.register_tab("b".into(), PanelLocation::Left, "B".into(), None, "pb".into());
        state.remove_tab("a");
        assert_eq!(state.active_left.as_deref(), Some("b"));
    }

    #[test]
    fn remove_plugin_tabs() {
        let mut state = PanelState::default();
        state.register_tab("ssh.1".into(), PanelLocation::Left, "S1".into(), None, "ssh".into());
        state.register_tab("ssh.2".into(), PanelLocation::Right, "S2".into(), None, "ssh".into());
        state.register_tab("git.1".into(), PanelLocation::Left, "G1".into(), None, "git".into());

        state.remove_plugin_tabs("ssh");

        assert!(!state.tabs.contains_key("ssh.1"));
        assert!(!state.tabs.contains_key("ssh.2"));
        assert!(state.tabs.contains_key("git.1"));
    }

    #[test]
    fn set_widgets_updates_tab() {
        let mut state = PanelState::default();
        state.register_tab("t".into(), PanelLocation::Left, "T".into(), None, "p".into());
        assert!(state.tabs["t"].widgets.is_empty());

        state.set_widgets("t", vec![Widget::Separator]);
        assert_eq!(state.tabs["t"].widgets.len(), 1);
    }

    #[test]
    fn set_widgets_missing_tab_is_noop() {
        let mut state = PanelState::default();
        state.set_widgets("nonexistent", vec![Widget::Separator]);
        // No panic.
    }

    #[test]
    fn register_tab_none_location_ignored() {
        let mut state = PanelState::default();
        state.register_tab("x".into(), PanelLocation::None, "X".into(), None, "x".into());
        // Tab is in the map but not in any location list.
        assert!(state.tabs.contains_key("x"));
        assert!(!state.has_tabs(PanelLocation::Left));
        assert!(!state.has_tabs(PanelLocation::Right));
        assert!(!state.has_tabs(PanelLocation::Bottom));
    }

    #[test]
    fn default_sizes() {
        let state = PanelState::default();
        assert_eq!(state.left_width, DEFAULT_PANEL_WIDTH);
        assert_eq!(state.right_width, DEFAULT_PANEL_WIDTH);
        assert_eq!(state.bottom_height, DEFAULT_BOTTOM_HEIGHT);
    }
}
